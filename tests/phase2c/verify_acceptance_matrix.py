"""Fail-closed integrity and independent-oracle verifier for Phase 2C."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from decimal import Decimal
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MATRIX = Path(__file__).with_name("acceptance-matrix.json")
INPUTS = ROOT / "tests/golden-cases/china-rates/phase2c-futures-delivery-inputs.json"
EXPECTED = (
    ROOT
    / "tests/golden-cases/china-rates/expected/phase2c-futures-delivery-v1-expected.json"
)
SOURCE_MANIFEST = ROOT / "tests/golden-cases/china-rates/phase2c-cffex-source-manifest.json"
ORACLE_SOURCE = ROOT / "tests/oracle/china-rates/phase2c_manual_oracle.py"

EXPECTED_IDS = {f"P2C-{index:03d}" for index in range(1, 19)}
EXPECTED_GUARDED = {
    "tests/golden-cases/china-rates/phase2c-futures-delivery-inputs.json",
    "tests/golden-cases/china-rates/expected/phase2c-futures-delivery-v1-expected.json",
    "tests/golden-cases/china-rates/phase2c-cffex-source-manifest.json",
    "tests/oracle/china-rates/phase2c_manual_oracle.py",
    "tests/oracle/china-rates/test_phase2c_manual_oracle.py",
    "crates/ficant-domain/tests/futures_delivery_contracts.rs",
    "crates/ficant-fixed-income-native/tests/futures_delivery_acceptance.rs",
    "crates/ficant-storage/tests/futures_delivery_arrow.rs",
    "crates/ficant-storage/tests/futures_delivery_sit.rs",
    "cpp/fixed-income-kernel/tests/test_futures_delivery.cpp",
    "cpp/fixed-income-kernel/tests/test_constants_and_layout.cpp",
}


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_oracle():
    spec = importlib.util.spec_from_file_location("phase2c_matrix_oracle", ORACLE_SOURCE)
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
    assert expected.get("schema") == "ficant.phase2c-futures-delivery.expected.v1"
    assert expected.get("ctd_bond_id") == "T-bond-ctd"
    assert {case["product"] for case in inputs["cases"]} == {"TS", "TF", "T", "TL"}
    assert {case["frequency"] for case in inputs["cases"]} == {1, 2}
    interim = [
        Decimal(result["interim_coupons"])
        for result in expected["case_results"].values()
    ]
    assert any(value > 0 for value in interim) and any(value == 0 for value in interim)

    source = ORACLE_SOURCE.read_text(encoding="utf-8")
    assert "Context(prec=50" in source and "ROUND_HALF_UP" in source
    assert "ficant_kernel" not in source and "ficant_fixed_income" not in source

    manifest = load_json(SOURCE_MANIFEST)
    assert manifest.get("schema") == "ficant.phase2c.cffex-source-manifest.v1"
    canonical = json.dumps(
        manifest["normalized_facts"],
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    assert hashlib.sha256(canonical).hexdigest() == manifest["normalized_facts_sha256"]
    assert manifest["normalized_facts_sha256"] == (
        "d1149c4594f3cc14ad977200e1bab6e48de3475d17dc03c7bb096ca369e05499"
    )
    assert all(
        item["url"].startswith("https://www.cffex.com.cn/")
        for item in manifest["sources"]
    )


def main():
    matrix = load_json(MATRIX)
    assert matrix.get("schema") == "ficant.quality.phase2c.acceptance-matrix.v1"
    assert matrix.get("base_commit") == "93dcf1efa1ed842a0c114457f356557f310ed18a"
    verify_hashes(matrix)
    verify_automation(matrix)
    verify_oracle_and_sources()
    print("Phase 2C acceptance matrix: PASS (18/18, independent Decimal Oracle)")


if __name__ == "__main__":
    main()
