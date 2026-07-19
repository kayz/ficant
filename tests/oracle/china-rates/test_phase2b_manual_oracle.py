import importlib.util
import json
import sys
import unittest
from decimal import Decimal
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
INPUT = ROOT / "tests/golden-cases/china-rates/phase2b-curve-carry-inputs.json"
EXPECTED = ROOT / "tests/golden-cases/china-rates/expected/phase2b-curve-carry-v1-expected.json"


def load_local(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules.setdefault(name, module)
    spec.loader.exec_module(module)
    return module


oracle = load_local("phase2b_manual_oracle_contract", "phase2b_manual_oracle.py")


class Phase2BManualOracleTests(unittest.TestCase):
    def setUp(self):
        self.inputs = json.loads(INPUT.read_text(encoding="utf-8"))
        self.expected = json.loads(EXPECTED.read_text(encoding="utf-8"))

    def test_frozen_expected_is_exact_manual_oracle_output(self):
        self.assertEqual(oracle.build_expected(self.inputs), self.expected)

    def test_curve_cases_cover_node_midpoint_and_uneven_interval(self):
        results = self.expected["curve_results"]
        self.assertEqual(results["CURVE-EXACT-NODE"]["yield_to_maturity"], "0.019000000000")
        self.assertEqual(results["CURVE-EXACT-MIDPOINT"]["yield_to_maturity"], "0.015000000000")
        self.assertNotEqual(
            results["CURVE-UNEVEN-INTERVAL"]["yield_to_maturity"],
            "0.018250000000",
            "uneven actual-day interval must not be treated as a midpoint",
        )

    def test_coupon_and_discount_cases_satisfy_frozen_decomposition(self):
        coupon = self.expected["carry_results"]["CARRY-COUPON-UPWARD"]
        discount = self.expected["carry_results"]["CARRY-DISCOUNT-UPWARD"]
        self.assertEqual(coupon["paid_cashflows"], "2.000000000000")
        self.assertEqual(discount["paid_cashflows"], "0.000000000000")
        for result in (coupon, discount):
            carry = (
                Decimal(result["horizon_dirty_at_initial_yield"])
                + Decimal(result["paid_cashflows"])
                - Decimal(result["initial_dirty_price"])
            )
            roll = (
                Decimal(result["horizon_dirty_at_rolled_yield"])
                - Decimal(result["horizon_dirty_at_initial_yield"])
            )
            self.assertLessEqual(abs(carry - Decimal(result["carry"])), Decimal("0.000000000001"))
            self.assertLessEqual(abs(roll - Decimal(result["roll_down"])), Decimal("0.000000000001"))
            self.assertLessEqual(
                abs(carry + roll - Decimal(result["total_return"])),
                Decimal("0.000000000001"),
            )

    def test_out_of_range_curve_query_fails_closed(self):
        with self.assertRaisesRegex(ValueError, "outside frozen curve range"):
            oracle.linear_yield(self.inputs["curve"], oracle.parse_date("2031-01-01"))


if __name__ == "__main__":
    unittest.main()
