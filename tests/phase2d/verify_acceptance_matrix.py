"""Fail-closed integrity and independent-oracle verifier for Phase 2D."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from decimal import Decimal
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MATRIX = Path(__file__).with_name("acceptance-matrix.json")
INPUTS = ROOT / "tests/golden-cases/china-rates/phase2d-futures-hedge-inputs.json"
EXPECTED = ROOT / "tests/golden-cases/china-rates/expected/phase2d-futures-hedge-v1-expected.json"
SOURCE_MANIFEST = ROOT / "tests/golden-cases/china-rates/phase2d-cffex-source-manifest.json"
ORACLE_SOURCE = ROOT / "tests/oracle/china-rates/phase2d_manual_oracle.py"

EXPECTED_IDS = {f"P2D-{index:03d}" for index in range(1, 19)}
EXPECTED_GUARDED = {
    "tests/golden-cases/china-rates/phase2d-futures-hedge-inputs.json",
    "tests/golden-cases/china-rates/expected/phase2d-futures-hedge-v1-expected.json",
    "tests/golden-cases/china-rates/phase2d-cffex-source-manifest.json",
    "tests/oracle/china-rates/phase2d_manual_oracle.py",
    "tests/oracle/china-rates/test_phase2d_manual_oracle.py",
    "crates/ficant-domain/tests/futures_hedge_contracts.rs",
    "crates/ficant-fixed-income-native/tests/futures_hedge_acceptance.rs",
    "crates/ficant-storage/tests/futures_hedge_arrow.rs",
    "crates/ficant-storage/tests/futures_hedge_sit.rs",
    "cpp/fixed-income-kernel/tests/test_futures_hedge.cpp",
    "cpp/fixed-income-kernel/tests/test_constants_and_layout.cpp",
}


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_oracle():
    spec = importlib.util.spec_from_file_location("phase2d_matrix_oracle", ORACLE_SOURCE)
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


def verify_oracle_and_sources():
    inputs = load_json(INPUTS)
    expected = load_json(EXPECTED)
    oracle = load_oracle()
    assert oracle.build_expected(inputs) == expected, "independent Decimal oracle drifted"
    assert expected.get("schema") == "ficant.phase2d-futures-hedge.expected.v1"
    assert {case["product"] for case in inputs["cases"]} == {"TS", "TF", "T", "TL"}
    results = expected["case_results"]
    contracts = [result["recommended_contracts"] for result in results.values()]
    assert any(value < 0 for value in contracts)
    assert any(value > 0 for value in contracts)
    assert any(value == 0 for value in contracts)
    effectiveness = [Decimal(result["hedge_effectiveness"]) for result in results.values()]
    assert Decimal(0) in effectiveness and Decimal(1) in effectiveness

    source = ORACLE_SOURCE.read_text(encoding="utf-8")
    assert "Context(prec=50" in source and "ROUND_HALF_UP" in source
    assert "ficant_kernel" not in source and "ficant_fixed_income" not in source

    manifest = load_json(SOURCE_MANIFEST)
    assert manifest.get("schema") == "ficant.phase2d.cffex-source-manifest.v1"
    canonical = json.dumps(
        manifest["normalized_facts"],
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    assert hashlib.sha256(canonical).hexdigest() == manifest["normalized_facts_sha256"]
    assert manifest["normalized_facts_sha256"] == (
        "b611332b8b9137ac9c1a24b01effccbfc4a796e9f4273fdfe23e168544f31052"
    )
    assert all(item["url"].startswith("https://www.cffex.com.cn/") for item in manifest["sources"])


def main():
    matrix = load_json(MATRIX)
    assert matrix.get("schema") == "ficant.quality.phase2d.acceptance-matrix.v1"
    assert matrix.get("base_commit") == "b7872f7e89bde71c9bbfa1fd39735125e2dba9a1"
    verify_hashes(matrix)
    verify_automation(matrix)
    verify_oracle_and_sources()
    print("Phase 2D acceptance matrix: PASS (18/18, independent Decimal Oracle)")


if __name__ == "__main__":
    main()
