"""Independent Decimal witness for the R5E coupon output-VAT-only analytics.

Only Python standard-library Decimal primitives are used.  The witness validates the frozen
authority envelope, classifies Bonds by first issue date, solves after-tax Bond YTM from the
same dirty price, calculates both Delivery IRRs in the approved operation order, and selects
market and subject CTD identities without importing any FICANT implementation.
"""

from __future__ import annotations

from decimal import Context, Decimal, ROUND_HALF_EVEN, localcontext
from typing import Any

CONTEXT = Context(prec=60, rounding=ROUND_HALF_EVEN)
Q12 = Decimal("0.000000000001")
EXPECTED_AUTHORITY = {
    "semantic_sha256": "54fa5adbeb8b164dc779ecc250ab622ab5747cdeb36f2b6da58f4d877ce5106a",
    "type_url": "type.googleapis.com/ficant.market.v1.TaxRulePackV2",
    "source": "ficant-authority/cgb-interest-tax/v1",
    "rate_unit": "01K2CGBVAT0000000000000000@1",
    "cutoff": "2025-08-08",
    "value_added_tax_rate": "0.06",
    "income_tax_rate": "0",
    "gross_coupon_basis": "VAT_INCLUDED",
    "rounding": "TIES_TO_EVEN",
    "claim_scope": "COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT",
}


def _d(value: str) -> Decimal:
    return Decimal(value)


def _q(value: Decimal) -> Decimal:
    return value.quantize(Q12, rounding=ROUND_HALF_EVEN)


def _render(value: Decimal) -> str:
    value = _q(value)
    if value == 0:
        value = abs(value)
    return format(value, "f")


def _validate_authority(inputs: dict[str, Any]) -> dict[str, Any]:
    authority = inputs.get("authority")
    if authority != EXPECTED_AUTHORITY:
        raise ValueError("authority envelope drift")
    return authority


def _vat_rate(authority: dict[str, Any], first_issue_date: str, vat: str, income: str) -> Decimal:
    before = first_issue_date < authority["cutoff"]
    expected_vat = "EXEMPT" if before else "TAXABLE"
    if vat != expected_vat or income != "EXEMPT":
        raise ValueError("Bond tax attributes do not match the authoritative first-issue rule")
    return Decimal(0) if before else _d(authority["value_added_tax_rate"])


def _net_coupon(gross: Decimal, vat_rate: Decimal) -> Decimal:
    if gross < 0 or vat_rate < 0:
        raise ValueError("negative coupon or rate")
    return _q(gross / (Decimal(1) + vat_rate))


def _dirty_price(case: dict[str, Any], coupon: Decimal, annual_yield: Decimal) -> Decimal:
    frequency = _d(case["periods_per_year"])
    remaining = int(case["remaining_periods"])
    first_fraction = _d(case["settlement_fraction_to_next_coupon"])
    base = Decimal(1) + annual_yield / frequency
    if base <= 0:
        raise ValueError("invalid yield base")
    face = _d(case["face_value"])
    total = Decimal(0)
    for index in range(remaining):
        periods = first_fraction + Decimal(index)
        cashflow = coupon + (face if index == remaining - 1 else Decimal(0))
        total += cashflow / (base**periods)
    return total


def _solve_yield(case: dict[str, Any], coupon: Decimal) -> Decimal:
    accrued = coupon * _d(case["accrued_period_fraction"])
    target = _d(case["clean_price"]) + accrued
    low = Decimal("-0.90")
    high = Decimal("1.00")
    low_error = _dirty_price(case, coupon, low) - target
    high_error = _dirty_price(case, coupon, high) - target
    if low_error * high_error >= 0:
        raise ValueError("Bond YTM root is not bracketed")
    for _ in range(260):
        midpoint = (low + high) / Decimal(2)
        error = _dirty_price(case, coupon, midpoint) - target
        if error == 0:
            return _q(midpoint)
        if error * low_error > 0:
            low = midpoint
            low_error = error
        else:
            high = midpoint
    return _q((low + high) / Decimal(2))


def _bond_result(authority: dict[str, Any], case: dict[str, Any]) -> dict[str, Any]:
    if case["current_issue_date"] < case["first_issue_date"]:
        raise ValueError("current issue precedes first issue")
    vat_rate = _vat_rate(
        authority,
        case["first_issue_date"],
        case["value_added_tax_status"],
        case["income_tax_status"],
    )
    gross = _d(case["gross_coupon_per_period"])
    adjusted = _net_coupon(gross, vat_rate)
    return {
        "case_id": case["case_id"],
        "value_added_tax_rate": _render(vat_rate),
        "gross_coupon_per_period": _render(gross),
        "tax_adjusted_coupon_per_period": _render(adjusted),
        "market_pre_tax_yield_to_maturity": _render(_solve_yield(case, gross)),
        "subject_tax_adjusted_yield_to_maturity": _render(_solve_yield(case, adjusted)),
        "claim_scope": authority["claim_scope"],
    }


def _delivery_irr(invoice: Decimal, coupon: Decimal, dirty: Decimal, days: int) -> Decimal:
    if dirty <= 0 or days <= 0:
        raise ValueError("invalid Delivery denominator")
    ratio = _q((invoice + coupon) / dirty)
    holding_return = ratio - Decimal(1)
    annual_numerator = holding_return * Decimal(365)
    return _q(annual_numerator / Decimal(days))


def _candidate_result(authority: dict[str, Any], candidate: dict[str, Any]) -> dict[str, Any]:
    vat_rate = _vat_rate(
        authority,
        candidate["first_issue_date"],
        candidate["value_added_tax_status"],
        candidate["income_tax_status"],
    )
    gross = _d(candidate["gross_interim_coupons"])
    adjusted = _net_coupon(gross, vat_rate)
    invoice = _d(candidate["invoice_price"])
    dirty = _d(candidate["purchase_dirty_price"])
    days = int(candidate["actual_days"])
    return {
        "bond_id": candidate["bond_id"],
        "gross_interim_coupons": _render(gross),
        "tax_adjusted_interim_coupons": _render(adjusted),
        "market_pre_tax_irr": _render(_delivery_irr(invoice, gross, dirty, days)),
        "subject_tax_adjusted_irr": _render(_delivery_irr(invoice, adjusted, dirty, days)),
        "market_net_basis": _render(_d(candidate["market_net_basis"])),
        "claim_scope": authority["claim_scope"],
    }


def _ctd(candidates: list[dict[str, Any]], rate_field: str) -> str:
    if not candidates:
        raise ValueError("empty Delivery basket")
    ordered = sorted(
        candidates,
        key=lambda candidate: (
            -_d(candidate[rate_field]),
            _d(candidate["market_net_basis"]),
            candidate["bond_id"],
        ),
    )
    return ordered[0]["bond_id"]


def _basket_result(authority: dict[str, Any], basket: dict[str, Any]) -> dict[str, Any]:
    candidates = [_candidate_result(authority, value) for value in basket["candidates"]]
    return {
        "basket_id": basket["basket_id"],
        "candidates": candidates,
        "market_ctd_bond_id": _ctd(candidates, "market_pre_tax_irr"),
        "subject_ctd_bond_id": _ctd(candidates, "subject_tax_adjusted_irr"),
    }


def build_expected(inputs: dict[str, Any]) -> dict[str, Any]:
    """Build the frozen expected document from exact independent Decimal calculations."""

    with localcontext(CONTEXT):
        if inputs.get("schema") != "ficant.r5e-tax-adjusted-analytics.inputs.v1":
            raise ValueError("input schema drift")
        authority = _validate_authority(inputs)
        bonds = [_bond_result(authority, case) for case in inputs["bond_cases"]]
        baskets = [_basket_result(authority, basket) for basket in inputs["delivery_baskets"]]
        return {
            "schema": "ficant.r5e-tax-adjusted-analytics.expected.v1",
            "authority_semantic_sha256": authority["semantic_sha256"],
            "bond_cases": bonds,
            "delivery_baskets": baskets,
        }
