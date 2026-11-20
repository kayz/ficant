from __future__ import annotations

import ast
from collections import Counter
from decimal import Decimal, localcontext
import hashlib
import json
from pathlib import Path

import pytest

from bp1_futures_hedge_v2_decimal_oracle import (
    CONTEXT,
    EXPECTED_DECIMAL_UNIT_ID,
    EXPECTED_DECIMAL_UNIT_VERSION,
    EXPECTED_RULE_PACK_BIN_REL,
    EXPECTED_RULE_PACK_BIN_SHA256,
    EXPECTED_RULE_PACK_JSON_LF_SHA256,
    EXPECTED_RULE_PACK_JSON_REL,
    build_expected,
    calculate_case,
    calculate_exact,
    load_verified_rule_pack,
)

ROOT = Path(__file__).resolve().parents[3]
INPUTS = ROOT / "tests/golden-cases/china-rates/bp1-futures-hedge-v2-inputs.json"
EXPECTED = (
    ROOT / "tests/golden-cases/china-rates/expected/bp1-futures-hedge-v2-expected.json"
)
PHASE2D_INPUTS = ROOT / "tests/golden-cases/china-rates/phase2d-futures-hedge-inputs.json"
PHASE2D_EXPECTED = (
    ROOT / "tests/golden-cases/china-rates/expected/phase2d-futures-hedge-v1-expected.json"
)
ORACLE = Path(__file__).with_name("bp1_futures_hedge_v2_decimal_oracle.py")


def _documents() -> tuple[dict, dict]:
    return (
        json.loads(INPUTS.read_text(encoding="utf-8")),
        json.loads(EXPECTED.read_text(encoding="utf-8")),
    )


def test_oracle_imports_only_standard_library_whitelist() -> None:
    tree = ast.parse(ORACLE.read_text(encoding="utf-8"))
    imported_roots = {
        alias.name.split(".", maxsplit=1)[0]
        for node in ast.walk(tree)
        if isinstance(node, ast.Import)
        for alias in node.names
    }
    imported_roots.update(
        node.module.split(".", maxsplit=1)[0]
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom) and node.module is not None
    )
    assert imported_roots == {
        "__future__",
        "decimal",
        "hashlib",
        "json",
        "pathlib",
        "typing",
    }


def test_rule_pack_bin_and_lf_json_digests_and_derived_notionals(
    tmp_path: Path,
) -> None:
    bin_bytes = (ROOT / EXPECTED_RULE_PACK_BIN_REL).read_bytes()
    json_lf_bytes = (ROOT / EXPECTED_RULE_PACK_JSON_REL).read_bytes().replace(
        b"\r\n", b"\n"
    )
    assert hashlib.sha256(bin_bytes).hexdigest() == EXPECTED_RULE_PACK_BIN_SHA256
    assert hashlib.sha256(json_lf_bytes).hexdigest() == EXPECTED_RULE_PACK_JSON_LF_SHA256

    pack = load_verified_rule_pack(ROOT)
    assert pack["bin_sha256"] == EXPECTED_RULE_PACK_BIN_SHA256
    assert pack["json_lf_sha256"] == EXPECTED_RULE_PACK_JSON_LF_SHA256
    assert pack["decimal_unit_id"] == EXPECTED_DECIMAL_UNIT_ID
    assert pack["decimal_unit_version"] == EXPECTED_DECIMAL_UNIT_VERSION
    assert pack["face_quote_basis"] == Decimal("100")
    assert pack["product_contract_sizes"] == {
        "T": 10_000,
        "TF": 10_000,
        "TL": 10_000,
        "TS": 20_000,
    }
    assert pack["product_contract_notionals"] == {
        "T": Decimal("1000000"),
        "TF": Decimal("1000000"),
        "TL": Decimal("1000000"),
        "TS": Decimal("2000000"),
    }

    # Verify fail-closed behavior if either .bin or .json digest drifts.
    pack_dir = tmp_path / "domain-packs/cgb-futures"
    pack_dir.mkdir(parents=True)
    (pack_dir / "cgb-futures-v2.bin").write_bytes(bin_bytes + b"\x00")
    (pack_dir / "cgb-futures-v2.json").write_bytes(json_lf_bytes)
    with pytest.raises(ValueError, match="cgb-futures-v2.bin SHA-256 mismatch"):
        load_verified_rule_pack(tmp_path)

    (pack_dir / "cgb-futures-v2.bin").write_bytes(bin_bytes)
    (pack_dir / "cgb-futures-v2.json").write_bytes(json_lf_bytes + b"\n")
    with pytest.raises(ValueError, match="cgb-futures-v2.json LF SHA-256 mismatch"):
        load_verified_rule_pack(tmp_path)


def test_independent_decimal_oracle_matches_frozen_expected_exactly() -> None:
    inputs, expected = _documents()
    computed = build_expected(inputs, repo_root=ROOT)
    assert computed == expected

    cases = inputs["cases"]
    assert len(cases) == 6
    assert Counter(case["product"] for case in cases) == {
        "TS": 3,
        "TF": 1,
        "T": 1,
        "TL": 1,
    }
    for result in computed["case_results"].values():
        for field in (
            "contract_notional",
            "face_quote_basis",
            "futures_contract_dv01",
            "raw_contracts",
            "residual_dv01",
            "hedge_effectiveness",
        ):
            assert result[field] != "-0.000000000000"


def test_ts_correction_exact_integer_half_tie_and_product_coverage() -> None:
    inputs, expected = _documents()
    phase2d_inputs = json.loads(PHASE2D_INPUTS.read_text(encoding="utf-8"))
    phase2d_expected = json.loads(PHASE2D_EXPECTED.read_text(encoding="utf-8"))

    v1_ts = next(case for case in phase2d_inputs["cases"] if case["product"] == "TS")
    v2_ts = next(
        case
        for case in inputs["cases"]
        if case["id"] == "TS-2m-long-risk-rounds-short-four"
    )
    assert v2_ts["target_dv01"] == v1_ts["target_dv01"]
    assert v2_ts["ctd_dv01_per_100"] == v1_ts["ctd_dv01_per_100"]
    assert v2_ts["conversion_factor"] == v1_ts["conversion_factor"]

    v1_ts_result = phase2d_expected["case_results"][v1_ts["id"]]
    v2_ts_result = expected["case_results"][v2_ts["id"]]
    assert v1_ts_result["recommended_contracts"] == -9
    assert v2_ts_result["recommended_contracts"] == -4
    assert v2_ts_result["contract_notional"] == "2000000.000000000000"
    assert v2_ts_result["futures_contract_dv01"] == "290.377490737959"

    exact_ts = expected["case_results"]["TS-2m-exact-long-two"]
    assert exact_ts["recommended_contracts"] == 2
    assert exact_ts["residual_dv01"] == "0.000000000000"
    assert exact_ts["hedge_effectiveness"] == "1.000000000000"

    tie_ts = expected["case_results"]["TS-2m-half-contract-tie-prefers-zero"]
    assert tie_ts["raw_contracts"] == "0.500000000000"
    assert tie_ts["recommended_contracts"] == 0
    assert tie_ts["residual_dv01"] == "-187.500000000000"
    assert tie_ts["hedge_effectiveness"] == "0.000000000000"

    results = expected["case_results"]
    assert {case["product"] for case in inputs["cases"]} == {"TS", "TF", "T", "TL"}
    assert any(Decimal(case["target_dv01"]) > 0 for case in inputs["cases"])
    assert any(Decimal(case["target_dv01"]) < 0 for case in inputs["cases"])
    assert any(res["recommended_contracts"] < 0 for res in results.values())
    assert any(res["recommended_contracts"] > 0 for res in results.values())
    assert any(res["recommended_contracts"] == 0 for res in results.values())


def test_doubling_contract_notional_doubles_dv01_and_halves_raw_contracts() -> None:
    inputs, _ = _documents()
    pack = load_verified_rule_pack(ROOT)
    face_quote_basis: Decimal = pack["face_quote_basis"]
    product_notionals: dict[str, Decimal] = pack["product_contract_notionals"]

    for case in inputs["cases"]:
        notional = product_notionals[case["product"]]
        doubled_notional = notional * Decimal(2)

        base_exact = calculate_exact(
            contract_notional=notional,
            face_quote_basis=face_quote_basis,
            target_dv01=Decimal(case["target_dv01"]),
            ctd_dv01_per_100=Decimal(case["ctd_dv01_per_100"]),
            conversion_factor=Decimal(case["conversion_factor"]),
        )
        doubled_exact = calculate_exact(
            contract_notional=doubled_notional,
            face_quote_basis=face_quote_basis,
            target_dv01=Decimal(case["target_dv01"]),
            ctd_dv01_per_100=Decimal(case["ctd_dv01_per_100"]),
            conversion_factor=Decimal(case["conversion_factor"]),
        )
        base_dv01 = base_exact["futures_contract_dv01"]
        doubled_dv01 = doubled_exact["futures_contract_dv01"]
        base_raw = base_exact["raw_contracts"]
        doubled_raw = doubled_exact["raw_contracts"]
        assert isinstance(base_dv01, Decimal) and isinstance(doubled_dv01, Decimal)
        assert isinstance(base_raw, Decimal) and isinstance(doubled_raw, Decimal)

        # High-precision metamorphic identity: 2x notional -> 2x contract DV01, 0.5x raw contracts.
        with localcontext(CONTEXT):
            assert abs(doubled_dv01 - Decimal(2) * base_dv01) <= (
                Decimal("1e-55") * abs(doubled_dv01)
            )
            assert abs(Decimal(2) * doubled_raw - base_raw) <= (
                Decimal("1e-55") * abs(base_raw)
            )

        # Quantized 12-decimal output identity within quantization rounding bound.
        base_q = calculate_case(
            case, contract_notional=notional, face_quote_basis=face_quote_basis
        )
        doubled_q = calculate_case(
            case, contract_notional=doubled_notional, face_quote_basis=face_quote_basis
        )
        assert abs(
            Decimal(doubled_q["futures_contract_dv01"])
            - Decimal(2) * Decimal(base_q["futures_contract_dv01"])
        ) <= Decimal("0.000000000002")
        assert abs(
            Decimal(2) * Decimal(doubled_q["raw_contracts"])
            - Decimal(base_q["raw_contracts"])
        ) <= Decimal("0.000000000002")

        # Scale invariance at fixed notional: doubling face_quote_basis & ctd_dv01_per_quote_unit.
        scaled_basis_exact = calculate_exact(
            contract_notional=notional,
            face_quote_basis=face_quote_basis * Decimal(2),
            target_dv01=Decimal(case["target_dv01"]),
            ctd_dv01_per_100=Decimal(case["ctd_dv01_per_100"]) * Decimal(2),
            conversion_factor=Decimal(case["conversion_factor"]),
        )
        assert scaled_basis_exact == base_exact


def test_tie_break_exact_integer_and_direction_rules() -> None:
    notional = Decimal("2000000")
    basis = Decimal("100")
    ctd_dv01 = Decimal("0.018")
    cf = Decimal("0.96")  # futures_contract_dv01 == 375 exactly

    # Half-contract ties prefer smaller |n|.
    for target, expected_raw, expected_contracts in [
        (Decimal("-187.5"), Decimal("0.5"), 0),
        (Decimal("187.5"), Decimal("-0.5"), 0),
        (Decimal("-562.5"), Decimal("1.5"), 1),
        (Decimal("562.5"), Decimal("-1.5"), -1),
    ]:
        res = calculate_exact(
            contract_notional=notional,
            face_quote_basis=basis,
            target_dv01=target,
            ctd_dv01_per_100=ctd_dv01,
            conversion_factor=cf,
        )
        assert res["raw_contracts"] == expected_raw
        assert res["recommended_contracts"] == expected_contracts

    # Exact integer hedges on both directions yield zero residual and effectiveness == 1.
    for target, expected_contracts in [
        (Decimal("1125"), -3),
        (Decimal("-1125"), 3),
    ]:
        res = calculate_exact(
            contract_notional=notional,
            face_quote_basis=basis,
            target_dv01=target,
            ctd_dv01_per_100=ctd_dv01,
            conversion_factor=cf,
        )
        assert res["recommended_contracts"] == expected_contracts
        assert res["residual_dv01"] == Decimal(0)
        assert res["hedge_effectiveness"] == Decimal(1)
        if target > 0:
            assert Decimal(res["raw_contracts"]) < 0
            assert int(res["recommended_contracts"]) <= 0
        else:
            assert Decimal(res["raw_contracts"]) > 0
            assert int(res["recommended_contracts"]) >= 0

    with pytest.raises(ValueError, match="target_dv01 must be non-zero"):
        calculate_exact(
            contract_notional=notional,
            face_quote_basis=basis,
            target_dv01=Decimal("0"),
            ctd_dv01_per_100=ctd_dv01,
            conversion_factor=cf,
        )
