import importlib.util
import json
import os
import sys
import unittest
from pathlib import Path


HERE = Path(__file__).resolve().parent


def default_evidence_path():
    workdir = os.environ.get("FICANT_QUANTLIB_WORKDIR")
    if workdir:
        return Path(workdir).expanduser() / "quantlib-evidence.json"
    cache_home = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")).expanduser()
    return cache_home / "ficant" / "iteration-3" / "quantlib-1.42.1" / "quantlib-evidence.json"


def load_verifier():
    spec = importlib.util.spec_from_file_location(
        "quantlib_integration_shared_verifier", HERE / "verify_quantlib_output.py"
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class QuantLibIntegrationTests(unittest.TestCase):
    def test_official_quantlib_1_42_1_was_independently_executed(self):
        evidence_path = Path(os.environ.get(
            "FICANT_QUANTLIB_EVIDENCE",
            default_evidence_path(),
        ))
        self.assertTrue(
            evidence_path.exists(),
            "BLOCKED: official QuantLib 1.42.1 evidence is absent; run the documented human-operator build command",
        )
        evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
        self.assertEqual(evidence["schema"], "ficant.test-oracle.quantlib-evidence.v3")
        self.assertEqual(evidence["quantlib_version"], "1.42.1")
        self.assertEqual(evidence["status"], "executed")
        self.assertEqual(evidence["agreement_status"], "within_frozen_tolerances")
        self.assertEqual(evidence["case_count"], 12)
        errors = load_verifier().verify_evidence_bundle(evidence_path)
        self.assertEqual(errors, [], "\n".join(errors))


if __name__ == "__main__":
    unittest.main()
