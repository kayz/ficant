#!/usr/bin/env python3
"""Layered validator for the I3-TW-ORACLE candidate.

Exit 0 requires both the deterministic normalized/manual layer and independently
executed official QuantLib 1.42.1 evidence.  A missing QuantLib prerequisite is
reported as BLOCKED and exits 1; it is never skipped or converted to green.
"""

import argparse
import hashlib
import importlib.util
import io
import json
import os
import sys
import unittest
from pathlib import Path


HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent.parent
DEFAULT_FIXTURES = ROOT / "tests" / "golden-cases" / "china-rates" / "fixtures"
DEFAULT_EXPECTED = ROOT / "tests" / "golden-cases" / "china-rates" / "expected"


def default_quantlib_workdir():
    override = os.environ.get("FICANT_QUANTLIB_WORKDIR")
    if override:
        return Path(override).expanduser()
    cache_home = os.environ.get("XDG_CACHE_HOME")
    if cache_home:
        return Path(cache_home).expanduser() / "ficant" / "iteration-3" / "quantlib-1.42.1"
    return Path.home() / ".cache" / "ficant" / "iteration-3" / "quantlib-1.42.1"


DEFAULT_QL_EVIDENCE = default_quantlib_workdir() / "quantlib-evidence.json"


def load_local(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def validate_assets(fixtures_dir, expected_dir):
    errors = []
    builder = load_local("oracle_builder_for_validator", "build_oracle.py")

    raw_path = fixtures_dir.parent / "dm-basic-info-payload.json"
    if raw_path.exists():
        errors.append(f"restricted raw payload must be absent: {raw_path}")

    expected_fixture_names = {f"bond-{bond['bond_id']}.json" for bond in builder.DERIVED_BONDS}
    actual_fixture_names = {path.name for path in fixtures_dir.glob("*.json")} if fixtures_dir.exists() else set()
    if actual_fixture_names != expected_fixture_names:
        errors.append(f"fixture names differ: expected={sorted(expected_fixture_names)} actual={sorted(actual_fixture_names)}")

    for bond in builder.DERIVED_BONDS:
        path = fixtures_dir / f"bond-{bond['bond_id']}.json"
        if not path.exists():
            errors.append(f"missing fixture: {path}")
            continue
        actual = json.loads(path.read_text(encoding="utf-8"))
        expected = builder.fixture_for(bond)
        if actual != expected:
            errors.append(f"fixture is not reproducible: {path.name}")

    expected_path = expected_dir / "cgb-reference-v1-expected.json"
    if not expected_path.exists():
        errors.append(f"missing expected candidate: {expected_path}")
    else:
        actual_expected = json.loads(expected_path.read_text(encoding="utf-8"))
        rebuilt_expected = builder.build_expected()
        if actual_expected != rebuilt_expected:
            errors.append("expected candidate differs from a fresh independent manual rebuild")
        if actual_expected.get("quality_status") != "pending_quality_approval":
            errors.append("expected candidate must remain pending_quality_approval")
        provenance = actual_expected.get("provenance", {})
        if provenance.get("production_cpp_used") is not False:
            errors.append("expected candidate must state production_cpp_used=false")
        if provenance.get("quantlib_agreement") != "blocked_pending_independent_execution":
            errors.append("candidate must not claim QuantLib agreement before Quality integration")
        required_ids = [f"Q-{number:03d}" for number in range(1, 24)]
        if actual_expected.get("acceptance_ids") != required_ids:
            errors.append("acceptance ID mapping must be exactly Q-001..Q-023")
        acceptance_mapping = actual_expected.get("acceptance_mapping", {})
        if list(acceptance_mapping) != required_ids:
            errors.append("machine-readable acceptance mapping must be exactly Q-001..Q-023")
        for q_id, mapping in acceptance_mapping.items():
            if not isinstance(mapping, dict) or not (
                mapping.get("cases") or mapping.get("invariants")
            ):
                errors.append(f"acceptance mapping is incomplete: {q_id}")
        identity = actual_expected.get("oracle_identity", {})
        if identity.get("role") != "frozen_target_contract_identity_not_oracle_execution_claim":
            errors.append("Oracle target identity role is missing")
        if identity.get("rule_pack", {}).get("status") != "pending_production_proof":
            errors.append("RulePack lineage must remain pending_production_proof")
        rule_pack = identity.get("rule_pack", {})
        if any(rule_pack.get(key) is not None for key in ("id", "version", "content_sha256")):
            errors.append("RulePack lineage must not invent an absent production proof")
        if identity.get("snapshot", {}).get("status") != "source_manifest_only_no_production_snapshot_proof":
            errors.append("Snapshot lineage must not claim absent production proof")
        snapshot = identity.get("snapshot", {})
        if any(
            snapshot.get(key) is not None
            for key in ("production_id", "production_version", "production_content_sha256")
        ):
            errors.append("Snapshot lineage must not invent an absent production proof")
        manifest_path = ROOT / snapshot.get("source_manifest", "")
        if not manifest_path.is_file():
            errors.append("Snapshot source manifest is missing")
        else:
            manifest_hash = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
            if manifest_hash != snapshot.get("source_manifest_sha256"):
                errors.append("Snapshot source manifest hash mismatch")
    return errors


def validate_manual_invariants():
    test_module = load_local("candidate_contract_for_validator", "test_candidate_contract.py")
    suite = unittest.defaultTestLoader.loadTestsFromModule(test_module)
    stream = io.StringIO()
    result = unittest.TextTestRunner(stream=stream, verbosity=2).run(suite)
    if result.wasSuccessful():
        return [], result.testsRun
    return [stream.getvalue().strip()], result.testsRun


def validate_quantlib_evidence(evidence_path):
    if not evidence_path.exists():
        return [f"official QuantLib 1.42.1 execution evidence absent: {evidence_path}"]
    verifier = load_local("quantlib_evidence_verifier_for_validator", "verify_quantlib_output.py")
    return verifier.verify_evidence_bundle(evidence_path)


def main(argv=None):
    parser = argparse.ArgumentParser(description="Validate I3-TW-ORACLE Q-001..Q-023")
    parser.add_argument("--fixtures-dir", type=Path, default=DEFAULT_FIXTURES)
    parser.add_argument("--expected-dir", type=Path, default=DEFAULT_EXPECTED)
    parser.add_argument(
        "--quantlib-evidence",
        type=Path,
        default=Path(os.environ.get("FICANT_QUANTLIB_EVIDENCE", DEFAULT_QL_EVIDENCE)),
    )
    args = parser.parse_args(argv)

    asset_errors = validate_assets(args.fixtures_dir, args.expected_dir)
    invariant_errors, invariant_count = validate_manual_invariants()
    manual_errors = asset_errors + invariant_errors

    print("I3-TW-ORACLE-REROUTE-1-R1")
    print("CONVENTION: cgb-reference-v1")
    print("VALUATION_AT: 2026-07-13T15:00:00+08:00")
    print("SETTLEMENT_DATE: 2026-07-14")
    print(f"NORMALIZED_MANUAL_TESTS: {invariant_count}")
    if manual_errors:
        print("NORMALIZED_MANUAL_LAYER: FAIL")
        print("Q-001..Q-023: FAIL")
        for error in manual_errors:
            print(f"MANUAL_ERROR: {error}")
    else:
        print("NORMALIZED_MANUAL_LAYER: PASS")
        print("Q-001..Q-023: PASS")

    quantlib_errors = validate_quantlib_evidence(args.quantlib_evidence)
    if quantlib_errors:
        print("QUANTLIB_INTEGRATION: BLOCKED")
        for error in quantlib_errors:
            print(f"QUANTLIB_BLOCKER: {error}")
    else:
        print("QUANTLIB_INTEGRATION: PASS")

    if manual_errors:
        print("OVERALL: FAILED")
        return 1
    if quantlib_errors:
        print("OVERALL: BLOCKED")
        return 1
    print("OVERALL: READY_PENDING_QUALITY_APPROVAL")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
