import json
import hashlib
from pathlib import Path

from phase2c_manual_oracle import build_expected

ROOT = Path(__file__).resolve().parents[3]
INPUTS = ROOT / "tests/golden-cases/china-rates/phase2c-futures-delivery-inputs.json"
EXPECTED = ROOT / "tests/golden-cases/china-rates/expected/phase2c-futures-delivery-v1-expected.json"


def test_independent_decimal_oracle_matches_frozen_expected() -> None:
    inputs = json.loads(INPUTS.read_text(encoding="utf-8"))
    expected = json.loads(EXPECTED.read_text(encoding="utf-8"))
    assert build_expected(inputs) == expected


def test_golden_suite_covers_all_products_and_coupon_paths() -> None:
    inputs = json.loads(INPUTS.read_text(encoding="utf-8"))
    assert {case["product"] for case in inputs["cases"]} == {"TS", "TF", "T", "TL"}
    assert {case["frequency"] for case in inputs["cases"]} == {1, 2}
    expected = build_expected(inputs)
    interim = [Decimal(result["interim_coupons"]) for result in expected["case_results"].values()]
    assert any(value > 0 for value in interim)
    assert any(value == 0 for value in interim)


from decimal import Decimal


def test_source_manifest_normalized_facts_digest_is_bound() -> None:
    manifest_path = ROOT / "tests/golden-cases/china-rates/phase2c-cffex-source-manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    canonical = json.dumps(
        manifest["normalized_facts"], ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    assert hashlib.sha256(canonical).hexdigest() == manifest["normalized_facts_sha256"]
    assert all(source["url"].startswith("https://www.cffex.com.cn/") for source in manifest["sources"])
