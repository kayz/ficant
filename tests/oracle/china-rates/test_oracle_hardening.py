import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent.parent
EXPECTED = ROOT / "tests" / "golden-cases" / "china-rates" / "expected" / "cgb-reference-v1-expected.json"
ORACLE_SOURCE = HERE / "quantlib_oracle.cpp"
FETCH_SCRIPT = HERE / "fetch_quantlib.sh"
TOOLCHAIN_LOCK = ROOT / "deploy" / "dev" / "toolchain.lock.toml"
SOURCE_URL = "https://github.com/lballabio/QuantLib/releases/download/v1.42.1/QuantLib-1.42.1.tar.gz"


def load_module(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def sha256(path):
    digest = hashlib.sha256()
    digest.update(path.read_bytes())
    return digest.hexdigest()


def artifact(path):
    return {"path": str(path.resolve()), "sha256": sha256(path)}


def executable(path, version):
    return {**artifact(path), "version": version}


def write_canonical_json(path, payload):
    path.write_bytes(
        (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
    )


def bash_path(path):
    path = Path(path).resolve()
    if os.name != "nt":
        return str(path)
    return f"/{path.drive[0].lower()}/{path.as_posix()[3:]}"


def aggregate_only_output():
    expected = json.loads(EXPECTED.read_text(encoding="utf-8"))
    fields = (
        "input_value", "clean_price", "dirty_price", "accrued_interest",
        "yield_to_maturity", "macaulay_duration", "modified_duration",
        "convexity", "dv01", "cashflow_count",
    )
    return {
        "quantlib_version": "1.42.1",
        "case_count": 12,
        "results": {
            key: {field: result[field] for field in fields}
            for key, result in expected["results"].items()
        },
    }


def conforming_output():
    expected = json.loads(EXPECTED.read_text(encoding="utf-8"))
    return {
        "schema": "ficant.test-oracle.quantlib-output.v2",
        "quantlib_version": "1.42.1",
        "case_count": 12,
        "candidate_id": expected["candidate_id"],
        "convention": expected["convention"],
        "calendar": expected["calendar"],
        "market_timezone": expected["market_timezone"],
        "valuation_at": expected["valuation_at"],
        "settlement_date": expected["settlement_date"],
        "oracle_identity": expected["oracle_identity"],
        "cashflow_semantics": expected["cashflow_semantics"],
        "results": json.loads(json.dumps(expected["results"])),
    }


def complete_cryptographic_build_manifest(work, output_path, program_path, archive_path):
    version_header = work / "version.hpp"
    version_header.write_text('#define QL_VERSION "1.42.1"\n', encoding="utf-8")
    cmake_cache = work / "CMakeCache.txt"
    install = work / "install"
    cmake_cache.write_text(
        "CMAKE_GENERATOR:INTERNAL=Ninja\n"
        f"CMAKE_MAKE_PROGRAM:FILEPATH={(work / 'ninja').resolve()}\n"
        "CMAKE_BUILD_TYPE:STRING=Release\n"
        f"CMAKE_INSTALL_PREFIX:PATH={install.resolve()}\n"
        "QL_BUILD_BENCHMARK:BOOL=OFF\n"
        "QL_BUILD_EXAMPLES:BOOL=OFF\n"
        "QL_BUILD_TEST_SUITE:BOOL=OFF\n",
        encoding="utf-8",
    )
    library = work / "libQuantLib.test"
    library.write_bytes(b"test installed QuantLib library")
    compiler = work / "c++"
    compiler.write_bytes(b"test compiler binary")
    cmake = work / "cmake"
    cmake.write_bytes(b"test CMake binary")
    ninja = work / "ninja"
    ninja.write_bytes(b"test Ninja binary")
    host_environment = work / "host-environment.json"
    write_canonical_json(host_environment, {
        "architecture": "test-architecture",
        "build_mode": "host-toolchain",
        "environment": {"CC": None, "CXX": str(compiler.resolve())},
        "os_release": {"PRETTY_NAME": "test-os 1"},
        "schema": "ficant.test-oracle.host-environment.v1",
        "uname": "test-os 1",
    })
    compile_identity = work / "compile-command.json"
    compile_command = [
        str(compiler.resolve()), "-std=c++20", "-O2", str(ORACLE_SOURCE.resolve()),
        f"-I{(install / 'include').resolve()}", str(library.resolve()),
        f"-Wl,-rpath,{(install / 'lib').resolve()}",
        "-o", str(program_path.resolve()),
    ]
    compile_identity.write_text(json.dumps(compile_command) + "\n", encoding="utf-8")
    manifest = {
        "schema": "ficant.test-oracle.quantlib-build-manifest.v3",
        "quantlib_version": "1.42.1",
        "build_mode": "host-toolchain",
        "container_image_digest": None,
        "source": {
            "url": SOURCE_URL,
            "archive": artifact(archive_path),
            "version_header": artifact(version_header),
            "version_header_identity": '#define QL_VERSION "1.42.1"',
        },
        "toolchain": {
            "compiler": executable(compiler, "test compiler 1"),
            "cmake": executable(cmake, "cmake version test"),
            "ninja": executable(ninja, "test Ninja 1"),
        },
        "repository": {
            "toolchain_lock": artifact(TOOLCHAIN_LOCK),
        },
        "environment": {
            "os": "test-os 1",
            "architecture": "test-architecture",
            "cmake_generator": "Ninja",
            "os_environment_manifest": artifact(host_environment),
            "cmake_config": {
                "CMAKE_BUILD_TYPE": "Release",
                "QL_BUILD_BENCHMARK": "OFF",
                "QL_BUILD_EXAMPLES": "OFF",
                "QL_BUILD_TEST_SUITE": "OFF",
                "CMAKE_INSTALL_PREFIX": str(install.resolve()),
            },
        },
        "build": {
            "cmake_cache": artifact(cmake_cache),
            "installed_library": artifact(library),
        },
        "oracle": {
            "source": artifact(ORACLE_SOURCE),
            "input": artifact(EXPECTED),
            "compile_command": compile_command,
            "compile_identity": artifact(compile_identity),
            "program": artifact(program_path),
            "output": artifact(output_path),
        },
    }
    aggregate_manifest = work / "toolchain-build-environment.json"
    write_canonical_json(aggregate_manifest, manifest)
    manifest["aggregate_manifest"] = artifact(aggregate_manifest)
    return manifest


def complete_build_manifest(work, output_path, program_path, archive_path):
    return complete_cryptographic_build_manifest(
        work, output_path, program_path, archive_path
    )


def run_verifier(output_payload, manifest_payload):
    work_holder = tempfile.TemporaryDirectory()
    work = Path(work_holder.name)
    archive = work / "QuantLib-1.42.1.tar.gz"
    archive.write_bytes(b"test archive")
    program = work / "quantlib_oracle"
    program.write_bytes(b"test program")
    output = work / "quantlib-output.json"
    output.write_text(json.dumps(output_payload), encoding="utf-8")
    manifest = work / "build-manifest.json"
    if manifest_payload == "complete":
        manifest_payload = complete_build_manifest(work, output, program, archive)
    manifest.write_text(json.dumps(manifest_payload), encoding="utf-8")
    evidence = work / "quantlib-evidence.json"
    completed = subprocess.run(
        [
            sys.executable, str(HERE / "verify_quantlib_output.py"),
            "--source-archive", str(archive),
            "--build-manifest", str(manifest),
            "--oracle-program", str(program),
            "--output", str(output),
            "--evidence", str(evidence),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        timeout=30,
    )
    work_holder.cleanup()
    return completed


class OracleHardeningTests(unittest.TestCase):
    def test_i3_d_oracle_001_rejects_output_missing_exact_cashflow_identity(self):
        completed = run_verifier(aggregate_only_output(), "complete")
        output = completed.stdout + completed.stderr
        self.assertNotEqual(completed.returncode, 0, output)
        self.assertIn("cashflow", output.lower())

    def test_i3_d_oracle_001_rejects_missing_frozen_timing_round_trip_and_finite_difference(self):
        verifier = load_module("complete_value_verifier_contract", "verify_quantlib_output.py")
        expected = json.loads(EXPECTED.read_text(encoding="utf-8"))
        output = conforming_output()
        for result in output["results"].values():
            result.pop("round_trip")
            result.pop("finite_difference")
            for flow in result["cashflows"]:
                flow.pop("time_years")
        errors = verifier.verify_output(output, expected)
        self.assertTrue(errors, "frozen timing/round-trip/finite-difference values were not re-verified")
        self.assertIn("round_trip", " ".join(errors))

    def test_i3_d_oracle_002_rejects_self_consistent_arbitrary_evidence(self):
        validator = load_module("validator_forgery_contract", "validator.py")
        verifier = load_module("verifier_forgery_contract", "verify_quantlib_output.py")
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            archive = work / "QuantLib-1.42.1.tar.gz"
            archive.write_bytes(b"arbitrary archive")
            program = work / "quantlib_oracle"
            program.write_bytes(b"arbitrary program")
            output_path = work / "quantlib-output.json"
            output_path.write_text(json.dumps({"arbitrary": "self-consistent"}), encoding="utf-8")
            manifest = complete_build_manifest(work, output_path, program, archive)
            manifest_path = work / "build-manifest.json"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            expected = json.loads(EXPECTED.read_text(encoding="utf-8"))
            evidence_artifacts = verifier.manifest_artifacts(manifest, manifest_path)
            evidence = {
                "schema": "ficant.test-oracle.quantlib-evidence.v3",
                "quantlib_version": "1.42.1",
                "status": "executed",
                "agreement_status": "within_frozen_tolerances",
                "case_count": 12,
                "expected_candidate": str(EXPECTED.resolve()),
                "expected_candidate_sha256": sha256(EXPECTED),
                "frozen_metadata": {
                    "candidate_id": expected["candidate_id"],
                    "convention": expected["convention"],
                    "calendar": expected["calendar"],
                    "market_timezone": expected["market_timezone"],
                    "valuation_at": expected["valuation_at"],
                    "settlement_date": expected["settlement_date"],
                    "acceptance_ids": expected["acceptance_ids"],
                },
                "artifacts": evidence_artifacts,
            }
            evidence_path = work / "evidence.json"
            evidence_path.write_text(json.dumps(evidence), encoding="utf-8")
            errors = validator.validate_quantlib_evidence(evidence_path)
        self.assertTrue(errors, "self-consistent arbitrary evidence was accepted")

    def test_i3_d_oracle_003_refuses_resolved_workdir_inside_repo_before_fetch(self):
        bash = shutil.which("bash")
        if os.name == "nt":
            git_bash = Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "Git" / "bin" / "bash.exe"
            if git_bash.exists():
                bash = str(git_bash)
        self.assertTrue(bash, "bash is required for the fetch safety behavior test")
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory) / "repo"
            script_dir = repo / "tests" / "oracle" / "china-rates"
            script_dir.mkdir(parents=True)
            copied_script = script_dir / "fetch_quantlib.sh"
            shutil.copy2(FETCH_SCRIPT, copied_script)
            subprocess.run(["git", "init", "-q", str(repo)], check=True, capture_output=True)
            env = os.environ.copy()
            env["FICANT_QUANTLIB_WORKDIR"] = bash_path(repo / "local-work")
            completed = subprocess.run(
                [
                    bash, "-c",
                    'curl() { echo CURL_CALLED; return 91; }; export -f curl; exec "$1"',
                    "oracle-fetch-test", bash_path(copied_script),
                ],
                cwd=repo, env=env,
                text=True, encoding="utf-8", errors="replace",
                capture_output=True, timeout=30,
            )
        output = completed.stdout + completed.stderr
        self.assertNotEqual(completed.returncode, 0, output)
        self.assertIn("REFUSING_REPO_LOCAL_WORKDIR", output)
        self.assertNotIn("CURL_CALLED", output)

    def test_i3_d_oracle_003_refuses_nonempty_unsentinelized_override_before_fetch(self):
        bash = shutil.which("bash")
        if os.name == "nt":
            git_bash = Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "Git" / "bin" / "bash.exe"
            if git_bash.exists():
                bash = str(git_bash)
        self.assertTrue(bash, "bash is required for the fetch safety behavior test")
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory) / "repo"
            script_dir = repo / "tests" / "oracle" / "china-rates"
            script_dir.mkdir(parents=True)
            copied_script = script_dir / "fetch_quantlib.sh"
            shutil.copy2(FETCH_SCRIPT, copied_script)
            subprocess.run(["git", "init", "-q", str(repo)], check=True, capture_output=True)
            unsafe_work = Path(directory) / "shared-cache"
            unsafe_work.mkdir()
            (unsafe_work / "unrelated.txt").write_text("must survive\n", encoding="utf-8")
            env = os.environ.copy()
            env["FICANT_QUANTLIB_WORKDIR"] = bash_path(unsafe_work)
            completed = subprocess.run(
                [
                    bash, "-c",
                    'curl() { echo CURL_CALLED; return 91; }; export -f curl; exec "$1"',
                    "oracle-fetch-test", bash_path(copied_script),
                ],
                cwd=repo, env=env, text=True, encoding="utf-8", errors="replace",
                capture_output=True, timeout=30,
            )
            self.assertEqual((unsafe_work / "unrelated.txt").read_text(encoding="utf-8"), "must survive\n")
        output = completed.stdout + completed.stderr
        self.assertNotEqual(completed.returncode, 0, output)
        self.assertIn("REFUSING_UNSAFE_WORKDIR", output)
        self.assertNotIn("CURL_CALLED", output)

    def test_i3_d_oracle_004_rejects_incomplete_build_identity(self):
        completed = run_verifier(aggregate_only_output(), {})
        output = completed.stdout + completed.stderr
        self.assertNotEqual(completed.returncode, 0, output)
        self.assertIn("build manifest", output.lower())

    def test_i3_d_oracle_004_rejects_unbound_cache_and_installed_library(self):
        verifier = load_module("build_binding_verifier_contract", "verify_quantlib_output.py")
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            archive = work / "QuantLib-1.42.1.tar.gz"
            archive.write_bytes(b"test archive")
            program = work / "quantlib_oracle"
            program.write_bytes(b"test program")
            output_path = work / "quantlib-output.json"
            output_path.write_text(json.dumps(conforming_output()), encoding="utf-8")
            manifest = complete_build_manifest(work, output_path, program, archive)
            cache_path = Path(manifest["build"]["cmake_cache"]["path"])
            cache_path.write_text("CMAKE_GENERATOR:INTERNAL=test-generator\n", encoding="utf-8")
            manifest["build"]["cmake_cache"] = artifact(cache_path)
            library_path = manifest["build"]["installed_library"]["path"]
            command = [item for item in manifest["oracle"]["compile_command"] if item != library_path]
            manifest["oracle"]["compile_command"] = command
            compile_path = Path(manifest["oracle"]["compile_identity"]["path"])
            compile_path.write_text(json.dumps(command) + "\n", encoding="utf-8")
            manifest["oracle"]["compile_identity"] = artifact(compile_path)
            manifest_path = work / "manifest.json"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            errors, _ = verifier.validate_build_manifest(
                manifest_path,
                source_archive=archive,
                oracle_program=program,
                output_path=output_path,
            )
        self.assertTrue(errors, "CMake cache and installed-library compile binding were not verified")

    def test_i3_d_oracle_005_maps_every_q_id_and_records_honest_lineage(self):
        builder = load_module("builder_lineage_contract", "build_oracle.py")
        payload = builder.build_expected()
        required_ids = {f"Q-{number:03d}" for number in range(1, 24)}
        self.assertIn("acceptance_mapping", payload)
        self.assertEqual(set(payload["acceptance_mapping"]), required_ids)
        for q_id, mapping in payload["acceptance_mapping"].items():
            self.assertTrue(mapping["cases"] or mapping["invariants"], q_id)
        identity = payload["oracle_identity"]
        self.assertEqual(identity["algorithm"], "ficant.cgb.fixed-rate.reference/1")
        self.assertEqual(identity["engine"], "ficant-fixed-income-native/0.1.0")
        self.assertEqual(identity["abi"], "FICANT_FIXED_INCOME_ABI_V1=1")
        self.assertNotIn("resolution", identity["calendar"])
        self.assertEqual(identity["calendar"]["resolution_scope"], "per_result")
        self.assertEqual(identity["calendar"]["coverage"], "2005-01-01..2026-12-31")
        self.assertEqual(identity["rule_pack"]["status"], "pending_production_proof")
        self.assertIsNone(identity["rule_pack"]["content_sha256"])
        self.assertEqual(identity["snapshot"]["status"], "source_manifest_only_no_production_snapshot_proof")
        self.assertIsNone(identity["snapshot"]["production_content_sha256"])
        self.assertEqual(payload["quality_status"], "pending_quality_approval")
        self.assertEqual(
            payload["provenance"]["quantlib_agreement"],
            "blocked_pending_independent_execution",
        )

    def test_i3_d_oracle_006_uses_one_based_cashflow_identity_in_every_producer(self):
        builder = load_module("builder_one_based_sequence_contract", "build_oracle.py")
        payload = builder.build_expected()
        self.assertEqual(
            payload["cashflow_semantics"]["sequence"],
            "one_based_ascending_payment_eligible_cashflow",
        )
        flows_seen = 0
        semantics_seen = [payload["cashflow_semantics"]]
        for case_key, result in payload["results"].items():
            flows = result["cashflows"]
            flows_seen += len(flows)
            semantics_seen.append(result["cashflow_semantics"])
            self.assertEqual(
                [flow["sequence"] for flow in flows],
                list(range(1, len(flows) + 1)),
                case_key,
            )
        self.assertEqual(flows_seen, 76)
        self.assertEqual(len(semantics_seen), 13)
        self.assertTrue(all(
            item["sequence"] == "one_based_ascending_payment_eligible_cashflow"
            for item in semantics_seen
        ))
        source = ORACLE_SOURCE.read_text(encoding="utf-8")
        self.assertIn("rows.size() + 1", source)
        self.assertIn("one_based_ascending_payment_eligible_cashflow", source)

    def test_i3_d_oracle_007_declares_per_result_frozen_calendar_resolution(self):
        builder = load_module("builder_calendar_resolution_contract", "build_oracle.py")
        payload = builder.build_expected()
        calendar = payload["oracle_identity"]["calendar"]
        self.assertNotIn("resolution", calendar)
        self.assertEqual(calendar["resolution_scope"], "per_result")
        self.assertEqual(calendar["resolution_policy"], {
            "exact_if": "all_required_dates_inside_frozen_exact_coverage",
            "exact_resolution": "EXACT",
            "otherwise_resolution": "PROVISIONAL_WEEKEND_ONLY",
        })
        resolutions = {
            key: result["identity"]["calendar_resolution"]
            for key, result in payload["results"].items()
        }
        self.assertEqual(
            {key for key, value in resolutions.items() if value == "EXACT"},
            {"269937.IB:YIELD_IN", "269937.IB:PRICE_IN"},
        )
        self.assertEqual(list(resolutions.values()).count("PROVISIONAL_WEEKEND_ONLY"), 10)
        allowed_sources = [
            HERE / "build_oracle.py",
            HERE / "quantlib_oracle.cpp",
            EXPECTED,
        ]
        for path in allowed_sources:
            rejected_legacy_name = "EXACT_MARKET" + "_CALENDAR"
            self.assertNotIn(rejected_legacy_name, path.read_text(encoding="utf-8"), str(path))

    def test_i3_d_oracle_008_binds_and_rejects_changed_toolchain_environment_identity(self):
        verifier = load_module("cryptographic_build_identity_contract", "verify_quantlib_output.py")
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            archive = work / "QuantLib-1.42.1.tar.gz"
            archive.write_bytes(b"test archive")
            program = work / "quantlib_oracle"
            program.write_bytes(b"test program")
            output_path = work / "quantlib-output.json"
            output_path.write_text(json.dumps(conforming_output()), encoding="utf-8")
            manifest = complete_cryptographic_build_manifest(work, output_path, program, archive)
            manifest_path = work / "build-manifest.json"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            errors, _ = verifier.validate_build_manifest(
                manifest_path,
                source_archive=archive,
                oracle_program=program,
                output_path=output_path,
            )
            self.assertFalse(errors, errors)

            mutations = {
                "missing compiler hash": (
                    lambda value: value["toolchain"]["compiler"].pop("sha256"),
                    "toolchain.compiler",
                ),
                "changed compiler hash": (
                    lambda value: value["toolchain"]["compiler"].update(sha256="0" * 64),
                    "toolchain.compiler hash mismatch",
                ),
                "changed CMake hash": (
                    lambda value: value["toolchain"]["cmake"].update(sha256="0" * 64),
                    "toolchain.cmake hash mismatch",
                ),
                "changed Ninja hash": (
                    lambda value: value["toolchain"]["ninja"].update(sha256="0" * 64),
                    "toolchain.ninja hash mismatch",
                ),
                "missing lock hash": (
                    lambda value: value["repository"]["toolchain_lock"].pop("sha256"),
                    "repository.toolchain_lock",
                ),
                "changed lock hash": (
                    lambda value: value["repository"]["toolchain_lock"].update(sha256="0" * 64),
                    "repository.toolchain_lock hash mismatch",
                ),
                "missing environment hash": (
                    lambda value: value["environment"]["os_environment_manifest"].pop("sha256"),
                    "environment.os_environment_manifest",
                ),
                "changed environment hash": (
                    lambda value: value["environment"]["os_environment_manifest"].update(sha256="0" * 64),
                    "environment.os_environment_manifest hash mismatch",
                ),
                "changed aggregate hash": (
                    lambda value: value["aggregate_manifest"].update(sha256="0" * 64),
                    "aggregate_manifest hash mismatch",
                ),
            }
            for label, (mutate, expected_error) in mutations.items():
                with self.subTest(label=label):
                    changed = json.loads(json.dumps(manifest))
                    mutate(changed)
                    manifest_path.write_text(json.dumps(changed), encoding="utf-8")
                    changed_errors, _ = verifier.validate_build_manifest(
                        manifest_path,
                        source_archive=archive,
                        oracle_program=program,
                        output_path=output_path,
                    )
                    self.assertTrue(changed_errors, label)
                    self.assertIn(expected_error, " ".join(changed_errors), changed_errors)

            noncanonical = json.loads(json.dumps(manifest))
            host_path = Path(noncanonical["environment"]["os_environment_manifest"]["path"])
            host_payload = json.loads(host_path.read_text(encoding="utf-8"))
            host_path.write_text(json.dumps(host_payload, indent=2) + "\n", encoding="utf-8")
            noncanonical["environment"]["os_environment_manifest"] = artifact(host_path)
            aggregate_path = Path(noncanonical["aggregate_manifest"]["path"])
            write_canonical_json(
                aggregate_path,
                {key: value for key, value in noncanonical.items() if key != "aggregate_manifest"},
            )
            noncanonical["aggregate_manifest"] = artifact(aggregate_path)
            manifest_path.write_text(json.dumps(noncanonical), encoding="utf-8")
            canonical_errors, _ = verifier.validate_build_manifest(
                manifest_path,
                source_archive=archive,
                oracle_program=program,
                output_path=output_path,
            )
            self.assertIn(
                "host environment manifest must be canonical JSON",
                " ".join(canonical_errors),
                canonical_errors,
            )

    def test_i3_d_oracle_014_routes_duration_by_discount_convention(self):
        source = ORACLE_SOURCE.read_text(encoding="utf-8")
        self.assertIn("const Duration::Type duration_type", source)
        self.assertIn("input.frequency == NoFrequency ? Duration::Simple : Duration::Macaulay", source)
        self.assertIn("valuation_leg, rate, duration_type, true", source)

    def test_i3_d_oracle_015_uses_actual_payment_leg_for_identity_and_accrued(self):
        source = ORACLE_SOURCE.read_text(encoding="utf-8")
        self.assertIn("const Leg& payment_leg = bond->cashflows()", source)
        self.assertIn("bond->accruedAmount(settlement)", source)
        self.assertIn("for (const auto& flow : payment_leg)", source)
        self.assertIn("if (flow->date() <= settlement) continue", source)

    def test_i3_d_oracle_016_values_payment_eligible_amounts_on_nominal_timeline(self):
        source = ORACLE_SOURCE.read_text(encoding="utf-8")
        self.assertIn("Leg valuation_leg", source)
        self.assertIn("std::max(nominal_date, settlement)", source)
        self.assertIn("valuation_leg.push_back(std::make_shared<SimpleCashFlow>", source)
        self.assertIn("valuation_leg, rate, true, settlement, settlement", source)

    def test_i3_d_oracle_017_reports_positive_central_difference_dv01(self):
        source = ORACLE_SOURCE.read_text(encoding="utf-8")
        self.assertIn("const Real finite_dv01 = std::abs(price_down - price_up) / 2.0", source)
        self.assertIn("finite_dv01, price_down, price_up, finite_dv01", source)
        self.assertIn("const Real analytic_dv01 = modified * dirty * 0.0001", source)


if __name__ == "__main__":
    unittest.main()
