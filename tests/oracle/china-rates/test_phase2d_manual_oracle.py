import hashlib
import json
from decimal import Decimal
from pathlib import Path

from phase2d_manual_oracle import build_expected

ROOT = Path(__file__).resolve().parents[3]
INPUTS = ROOT / "tests/golden-cases/china-rates/phase2d-futures-hedge-inputs.json"
EXPECTED = ROOT / "tests/golden-cases/china-rates/expected/phase2d-futures-hedge-v1-expected.json"


def test_independent_decimal_oracle_matches_frozen_expected() -> None:
    inputs = json.loads(INPUTS.read_text(encoding="utf-8"))
    expected = json.loads(EXPECTED.read_text(encoding="utf-8"))
    assert build_expected(inputs) == expected


def test_golden_suite_covers_direction_rounding_exactness_and_tie() -> None:
    inputs = json.loads(INPUTS.read_text(encoding="utf-8"))
    expected = build_expected(inputs)
    assert {case["product"] for case in inputs["cases"]} == {"TS", "TF", "T", "TL"}
    results = expected["case_results"]
    assert any(result["recommended_contracts"] < 0 for result in results.values())
    assert any(result["recommended_contracts"] > 0 for result in results.values())
    assert any(result["recommended_contracts"] == 0 for result in results.values())
    assert any(Decimal(result["hedge_effectiveness"]) == 1 for result in results.values())
    assert any(Decimal(result["hedge_effectiveness"]) == 0 for result in results.values())


def test_source_manifest_normalized_facts_digest_is_bound() -> None:
    manifest_path = ROOT / "tests/golden-cases/china-rates/phase2d-cffex-source-manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    canonical = json.dumps(
        manifest["normalized_facts"], ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    assert hashlib.sha256(canonical).hexdigest() == manifest["normalized_facts_sha256"]
    assert all(source["url"].startswith("https://www.cffex.com.cn/") for source in manifest["sources"])
