"""Independent standard-library Decimal oracle for BP-1 CGB futures hedge v2.

This module uses only Python standard-library primitives (`decimal`, `hashlib`,
`json`, `pathlib`, `typing`). It binds both the authoritative binary and
LF-normalized JSON digests of `domain-packs/cgb-futures/cgb-futures-v2.*`,
derives per-product contract notionals from the rule pack, and computes
60-digit exact Decimal hedge measures quantized with ROUND_HALF_EVEN to 12
fractional digits.
"""

from __future__ import annotations

from decimal import (
    Context,
    Decimal,
    ROUND_CEILING,
    ROUND_FLOOR,
    ROUND_HALF_EVEN,
    localcontext,
)
import hashlib
import json
from pathlib import Path
from typing import Any

CONTEXT = Context(prec=60, rounding=ROUND_HALF_EVEN)
OUTPUT_QUANTUM = Decimal("0.000000000001")
TIE_TOLERANCE_SCALE = Decimal("1e-12")
ZERO = Decimal(0)
ONE = Decimal(1)

EXPECTED_RULE_PACK_BIN_REL = "domain-packs/cgb-futures/cgb-futures-v2.bin"
EXPECTED_RULE_PACK_JSON_REL = "domain-packs/cgb-futures/cgb-futures-v2.json"
EXPECTED_RULE_PACK_BIN_SHA256 = (
    "8d8a0b944557cf87ed3d1f598ab3e02fa7f0aec125c1c26c17280acbd24367c5"
)
EXPECTED_RULE_PACK_JSON_LF_SHA256 = (
    "38c7dc7f84b2cd864d5981c96afce76ecb2392dc0fd9304307b80c65b1fd50c4"
)
EXPECTED_DECIMAL_UNIT_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAP"
EXPECTED_DECIMAL_UNIT_VERSION = 1
EXPECTED_PER_100_FACE_QUOTE_BASIS = Decimal(100)


def _repo_root(repo_root: Path | None = None) -> Path:
    if repo_root is not None:
        return repo_root
    return Path(__file__).resolve().parents[3]


def _render(value: Decimal) -> str:
    quantized = value.quantize(OUTPUT_QUANTUM, rounding=ROUND_HALF_EVEN)
    if quantized == 0:
        quantized = abs(quantized)
    return format(quantized, "f")


def _parse_pack_decimal(raw: dict[str, Any], field_name: str) -> Decimal:
    unit = raw.get("unit")
    if not isinstance(unit, dict):
        raise ValueError(f"{field_name}.unit is required")
    unit_id_obj = unit.get("unitId")
    if not isinstance(unit_id_obj, dict):
        raise ValueError(f"{field_name}.unit.unitId is required")
    unit_id = unit_id_obj.get("value")
    unit_version = unit.get("version")
    if unit_id != EXPECTED_DECIMAL_UNIT_ID:
        raise ValueError(
            f"{field_name} unitId mismatch: got {unit_id!r}, "
            f"expected {EXPECTED_DECIMAL_UNIT_ID!r}"
        )
    if str(unit_version) != str(EXPECTED_DECIMAL_UNIT_VERSION):
        raise ValueError(
            f"{field_name} unit version mismatch: got {unit_version!r}, "
            f"expected {EXPECTED_DECIMAL_UNIT_VERSION!r}"
        )
    coefficient = Decimal(str(raw["coefficient"]))
    scale = int(raw["scale"])
    if scale < 0:
        raise ValueError(f"{field_name}.scale must be non-negative")
    with localcontext(CONTEXT):
        return coefficient * (Decimal(10) ** (-scale))


def load_verified_rule_pack(repo_root: Path | None = None) -> dict[str, Any]:
    """Loads, digest-verifies, and independently parses cgb-futures-v2 pack files."""

    root = _repo_root(repo_root)
    bin_path = root / EXPECTED_RULE_PACK_BIN_REL
    json_path = root / EXPECTED_RULE_PACK_JSON_REL

    bin_bytes = bin_path.read_bytes()
    bin_sha256 = hashlib.sha256(bin_bytes).hexdigest()
    if bin_sha256 != EXPECTED_RULE_PACK_BIN_SHA256:
        raise ValueError(
            f"cgb-futures-v2.bin SHA-256 mismatch: got {bin_sha256}, "
            f"expected {EXPECTED_RULE_PACK_BIN_SHA256}"
        )

    json_lf_bytes = json_path.read_bytes().replace(b"\r\n", b"\n")
    json_lf_sha256 = hashlib.sha256(json_lf_bytes).hexdigest()
    if json_lf_sha256 != EXPECTED_RULE_PACK_JSON_LF_SHA256:
        raise ValueError(
            f"cgb-futures-v2.json LF SHA-256 mismatch: got {json_lf_sha256}, "
            f"expected {EXPECTED_RULE_PACK_JSON_LF_SHA256}"
        )

    payload = json.loads(json_lf_bytes.decode("utf-8"))
    nominal_coupon = _parse_pack_decimal(payload["nominalCoupon"], "nominalCoupon")
    face_quote_basis = _parse_pack_decimal(payload["faceQuoteBasis"], "faceQuoteBasis")
    if face_quote_basis != EXPECTED_PER_100_FACE_QUOTE_BASIS:
        raise ValueError(
            f"faceQuoteBasis must equal 100 for per-100 hedge, got {face_quote_basis}"
        )

    products = payload.get("products")
    if not isinstance(products, list) or not products:
        raise ValueError("cgb-futures-v2.json must contain a non-empty products list")

    product_contract_sizes: dict[str, int] = {}
    product_contract_notionals: dict[str, Decimal] = {}
    with localcontext(CONTEXT):
        for item in products:
            code = item.get("productCode")
            if not isinstance(code, str) or not code or code in product_contract_sizes:
                raise ValueError(f"invalid or duplicate productCode: {code!r}")
            size = item.get("contractSizeInQuoteUnits")
            if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
                raise ValueError(
                    f"product {code} contractSizeInQuoteUnits must be a positive int"
                )
            product_contract_sizes[code] = size
            product_contract_notionals[code] = face_quote_basis * Decimal(size)

    return {
        "bin_path": EXPECTED_RULE_PACK_BIN_REL,
        "bin_sha256": bin_sha256,
        "json_path": EXPECTED_RULE_PACK_JSON_REL,
        "json_lf_sha256": json_lf_sha256,
        "decimal_unit_id": EXPECTED_DECIMAL_UNIT_ID,
        "decimal_unit_version": EXPECTED_DECIMAL_UNIT_VERSION,
        "nominal_coupon": nominal_coupon,
        "face_quote_basis": face_quote_basis,
        "product_contract_sizes": product_contract_sizes,
        "product_contract_notionals": product_contract_notionals,
    }


def calculate_exact(
    *,
    contract_notional: Decimal,
    face_quote_basis: Decimal,
    target_dv01: Decimal,
    ctd_dv01_per_100: Decimal,
    conversion_factor: Decimal,
) -> dict[str, Decimal | int]:
    """Computes high-precision 60-digit Decimal futures hedge measures."""

    if contract_notional <= ZERO:
        raise ValueError("contract_notional must be positive")
    if face_quote_basis <= ZERO:
        raise ValueError("face_quote_basis must be positive")
    if ctd_dv01_per_100 <= ZERO:
        raise ValueError("ctd_dv01_per_100 must be positive")
    if conversion_factor <= ZERO:
        raise ValueError("conversion_factor must be positive")
    if target_dv01 == ZERO:
        raise ValueError("target_dv01 must be non-zero")

    with localcontext(CONTEXT):
        futures_contract_dv01 = (
            ctd_dv01_per_100 * (contract_notional / face_quote_basis) / conversion_factor
        )
        raw_contracts = -target_dv01 / futures_contract_dv01
        floor_contracts = int(raw_contracts.to_integral_value(rounding=ROUND_FLOOR))
        ceil_contracts = int(raw_contracts.to_integral_value(rounding=ROUND_CEILING))

        candidates = {floor_contracts, ceil_contracts, 0}
        residuals = {
            contracts: target_dv01 + Decimal(contracts) * futures_contract_dv01
            for contracts in candidates
        }
        risks = {contracts: abs(res) for contracts, res in residuals.items()}
        min_risk = min(risks.values())
        tolerance = TIE_TOLERANCE_SCALE * max(ONE, abs(target_dv01))
        tied_candidates = [
            contracts
            for contracts, risk in risks.items()
            if risk - min_risk <= tolerance
        ]
        recommended = min(
            tied_candidates, key=lambda contracts: (abs(contracts), contracts)
        )
        residual_dv01 = residuals[recommended]
        residual_risk = risks[recommended]
        effectiveness = ONE - (residual_risk / abs(target_dv01))
        effectiveness = min(ONE, max(ZERO, effectiveness))

        return {
            "futures_contract_dv01": futures_contract_dv01,
            "raw_contracts": raw_contracts,
            "recommended_contracts": recommended,
            "residual_dv01": residual_dv01,
            "hedge_effectiveness": effectiveness,
        }


def calculate_case(
    case: dict[str, Any],
    *,
    contract_notional: Decimal,
    face_quote_basis: Decimal,
) -> dict[str, str | int]:
    """Evaluates a single golden case and renders fixed-point outputs."""

    exact = calculate_exact(
        contract_notional=contract_notional,
        face_quote_basis=face_quote_basis,
        target_dv01=Decimal(case["target_dv01"]),
        ctd_dv01_per_100=Decimal(case["ctd_dv01_per_100"]),
        conversion_factor=Decimal(case["conversion_factor"]),
    )
    return {
        "product": case["product"],
        "contract_notional": _render(contract_notional),
        "face_quote_basis": _render(face_quote_basis),
        "futures_contract_dv01": _render(exact["futures_contract_dv01"]),  # type: ignore[arg-type]
        "raw_contracts": _render(exact["raw_contracts"]),  # type: ignore[arg-type]
        "recommended_contracts": int(exact["recommended_contracts"]),
        "residual_dv01": _render(exact["residual_dv01"]),  # type: ignore[arg-type]
        "hedge_effectiveness": _render(exact["hedge_effectiveness"]),  # type: ignore[arg-type]
    }


def _verify_inputs_rule_pack_metadata(
    inputs: dict[str, Any], pack: dict[str, Any]
) -> None:
    meta = inputs.get("rule_pack")
    if not isinstance(meta, dict):
        raise ValueError("inputs.rule_pack metadata is required")
    for key in (
        "bin_path",
        "bin_sha256",
        "json_path",
        "json_lf_sha256",
        "decimal_unit_id",
        "decimal_unit_version",
    ):
        if meta.get(key) != pack[key]:
            raise ValueError(
                f"inputs.rule_pack.{key} mismatch: got {meta.get(key)!r}, "
                f"expected {pack[key]!r}"
            )
    if meta.get("face_quote_basis") != _render(pack["face_quote_basis"]):
        raise ValueError("inputs.rule_pack.face_quote_basis mismatch")

    declared_products = meta.get("products")
    if not isinstance(declared_products, dict):
        raise ValueError("inputs.rule_pack.products metadata is required")
    if set(declared_products.keys()) != set(pack["product_contract_sizes"].keys()):
        raise ValueError("inputs.rule_pack.products key set mismatch")
    for code, entry in declared_products.items():
        if entry.get("contract_size_in_quote_units") != pack["product_contract_sizes"][code]:
            raise ValueError(f"inputs.rule_pack.products[{code}] size mismatch")
        if entry.get("contract_notional") != _render(
            pack["product_contract_notionals"][code]
        ):
            raise ValueError(f"inputs.rule_pack.products[{code}] notional mismatch")


def build_expected(
    inputs: dict[str, Any], repo_root: Path | None = None
) -> dict[str, Any]:
    """Builds the frozen expected document from independent exact-Decimal math."""

    pack = load_verified_rule_pack(repo_root)
    _verify_inputs_rule_pack_metadata(inputs, pack)

    face_quote_basis: Decimal = pack["face_quote_basis"]
    product_notionals: dict[str, Decimal] = pack["product_contract_notionals"]

    case_results: dict[str, dict[str, str | int]] = {}
    for case in inputs["cases"]:
        case_id = case["id"]
        if case_id in case_results:
            raise ValueError(f"duplicate case id: {case_id}")
        product = case["product"]
        if product not in product_notionals:
            raise ValueError(f"unknown product in case {case_id}: {product}")
        case_results[case_id] = calculate_case(
            case,
            contract_notional=product_notionals[product],
            face_quote_basis=face_quote_basis,
        )

    return {
        "schema": "ficant.bp1-futures-hedge-v2.expected.v1",
        "rule_pack": {
            "bin_path": pack["bin_path"],
            "bin_sha256": pack["bin_sha256"],
            "json_path": pack["json_path"],
            "json_lf_sha256": pack["json_lf_sha256"],
            "decimal_unit_id": pack["decimal_unit_id"],
            "decimal_unit_version": pack["decimal_unit_version"],
            "face_quote_basis": _render(face_quote_basis),
            "product_contract_notionals": {
                code: _render(product_notionals[code])
                for code in sorted(product_notionals)
            },
        },
        "case_results": case_results,
    }
