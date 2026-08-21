"""Independent Decimal witness for the frozen R8A Portfolio360 point-in-time metrics.

The witness consumes only the public fixture schema and Python's Decimal primitives. It does
not import FICANT, a generated contract, or a production aggregation helper. Position-level
Rates and PortfolioRisk facts are treated as already verified public inputs; this module proves
only the R8A aggregation, weighting, benchmark-difference, rounding, and fail-closed rules.
"""

from __future__ import annotations

from decimal import Context, Decimal, InvalidOperation, ROUND_HALF_EVEN, localcontext
from typing import Any


CONTEXT = Context(prec=80, rounding=ROUND_HALF_EVEN)
ZERO = Decimal(0)
REQUIRED_CONVENTION = {
    "schema_id": "ficant.portfolio-metric-convention.v1",
    "ytm_weighting": "MARKET_VALUE_TIMES_MODIFIED_DURATION",
    "duration_weighting": "MARKET_VALUE",
    "convexity_weighting": "MARKET_VALUE",
    "coupon_weighting": "NOTIONAL",
    "remaining_life_weighting": "NOTIONAL",
    "rounding": "TIES_TO_EVEN",
    "freshness_limit_seconds": "86400",
}
WEIGHTED_METRICS = (
    "weighted_ytm",
    "modified_duration",
    "convexity",
    "weighted_coupon_rate",
    "weighted_remaining_years",
)
BASIC_METRIC_ORDER = (
    "market_value",
    "economic_pnl",
    *WEIGHTED_METRICS,
    "dv01",
)


def _required(mapping: dict[str, Any], key: str, path: str) -> Any:
    if key not in mapping or mapping[key] is None or mapping[key] == "":
        raise ValueError(f"{path}.{key} is required")
    return mapping[key]


def _decimal(value: Any, path: str) -> Decimal:
    if not isinstance(value, str):
        raise ValueError(f"{path} must be a Decimal string")
    try:
        result = Decimal(value)
    except InvalidOperation as error:
        raise ValueError(f"{path} must be a finite Decimal string") from error
    if not result.is_finite():
        raise ValueError(f"{path} must be a finite Decimal string")
    return result


def _plain_decimal(value: Decimal) -> str:
    if value == ZERO:
        value = abs(value)
    return format(value, "f")


def render_decimal(value: Decimal, scale: int) -> str:
    """Render one public DecimalValue using the frozen ties-to-even convention."""

    if not isinstance(scale, int) or isinstance(scale, bool) or scale < 0 or scale > 18:
        raise ValueError("output_scale must be an integer from 0 through 18")
    if not value.is_finite():
        raise ValueError("cannot render a non-finite Decimal")
    quantum = Decimal(1).scaleb(-scale)
    try:
        with localcontext(CONTEXT):
            rounded = value.quantize(quantum, rounding=ROUND_HALF_EVEN)
    except InvalidOperation as error:
        raise ValueError("Decimal cannot be represented at the requested scale") from error
    if rounded == ZERO:
        rounded = abs(rounded)
    return format(rounded, "f")


def _validate_hash(value: Any, path: str) -> None:
    if not isinstance(value, str) or not value.startswith("sha256:") or len(value) != 71:
        raise ValueError(f"{path} must be an exact sha256 binding")


def _validate_ref(
    reference: dict[str, Any], path: str, identity_key: str, *, require_times: bool = False
) -> None:
    _required(reference, identity_key, path)
    _required(reference, "version", path) if identity_key != "snapshot_id" else None
    content_hash = _required(reference, "content_hash", path)
    _validate_hash(content_hash, f"{path}.content_hash")
    if require_times:
        _required(reference, "observed_at", path)
        _required(reference, "visible_at", path)


def _validate_authority(inputs: dict[str, Any]) -> tuple[int, str]:
    if _required(inputs, "schema_id", "inputs") != (
        "ficant.portfolio360.metric-oracle-input.v1"
    ):
        raise ValueError("unsupported input schema_id")
    scale = _required(inputs, "output_scale", "inputs")
    if not isinstance(scale, int) or isinstance(scale, bool) or scale < 0 or scale > 18:
        raise ValueError("output_scale must be an integer from 0 through 18")

    authority = _required(inputs, "authority", "inputs")
    _required(authority, "owner", "authority")
    subject_ref = _required(authority, "subject_ref", "authority")
    _validate_ref(subject_ref, "authority.subject_ref", "subject_id")
    _required(authority, "valuation_at", "authority")
    _required(authority, "knowledge_at", "authority")
    currency_unit = _required(authority, "currency_unit", "authority")
    if currency_unit != "CNY":
        raise ValueError("R8A Decimal fixture authority must use CNY")

    convention_ref = _required(authority, "metric_convention_ref", "authority")
    _validate_ref(
        convention_ref,
        "authority.metric_convention_ref",
        "convention_id",
    )
    convention = _required(authority, "metric_convention", "authority")
    convention_hash = _required(convention, "content_hash", "authority.metric_convention")
    _validate_hash(convention_hash, "authority.metric_convention.content_hash")
    if convention_hash != convention_ref["content_hash"]:
        raise ValueError("metric convention content_hash drift")
    for key, expected in REQUIRED_CONVENTION.items():
        actual = _required(convention, key, "authority.metric_convention")
        if actual != expected:
            raise ValueError(f"unsupported metric convention value for {key}")
    return scale, currency_unit


def _factor_ids(inputs: dict[str, Any]) -> list[str]:
    factors = _required(inputs, "factors", "inputs")
    if not isinstance(factors, list) or not factors:
        raise ValueError("at least one KRD factor is required")
    result = [_required(factor, "factor_id", "factor") for factor in factors]
    if len(set(result)) != len(result):
        raise ValueError("KRD factor ids must be unique")
    return result


def _validate_result_ref(reference: dict[str, Any], path: str) -> None:
    _required(reference, "result_id", path)
    content_hash = _required(reference, "content_hash", path)
    _validate_hash(content_hash, f"{path}.content_hash")


def _bond_lookup(
    inputs: dict[str, Any], factor_ids: list[str], currency_unit: str
) -> dict[str, dict[str, Any]]:
    bonds = _required(inputs, "bonds", "inputs")
    if not isinstance(bonds, list) or not bonds:
        raise ValueError("at least one CGB bond is required")
    result: dict[str, dict[str, Any]] = {}
    for bond in bonds:
        instrument_id = _required(bond, "instrument_id", "bond")
        path = f"bonds[{instrument_id}]"
        if instrument_id in result:
            raise ValueError(f"duplicate bond {instrument_id}")
        if _required(bond, "instrument_type", path) != "CGB":
            raise ValueError("only CGB instruments are admitted by this fixture")
        _required(bond, "version", path)
        bond_hash = _required(bond, "content_hash", path)
        _validate_hash(bond_hash, f"{path}.content_hash")
        if _required(bond, "currency_unit", path) != currency_unit:
            raise ValueError(f"currency unit drift for {instrument_id}")
        for key in (
            "notional_per_quantity",
            "market_value_per_quantity",
            "economic_pnl_per_quantity",
            "ytm",
            "modified_duration",
            "convexity",
            "coupon_rate",
            "remaining_years",
            "dv01_per_quantity",
        ):
            _decimal(_required(bond, key, path), f"{path}.{key}")
        if _decimal(bond["notional_per_quantity"], f"{path}.notional_per_quantity") <= ZERO:
            raise ValueError(f"{path}.notional_per_quantity must be positive")
        if _decimal(bond["market_value_per_quantity"], f"{path}.market_value_per_quantity") <= ZERO:
            raise ValueError(f"{path}.market_value_per_quantity must be positive")

        krd = _required(bond, "krd_per_quantity", path)
        if set(krd) != set(factor_ids):
            raise ValueError(f"{path}.krd_per_quantity must bind every exact factor")
        node_sum = sum(
            (
                _decimal(krd[factor_id], f"{path}.krd_per_quantity.{factor_id}")
                for factor_id in factor_ids
            ),
            ZERO,
        )
        parallel = _decimal(bond["dv01_per_quantity"], f"{path}.dv01_per_quantity")
        if node_sum != parallel:
            raise ValueError(f"{path} KRD nodes do not sum to parallel DV01")
        _validate_result_ref(
            _required(bond, "analyze_bond_result_ref", path),
            f"{path}.analyze_bond_result_ref",
        )
        _validate_result_ref(
            _required(bond, "portfolio_risk_result_ref", path),
            f"{path}.portfolio_risk_result_ref",
        )
        result[instrument_id] = bond
    return result


def _validate_entity_binding(entity: dict[str, Any], entity_kind: str) -> str:
    identity_key = f"{entity_kind}_id"
    entity_id = _required(entity, identity_key, entity_kind)
    reference_key = f"{entity_kind}_ref"
    reference = _required(entity, reference_key, entity_kind)
    _required(reference, "version", f"{entity_kind}.{reference_key}")
    content_hash = _required(reference, "content_hash", f"{entity_kind}.{reference_key}")
    _validate_hash(content_hash, f"{entity_kind}.{reference_key}.content_hash")
    snapshot = _required(entity, "position_snapshot_ref", entity_kind)
    _validate_ref(
        snapshot,
        f"{entity_kind}.position_snapshot_ref",
        "snapshot_id",
        require_times=True,
    )
    return entity_id


def _position_records(
    entities: list[dict[str, Any]],
    entity_kind: str,
    bonds: dict[str, dict[str, Any]],
) -> list[tuple[dict[str, Any], dict[str, Any], Decimal]]:
    records: list[tuple[dict[str, Any], dict[str, Any], Decimal]] = []
    position_ids: set[str] = set()
    for entity in entities:
        _validate_entity_binding(entity, entity_kind)
        positions = _required(entity, "positions", entity_kind)
        if not isinstance(positions, list) or not positions:
            raise ValueError(f"{entity_kind} must bind at least one position")
        for position in positions:
            position_id = _required(position, "position_id", "position")
            if position_id in position_ids:
                raise ValueError(f"duplicate position {position_id}")
            position_ids.add(position_id)
            instrument_id = _required(position, "instrument_id", f"positions[{position_id}]")
            if instrument_id not in bonds:
                raise ValueError(f"position {position_id} has no exact CGB authority")
            bond = bonds[instrument_id]
            instrument_ref = _required(
                position, "instrument_ref", f"positions[{position_id}]"
            )
            version = _required(
                instrument_ref, "version", f"positions[{position_id}].instrument_ref"
            )
            content_hash = _required(
                instrument_ref,
                "content_hash",
                f"positions[{position_id}].instrument_ref",
            )
            _validate_hash(
                content_hash, f"positions[{position_id}].instrument_ref.content_hash"
            )
            if version != bond["version"] or content_hash != bond["content_hash"]:
                raise ValueError(f"instrument authority drift for position {position_id}")
            quantity = _decimal(
                _required(position, "quantity", f"positions[{position_id}]"),
                f"positions[{position_id}].quantity",
            )
            records.append((position, bond, quantity))
    if not records:
        raise ValueError("an aggregate requires at least one position")
    return records


def _aggregate(
    aggregate_id: str,
    portfolio_ids: list[str],
    entities: list[dict[str, Any]],
    entity_kind: str,
    bonds: dict[str, dict[str, Any]],
    factor_ids: list[str],
    scale: int,
) -> dict[str, Any]:
    records = _position_records(entities, entity_kind, bonds)
    market_value = ZERO
    economic_pnl = ZERO
    parallel_dv01 = ZERO
    factor_totals = {factor_id: ZERO for factor_id in factor_ids}
    for _, bond, quantity in records:
        market_value += quantity * _decimal(
            bond["market_value_per_quantity"], "bond.market_value_per_quantity"
        )
        economic_pnl += quantity * _decimal(
            bond["economic_pnl_per_quantity"], "bond.economic_pnl_per_quantity"
        )
        parallel_dv01 += quantity * _decimal(
            bond["dv01_per_quantity"], "bond.dv01_per_quantity"
        )
        for factor_id in factor_ids:
            factor_totals[factor_id] += quantity * _decimal(
                bond["krd_per_quantity"][factor_id],
                f"bond.krd_per_quantity.{factor_id}",
            )
    if sum(factor_totals.values(), ZERO) != parallel_dv01:
        raise ValueError("aggregate KRD nodes do not sum to parallel DV01")

    basic_metrics = {
        "market_value": render_decimal(market_value, scale),
        "economic_pnl": render_decimal(economic_pnl, scale),
    }
    non_positive = [record for record in records if record[2] <= ZERO]
    missing_reasons: list[str] = []
    if non_positive:
        missing_reasons.append(
            "short_or_non_positive_position_excluded_from_weighted_averages"
        )
    else:
        market_value_denominator = market_value
        if market_value_denominator == ZERO:
            raise ValueError("market value weighting denominator is zero")
        ytm_denominator = sum(
            (
                quantity
                * _decimal(
                    bond["market_value_per_quantity"],
                    "bond.market_value_per_quantity",
                )
                * _decimal(bond["modified_duration"], "bond.modified_duration")
                for _, bond, quantity in records
            ),
            ZERO,
        )
        if ytm_denominator == ZERO:
            raise ValueError("ytm weighting denominator is zero")
        notional_denominator = sum(
            (
                quantity
                * _decimal(
                    bond["notional_per_quantity"], "bond.notional_per_quantity"
                )
                for _, bond, quantity in records
            ),
            ZERO,
        )
        if notional_denominator == ZERO:
            raise ValueError("notional weighting denominator is zero")

        with localcontext(CONTEXT):
            weighted_ytm = sum(
                (
                    quantity
                    * _decimal(
                        bond["market_value_per_quantity"],
                        "bond.market_value_per_quantity",
                    )
                    * _decimal(bond["modified_duration"], "bond.modified_duration")
                    * _decimal(bond["ytm"], "bond.ytm")
                    for _, bond, quantity in records
                ),
                ZERO,
            ) / ytm_denominator
            modified_duration = ytm_denominator / market_value_denominator
            convexity = sum(
                (
                    quantity
                    * _decimal(
                        bond["market_value_per_quantity"],
                        "bond.market_value_per_quantity",
                    )
                    * _decimal(bond["convexity"], "bond.convexity")
                    for _, bond, quantity in records
                ),
                ZERO,
            ) / market_value_denominator
            weighted_coupon_rate = sum(
                (
                    quantity
                    * _decimal(
                        bond["notional_per_quantity"], "bond.notional_per_quantity"
                    )
                    * _decimal(bond["coupon_rate"], "bond.coupon_rate")
                    for _, bond, quantity in records
                ),
                ZERO,
            ) / notional_denominator
            weighted_remaining_years = sum(
                (
                    quantity
                    * _decimal(
                        bond["notional_per_quantity"], "bond.notional_per_quantity"
                    )
                    * _decimal(bond["remaining_years"], "bond.remaining_years")
                    for _, bond, quantity in records
                ),
                ZERO,
            ) / notional_denominator
        basic_metrics.update(
            {
                "weighted_ytm": render_decimal(weighted_ytm, scale),
                "modified_duration": render_decimal(modified_duration, scale),
                "convexity": render_decimal(convexity, scale),
                "weighted_coupon_rate": render_decimal(
                    weighted_coupon_rate, scale
                ),
                "weighted_remaining_years": render_decimal(
                    weighted_remaining_years, scale
                ),
            }
        )
    basic_metrics["dv01"] = render_decimal(parallel_dv01, scale)

    return {
        "aggregate_id": aggregate_id,
        "portfolio_ids": portfolio_ids,
        "position_count": len(records),
        "data_mode": "PARTIAL" if missing_reasons else "REAL",
        "coverage": {
            "weighted_average_participating_position_count": len(records)
            - len(non_positive),
            "missing_reasons": missing_reasons,
        },
        "basic_metrics": basic_metrics,
        "krd_summary": {
            "factor_totals": [
                {
                    "factor_id": factor_id,
                    "dv01": render_decimal(factor_totals[factor_id], scale),
                }
                for factor_id in factor_ids
            ],
            "parallel_dv01": render_decimal(parallel_dv01, scale),
        },
    }


def _validated_inputs(
    inputs: dict[str, Any],
) -> tuple[int, list[str], dict[str, dict[str, Any]]]:
    scale, currency_unit = _validate_authority(inputs)
    factor_ids = _factor_ids(inputs)
    bonds = _bond_lookup(inputs, factor_ids, currency_unit)
    return scale, factor_ids, bonds


def aggregate_scope(
    inputs: dict[str, Any], portfolio_ids: list[str] | None = None
) -> dict[str, Any]:
    """Aggregate the exact selected Portfolio members under the frozen R8A convention."""

    scale, factor_ids, bonds = _validated_inputs(inputs)
    portfolios = _required(inputs, "portfolios", "inputs")
    if not isinstance(portfolios, list) or not portfolios:
        raise ValueError("at least one Portfolio is required")
    by_id: dict[str, dict[str, Any]] = {}
    for portfolio in portfolios:
        portfolio_id = _required(portfolio, "portfolio_id", "portfolio")
        if portfolio_id in by_id:
            raise ValueError(f"duplicate Portfolio {portfolio_id}")
        by_id[portfolio_id] = portfolio
    selected_ids = list(by_id) if portfolio_ids is None else list(portfolio_ids)
    if not selected_ids:
        raise ValueError("Portfolio scope must not be empty")
    if len(set(selected_ids)) != len(selected_ids):
        raise ValueError("Portfolio scope ids must be unique")
    for portfolio_id in selected_ids:
        if portfolio_id not in by_id:
            raise ValueError(f"unknown Portfolio {portfolio_id}")
    selected = [by_id[portfolio_id] for portfolio_id in selected_ids]
    return _aggregate(
        "scope:" + ",".join(selected_ids),
        selected_ids,
        selected,
        "portfolio",
        bonds,
        factor_ids,
        scale,
    )


def _clone_value(value: Any) -> Any:
    if isinstance(value, dict):
        return {key: _clone_value(item) for key, item in value.items()}
    if isinstance(value, list):
        return [_clone_value(item) for item in value]
    return value


def scaled_inputs(inputs: dict[str, Any], multiplier: Decimal) -> dict[str, Any]:
    """Return an isolated quantity-scaled fixture for exact metamorphic witnesses."""

    if not multiplier.is_finite() or multiplier == ZERO:
        raise ValueError("quantity multiplier must be a finite non-zero Decimal")
    cloned = _clone_value(inputs)
    for portfolio in cloned["portfolios"]:
        for position in portfolio["positions"]:
            position["quantity"] = _plain_decimal(
                _decimal(position["quantity"], "position.quantity") * multiplier
            )
    for position in cloned["benchmark"]["positions"]:
        position["quantity"] = _plain_decimal(
            _decimal(position["quantity"], "benchmark.position.quantity") * multiplier
        )
    return cloned


def _benchmark_difference(
    portfolio: dict[str, Any],
    benchmark: dict[str, Any],
    factor_ids: list[str],
    scale: int,
) -> dict[str, Any]:
    portfolio_metrics = portfolio["basic_metrics"]
    benchmark_metrics = benchmark["basic_metrics"]
    metric_differences = {
        metric: render_decimal(
            _decimal(portfolio_metrics[metric], f"portfolio.{metric}")
            - _decimal(benchmark_metrics[metric], f"benchmark.{metric}"),
            scale,
        )
        for metric in BASIC_METRIC_ORDER
        if metric in portfolio_metrics and metric in benchmark_metrics
    }
    portfolio_nodes = {
        node["factor_id"]: _decimal(node["dv01"], "portfolio.krd.dv01")
        for node in portfolio["krd_summary"]["factor_totals"]
    }
    benchmark_nodes = {
        node["factor_id"]: _decimal(node["dv01"], "benchmark.krd.dv01")
        for node in benchmark["krd_summary"]["factor_totals"]
    }
    return {
        "portfolio_id": portfolio["aggregate_id"],
        "basic_metrics": metric_differences,
        "krd_factor_differences": [
            {
                "factor_id": factor_id,
                "dv01": render_decimal(
                    portfolio_nodes[factor_id] - benchmark_nodes[factor_id], scale
                ),
            }
            for factor_id in factor_ids
        ],
    }


def build_expected(inputs: dict[str, Any]) -> dict[str, Any]:
    """Build the complete frozen expected document from public R8A fixture facts."""

    scale, factor_ids, bonds = _validated_inputs(inputs)
    portfolios = _required(inputs, "portfolios", "inputs")
    portfolio_results = [
        _aggregate(
            portfolio["portfolio_id"],
            [portfolio["portfolio_id"]],
            [portfolio],
            "portfolio",
            bonds,
            factor_ids,
            scale,
        )
        for portfolio in portfolios
    ]
    scope = aggregate_scope(inputs)
    benchmark_document = _required(inputs, "benchmark", "inputs")
    benchmark = _aggregate(
        benchmark_document["benchmark_id"],
        [],
        [benchmark_document],
        "benchmark",
        bonds,
        factor_ids,
        scale,
    )
    multiplier = Decimal("3")
    scaled_scope = aggregate_scope(scaled_inputs(inputs, multiplier))
    inverse_scope = aggregate_scope(scaled_inputs(inputs, Decimal("-1")))
    authority = inputs["authority"]
    return {
        "schema_id": "ficant.portfolio360.metric-oracle-expected.v1",
        "input_schema_id": inputs["schema_id"],
        "owner": authority["owner"],
        "valuation_at": authority["valuation_at"],
        "knowledge_at": authority["knowledge_at"],
        "currency_unit": authority["currency_unit"],
        "metric_convention_ref": _clone_value(authority["metric_convention_ref"]),
        "rounding": "TIES_TO_EVEN",
        "output_scale": scale,
        "factors": factor_ids,
        "portfolios": portfolio_results,
        "scope": scope,
        "benchmark": benchmark,
        "benchmark_differences": [
            _benchmark_difference(portfolio, benchmark, factor_ids, scale)
            for portfolio in portfolio_results
        ],
        "metamorphic_witness": {
            "quantity_multiplier": _plain_decimal(multiplier),
            "scaled_scope": scaled_scope,
            "inverse_scope": inverse_scope,
        },
    }
