import subprocess
import sys
import unittest
from pathlib import Path


HERE = Path(__file__).resolve().parent


class ValidatorContractTests(unittest.TestCase):
    def test_missing_quantlib_is_the_only_integrated_blocker(self):
        completed = subprocess.run(
            [sys.executable, str(HERE / "validator.py")],
            cwd=HERE.parent.parent.parent,
            text=True,
            capture_output=True,
            timeout=30,
        )
        output = completed.stdout + completed.stderr
        self.assertEqual(completed.returncode, 1, output)
        self.assertIn("NORMALIZED_MANUAL_LAYER: PASS", output)
        self.assertIn("Q-001..Q-023: PASS", output)
        self.assertIn("QUANTLIB_INTEGRATION: BLOCKED", output)
        self.assertIn("OVERALL: BLOCKED", output)
        self.assertNotIn("raw_payload_exists", output)


if __name__ == "__main__":
    unittest.main()
