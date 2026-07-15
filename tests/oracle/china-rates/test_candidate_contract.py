import importlib.util
import hashlib
import json
import subprocess
import sys
import unittest
from datetime import date
from decimal import Decimal
from pathlib import Path


HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent.parent
FIXTURES = ROOT / "tests" / "golden-cases" / "china-rates" / "fixtures"
EXPECTED = ROOT / "tests" / "golden-cases" / "china-rates" / "expected"

OBJECT_SHA = "765d8afe8605562dbf1c4d2a23513de25e98945496f8d297565c1d943eed8faf"
CANONICAL_SHA = "8216f586cbec959a08bb62a5e00c2492c99dc01e641e0c876a918b710e9d50ff"
BOND_IDS = [
    "269937.IB", "260013.IB", "260011.IB", "260008.IB", "260012.IB", "260010.IB"
]


def load_module(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


manual = load_module("oracle_manual_contract", "oracle_manual.py")


class CandidateAssetTests(unittest.TestCase):
    def test_source_manifest_blob_lf_identity_and_governed_attributes(self):
        manifest = "tests/golden-cases/china-rates/iteration-3-cgb-basic-info-source-manifest.json"
        governed = [
            manifest,
            "tests/golden-cases/china-rates/expected/cgb-reference-v1-expected.json",
            "tests/oracle/china-rates/build_oracle.py",
            "tests/oracle/china-rates/quantlib_oracle.cpp",
            "tests/oracle/china-rates/test_candidate_contract.py",
        ]
        blob = subprocess.run(
            ["git", "rev-parse", f"HEAD:{manifest}"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        self.assertEqual(blob, "05f4c74b4fb0766e39360cdbc965a8fc6089dad6")
        lf_content = subprocess.run(
            ["git", "show", f"HEAD:{manifest}"],
            cwd=ROOT,
            check=True,
            capture_output=True,
        ).stdout
        self.assertEqual(
            hashlib.sha256(lf_content).hexdigest(),
            "078c14aaa67bc3d819d0a089e415d13029e09d88d43d0946dbdf10e7e8221dd1",
        )
        attributes = subprocess.run(
            ["git", "check-attr", "text", "eol", "--", *governed],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
        self.assertEqual(len(attributes), len(governed) * 2)
        for line in attributes:
            self.assertRegex(line, r": (text: set|eol: lf)$")

    def test_q001_q012_six_bonds_have_both_modes_and_complete_results(self):
        payload = json.loads((EXPECTED / "cgb-reference-v1-expected.json").read_text(encoding="utf-8"))
        required = {
            "cashflows", "accrued_interest", "clean_price", "dirty_price",
            "yield_to_maturity", "macaulay_duration", "modified_duration",
            "convexity", "dv01", "round_trip", "finite_difference",
        }
        self.assertEqual(payload["quality_status"], "pending_quality_approval")
        self.assertFalse(payload["provenance"]["production_cpp_used"])
        for bond_id in BOND_IDS:
            for mode in ("YIELD_IN", "PRICE_IN"):
                result = payload["results"][f"{bond_id}:{mode}"]
                self.assertTrue(required.issubset(result), f"{bond_id}:{mode}")
                self.assertEqual(
                    Decimal(result["dirty_price"]),
                    Decimal(result["clean_price"]) + Decimal(result["accrued_interest"]),
                )
                for field in (
                    "input_value", "clean_price", "dirty_price", "accrued_interest",
                    "yield_to_maturity", "macaulay_duration", "modified_duration",
                    "convexity", "dv01",
                ):
                    self.assertRegex(result[field], r"^-?\d+\.\d{12}$")

    def test_normalized_fixtures_are_complete_and_preserve_exact_lineage(self):
        self.assertFalse((ROOT / "tests" / "golden-cases" / "china-rates" / "dm-basic-info-payload.json").exists())
        for bond_id in BOND_IDS:
            fixture = json.loads((FIXTURES / f"bond-{bond_id}.json").read_text(encoding="utf-8"))
            self.assertEqual(fixture["source_lineage"]["object_sha256"], OBJECT_SHA)
            self.assertEqual(fixture["source_lineage"]["canonical_records_sha256"], CANONICAL_SHA)
            self.assertEqual(fixture["source_lineage"]["record_count"], 6)
            self.assertEqual(fixture["market_timezone"], "Asia/Shanghai")
            self.assertEqual(fixture["convention"], "cgb-reference-v1")

    def test_builder_has_no_raw_payload_dependency(self):
        text = (HERE / "build_oracle.py").read_text(encoding="utf-8")
        self.assertNotIn("dm-basic-info-payload.json", text)
        self.assertNotIn("load_raw_data", text)


class OracleInvariantTests(unittest.TestCase):
    def test_q013_epoch_is_converted_in_asia_shanghai_before_date_truncation(self):
        self.assertTrue((HERE / "source_normalization.py").exists())
        normalization = load_module("source_normalization_contract", "source_normalization.py")
        epoch_ms = 1781712000000  # 2026-06-17 16:00:00Z = 2026-06-18 Asia/Shanghai
        self.assertEqual(normalization.market_date_from_epoch_ms(epoch_ms), date(2026, 6, 18))

    def test_q014_adjusted_payment_date_owns_cashflow_inclusion(self):
        before = manual.generate_cashflows(
            "T", date(2026, 5, 17), date(2027, 5, 15), 0.01, 1, date(2027, 5, 16)
        )
        on_payment = manual.generate_cashflows(
            "T", date(2026, 5, 17), date(2027, 5, 15), 0.01, 1, date(2027, 5, 17)
        )
        self.assertEqual(len(before), 1)
        self.assertEqual(before[0]["payment_date"], "2027-05-17")
        self.assertEqual(on_payment, [])

    def test_q015_following_adjustment_does_not_add_interest_time(self):
        flows = manual.generate_cashflows(
            "T", date(2026, 5, 15), date(2027, 5, 15), 0.01, 1, date(2026, 5, 15)
        )
        self.assertEqual(flows[0]["payment_date"], "2027-05-17")
        self.assertEqual(flows[0]["time_years"], Decimal("1"))

    def test_q016_discount_year_fraction_splits_leap_and_common_years(self):
        value = manual.actual_actual_discount(date(2024, 7, 1), date(2025, 7, 1))
        expected = Decimal(184) / Decimal(366) + Decimal(181) / Decimal(365)
        self.assertAlmostEqual(float(value), float(expected), places=14)

    def test_q017_discount_bond_uses_one_simple_yield_payment(self):
        flows = manual.generate_cashflows(
            "269937.IB", date(2026, 6, 18), date(2026, 12, 17), 0, 0, date(2026, 7, 14)
        )
        pv, _ = manual.compute_pv_given_yield(flows, 0.011, date(2026, 7, 14), 0)
        year_fraction = manual.actual_actual_discount(date(2026, 7, 14), date(2026, 12, 17))
        self.assertEqual(len(flows), 1)
        self.assertEqual(flows[0]["coupon"], Decimal("0"))
        self.assertAlmostEqual(float(pv), float(Decimal(100) / (Decimal(1) + Decimal("0.011") * Decimal(str(year_fraction)))), places=13)

    def test_q018_semiannual_schedule_has_both_coupon_anchors(self):
        flows = manual.generate_cashflows(
            "260010.IB", date(2026, 5, 15), date(2036, 5, 15), 0.0172, 2, date(2026, 7, 14)
        )
        self.assertEqual(len(flows), 20)
        self.assertEqual(flows[0]["nominal_date"], "2026-11-15")
        self.assertEqual(flows[-1]["nominal_date"], "2036-05-15")
        self.assertTrue(all(f["nominal_date"][5:] in {"05-15", "11-15"} for f in flows))

    def test_q019_settlement_boundary_is_strict(self):
        included = manual.generate_cashflows(
            "T", date(2026, 6, 18), date(2026, 12, 17), 0, 0, date(2026, 12, 16)
        )
        excluded = manual.generate_cashflows(
            "T", date(2026, 6, 18), date(2026, 12, 17), 0, 0, date(2026, 12, 17)
        )
        self.assertEqual(len(included), 1)
        self.assertEqual(excluded, [])

    def test_q020_cashflow_identity_and_amount_invariants(self):
        flows = manual.generate_cashflows(
            "T", date(2026, 6, 25), date(2028, 6, 25), 0.0121, 1, date(2026, 7, 14)
        )
        self.assertEqual([f["sequence"] for f in flows], [1, 2])
        for flow in flows:
            self.assertEqual(flow["total"], flow["coupon"] + flow["principal"])
        self.assertEqual(sum((f["principal"] for f in flows), Decimal(0)), Decimal(100))

    def test_q021_price_yield_round_trip(self):
        result = manual.value_bond(
            "260013.IB", date(2026, 6, 25), date(2028, 6, 25), 0.0121, 1,
            date(2026, 7, 14), "YIELD_IN", 0.013
        )
        inverse = manual.value_bond(
            "260013.IB", date(2026, 6, 25), date(2028, 6, 25), 0.0121, 1,
            date(2026, 7, 14), "PRICE_IN", result["clean_price"]
        )
        self.assertLessEqual(abs(inverse["yield_to_maturity"] - 0.013), 1e-10)

    def test_q022_decimal_output_is_twelve_place_half_even(self):
        self.assertEqual(manual.decimal12(Decimal("1.2345678901235")), "1.234567890124")
        self.assertEqual(manual.decimal12(Decimal("1.2345678901245")), "1.234567890124")

    def test_q023_centered_risk_finite_differences(self):
        result = manual.value_bond(
            "260010.IB", date(2026, 5, 15), date(2036, 5, 15), 0.0172, 2,
            date(2026, 7, 14), "YIELD_IN", 0.018
        )
        fd = result["finite_diff"]
        self.assertLessEqual(fd["dv01_rel_diff"], 1e-4)
        self.assertLessEqual(fd["convexity_rel_diff"], 1e-4)


if __name__ == "__main__":
    unittest.main()
