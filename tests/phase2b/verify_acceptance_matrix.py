"""Fail-closed integrity and independent-oracle verifier for Phase 2B."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from decimal import Decimal
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MATRIX = Path(__file__).with_name("acceptance-matrix.json")
INPUTS = ROOT / "tests/golden-cases/china-rates/phase2b-curve-carry-inputs.json"
EXPECTED = ROOT / "tests/golden-cases/china-rates/expected/phase2b-curve-carry-v1-expected.json"
QUANTLIB_OUTPUT = (
    ROOT
    / "tests/golden-cases/china-rates/expected/phase2b-quantlib-1.42.1-output.json"
)
MANUAL_SOURCE = ROOT / "tests/oracle/china-rates/phase2b_manual_oracle.py"
QUANTLIB_SOURCE = ROOT / "tests/oracle/china-rates/phase2b_quantlib_oracle.py"

EXPECTED_IDS = {f"P2B-{index:03d}" for index in range(1, 17)}
EXPECTED_GUARDED = {
    "tests/golden-cases/china-rates/phase2b-curve-carry-inputs.json",
    "tests/golden-cases/china-rates/expected/phase2b-curve-carry-v1-expected.json",
    "tests/golden-cases/china-rates/expected/phase2b-quantlib-1.42.1-output.json",
    "tests/oracle/china-rates/phase2b_manual_oracle.py",
    "tests/oracle/china-rates/phase2b_quantlib_oracle.py",
    "crates/ficant-fixed-income-native/tests/phase2b_reference_acceptance.rs",
    "crates/ficant-storage/tests/carry_roll_arrow.rs",
    "crates/ficant-storage/tests/carry_roll_sit.rs",
    "cpp/fixed-income-kernel/tests/test_carry_roll.cpp",
    "cpp/fixed-income-kernel/tests/test_i3_regression.cpp",
}


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_manual_oracle():
    spec = importlib.util.spec_from_file_location("phase2b_matrix_manual_oracle", MANUAL_SOURCE)
    module = importlib.util.module_from_spec(spec)
    sys.modules.setdefault(spec.name, module)
    spec.loader.exec_module(module)
    return module


def verify_hashes(matrix):
    guarded = matrix.get("guarded_files")
    assert isinstance(guarded, dict), "guarded_files must be an object"
    assert set(guarded) == EXPECTED_GUARDED, "guarded file set drifted"
    for relative, expected_hash in guarded.items():
        path = ROOT / relative
        assert path.is_file(), f"missing guarded file: {relative}"
        actual = sha256(path)
        assert actual == expected_hash, f"guarded hash drift: {relative}: {actual}"


def verify_automation(matrix):
    entries = matrix.get("acceptance")
    assert isinstance(entries, list), "acceptance must be an array"
    ids = [entry.get("id") for entry in entries]
    assert len(ids) == len(set(ids)), "acceptance IDs must be unique"
    assert set(ids) == EXPECTED_IDS, "acceptance ID coverage drifted"
    for entry in entries:
        automation = entry.get("automation")
        assert isinstance(automation, list) and automation, f"{entry['id']} lacks automation"
        for route in automation:
            source = route.get("source")
            command = route.get("command")
            assert isinstance(source, str) and (ROOT / source).is_file(), (
                f"{entry['id']} automation source is missing: {source}"
            )
            assert isinstance(command, str) and command.strip(), (
                f"{entry['id']} automation command is blank"
            )
            selector = route.get("selector")
            if selector is not None:
                assert selector in (ROOT / source).read_text(encoding="utf-8"), (
                    f"{entry['id']} selector is absent from {source}: {selector}"
                )


def assert_close(actual, expected, tolerance, label):
    difference = abs(Decimal(str(actual)) - Decimal(str(expected)))
    assert difference <= tolerance, (
        f"QuantLib disagreement for {label}: actual={actual}, expected={expected}, "
        f"difference={difference}, tolerance={tolerance}"
    )


def verify_oracles():
    inputs = load_json(INPUTS)
    expected = load_json(EXPECTED)
    manual = load_manual_oracle()
    assert manual.build_expected(inputs) == expected, "manual Decimal oracle output drifted"
    provenance = expected.get("provenance", {})
    assert provenance == {
        "expected_source": "independent_decimal_python_oracle",
        "production_rust_or_cpp_used": False,
        "manual_formula_layer": "executed",
        "quantlib_required_version": "1.42.1",
        "quantlib_agreement": "verified_official_1_42_1",
    }, "frozen Oracle provenance is incomplete"

    quantlib = load_json(QUANTLIB_OUTPUT)
    assert quantlib.get("schema") == "ficant.test-oracle.phase2b-quantlib-output.v1"
    assert quantlib.get("quantlib_version") == "1.42.1"
    assert quantlib.get("convention") == expected.get("convention")
    assert set(quantlib.get("curve_results", {})) == set(expected["curve_results"])
    assert set(quantlib.get("carry_results", {})) == set(expected["carry_results"])

    yield_tolerance = Decimal(expected["tolerances"]["yield_abs"])
    value_tolerance = Decimal(expected["tolerances"]["price_and_return_abs"])
    for case_id, reference in expected["curve_results"].items():
        actual = quantlib["curve_results"][case_id]
        assert actual["query_date"] == reference["query_date"], f"{case_id} query date drift"
        assert_close(
            actual["yield_to_maturity"],
            reference["yield_to_maturity"],
            yield_tolerance,
            f"{case_id}.yield_to_maturity",
        )
    yield_fields = {"initial_yield", "rolled_yield"}
    date_fields = {"initial_curve_query_date", "rolled_curve_query_date"}
    for case_id, reference in expected["carry_results"].items():
        actual = quantlib["carry_results"][case_id]
        assert set(actual) == set(reference), f"{case_id} QuantLib field set drift"
        for field, expected_value in reference.items():
            if field in date_fields:
                assert actual[field] == expected_value, f"{case_id}.{field} drift"
            else:
                assert_close(
                    actual[field],
                    expected_value,
                    yield_tolerance if field in yield_fields else value_tolerance,
                    f"{case_id}.{field}",
                )

    quantlib_source = QUANTLIB_SOURCE.read_text(encoding="utf-8")
    assert "import QuantLib as ql" in quantlib_source
    assert '"quantlib_version": ql.__version__' in quantlib_source
    assert "ficant_kernel" not in quantlib_source
    assert "ficant_fixed_income" not in quantlib_source
    manual_source = MANUAL_SOURCE.read_text(encoding="utf-8")
    assert "ficant_kernel" not in manual_source
    assert "ficant_fixed_income" not in manual_source


def main():
    matrix = load_json(MATRIX)
    assert matrix.get("schema") == "ficant.quality.phase2b.acceptance-matrix.v1"
    assert matrix.get("base_commit") == "5b2be2453937b82091b34e256bc4fb69aa9e7415"
    verify_hashes(matrix)
    verify_automation(matrix)
    verify_oracles()
    print("Phase 2B acceptance matrix: PASS (16/16, manual + QuantLib 1.42.1)")


if __name__ == "__main__":
    main()
