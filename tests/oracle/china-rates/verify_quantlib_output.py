#!/usr/bin/env python3
"""Verify QuantLib output and its complete independently-built evidence bundle."""

import argparse
import hashlib
import json
from decimal import Decimal, InvalidOperation
from pathlib import Path


HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent.parent
EXPECTED = ROOT / "tests" / "golden-cases" / "china-rates" / "expected" / "cgb-reference-v1-expected.json"
ORACLE_SOURCE = HERE / "quantlib_oracle.cpp"
TOOLCHAIN_LOCK = ROOT / "deploy" / "dev" / "toolchain.lock.toml"
QUANTLIB_VERSION = "1.42.1"
SOURCE_URL = "https://github.com/lballabio/QuantLib/releases/download/v1.42.1/QuantLib-1.42.1.tar.gz"
OUTPUT_SCHEMA = "ficant.test-oracle.quantlib-output.v2"
BUILD_SCHEMA = "ficant.test-oracle.quantlib-build-manifest.v3"
EVIDENCE_SCHEMA = "ficant.test-oracle.quantlib-evidence.v3"
VERSION_HEADER_IDENTITY = '#define QL_VERSION "1.42.1"'


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def within(actual, expected, absolute=None, relative=None, floor=None):
    try:
        actual = Decimal(str(actual))
        expected = Decimal(str(expected))
        if not actual.is_finite() or not expected.is_finite():
            return False
        difference = abs(actual - expected)
        if absolute is not None:
            return difference <= absolute
        if floor is not None and difference <= floor:
            return True
        return difference / max(abs(expected), Decimal("1e-30")) <= relative
    except (InvalidOperation, TypeError, ValueError):
        return False


def load_json(path, label, errors):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception as error:
        errors.append(f"invalid {label} JSON: {error}")
        return None


def artifact_errors(reference, label, expected_path=None):
    errors = []
    if not isinstance(reference, dict) or set(reference) != {"path", "sha256"}:
        return [f"build manifest {label} must contain exactly path and sha256"]
    path_value = reference.get("path")
    recorded_hash = reference.get("sha256")
    if not isinstance(path_value, str) or not path_value:
        return [f"build manifest {label}.path is missing"]
    if not isinstance(recorded_hash, str) or len(recorded_hash) != 64:
        return [f"build manifest {label}.sha256 is invalid"]
    try:
        int(recorded_hash, 16)
    except ValueError:
        return [f"build manifest {label}.sha256 is invalid"]
    path = Path(path_value)
    if not path.is_absolute():
        errors.append(f"build manifest {label}.path must be absolute")
        return errors
    if expected_path is not None and path.resolve() != Path(expected_path).resolve():
        errors.append(f"build manifest {label}.path identity mismatch")
    if not path.is_file():
        errors.append(f"build manifest {label} artifact missing: {path}")
    elif sha256(path) != recorded_hash:
        errors.append(f"build manifest {label} hash mismatch")
    return errors


def executable_errors(reference, label):
    if not isinstance(reference, dict) or set(reference) != {"path", "sha256", "version"}:
        return [f"build manifest {label} must contain exactly path, sha256, and version"]
    errors = artifact_errors(
        {"path": reference.get("path"), "sha256": reference.get("sha256")},
        label,
    )
    if not isinstance(reference.get("version"), str) or not reference["version"].strip():
        errors.append(f"build manifest {label}.version is missing")
    return errors


def canonical_json_bytes(payload):
    return (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")


def validate_build_manifest(
    manifest_path,
    *,
    expected_path=EXPECTED,
    source_archive=None,
    oracle_program=None,
    output_path=None,
):
    errors = []
    manifest = load_json(Path(manifest_path), "build manifest", errors)
    if manifest is None:
        return errors, None
    if manifest.get("schema") != BUILD_SCHEMA:
        errors.append(f"build manifest schema must be {BUILD_SCHEMA}")
    if manifest.get("quantlib_version") != QUANTLIB_VERSION:
        errors.append("build manifest QuantLib version mismatch")
    if manifest.get("build_mode") != "host-toolchain":
        errors.append("build manifest build mode must be host-toolchain")
    if manifest.get("container_image_digest") is not None:
        errors.append("host-toolchain build manifest must not invent a container image digest")

    source = manifest.get("source")
    if not isinstance(source, dict):
        errors.append("build manifest source identity is missing")
        source = {}
    if source.get("url") != SOURCE_URL:
        errors.append("build manifest source URL mismatch")
    if source.get("version_header_identity") != VERSION_HEADER_IDENTITY:
        errors.append("build manifest version header identity mismatch")
    errors.extend(artifact_errors(source.get("archive"), "source.archive", source_archive))
    errors.extend(artifact_errors(source.get("version_header"), "source.version_header"))
    version_header = source.get("version_header")
    version_header_path = version_header.get("path") if isinstance(version_header, dict) else None
    if isinstance(version_header_path, str) and Path(version_header_path).is_file():
        header_text = Path(version_header_path).read_text(encoding="utf-8", errors="replace")
        if VERSION_HEADER_IDENTITY not in header_text.splitlines():
            errors.append("build manifest version header does not declare QuantLib 1.42.1")

    toolchain = manifest.get("toolchain")
    if not isinstance(toolchain, dict):
        errors.append("build manifest toolchain identity is missing")
        toolchain = {}
    errors.extend(executable_errors(toolchain.get("compiler"), "toolchain.compiler"))
    errors.extend(executable_errors(toolchain.get("cmake"), "toolchain.cmake"))

    repository = manifest.get("repository")
    if not isinstance(repository, dict):
        errors.append("build manifest repository identity is missing")
        repository = {}
    errors.extend(artifact_errors(
        repository.get("toolchain_lock"),
        "repository.toolchain_lock",
        TOOLCHAIN_LOCK,
    ))

    environment = manifest.get("environment")
    if not isinstance(environment, dict):
        errors.append("build manifest environment identity is missing")
        environment = {}
    for key in ("os", "architecture", "cmake_generator"):
        if not isinstance(environment.get(key), str) or not environment[key].strip():
            errors.append(f"build manifest environment.{key} is missing")
    errors.extend(artifact_errors(
        environment.get("os_environment_manifest"),
        "environment.os_environment_manifest",
    ))
    host_reference = environment.get("os_environment_manifest")
    host_path = host_reference.get("path") if isinstance(host_reference, dict) else None
    if isinstance(host_path, str) and Path(host_path).is_file():
        host_payload = load_json(Path(host_path), "host environment manifest", errors)
        if isinstance(host_payload, dict):
            if host_payload.get("schema") != "ficant.test-oracle.host-environment.v1":
                errors.append("build manifest host environment schema mismatch")
            if host_payload.get("build_mode") != manifest.get("build_mode"):
                errors.append("build manifest host environment build mode mismatch")
            if host_payload.get("uname") != environment.get("os"):
                errors.append("build manifest host environment OS identity mismatch")
            if host_payload.get("architecture") != environment.get("architecture"):
                errors.append("build manifest host environment architecture mismatch")
            if Path(host_path).read_bytes() != canonical_json_bytes(host_payload):
                errors.append("build manifest host environment manifest must be canonical JSON")
    config = environment.get("cmake_config")
    required_config = {
        "CMAKE_BUILD_TYPE": "Release",
        "QL_BUILD_BENCHMARK": "OFF",
        "QL_BUILD_EXAMPLES": "OFF",
        "QL_BUILD_TEST_SUITE": "OFF",
    }
    if not isinstance(config, dict):
        errors.append("build manifest CMake configuration is missing")
    else:
        for key, value in required_config.items():
            if config.get(key) != value:
                errors.append(f"build manifest CMake configuration {key} mismatch")
        if not isinstance(config.get("CMAKE_INSTALL_PREFIX"), str) or not config["CMAKE_INSTALL_PREFIX"]:
            errors.append("build manifest CMake install prefix is missing")

    build = manifest.get("build")
    if not isinstance(build, dict):
        errors.append("build manifest build identity is missing")
        build = {}
    errors.extend(artifact_errors(build.get("cmake_cache"), "build.cmake_cache"))
    errors.extend(artifact_errors(build.get("installed_library"), "build.installed_library"))
    cache_reference = build.get("cmake_cache")
    cache_path = cache_reference.get("path") if isinstance(cache_reference, dict) else None
    if isinstance(cache_path, str) and Path(cache_path).is_file() and isinstance(config, dict):
        cache_values = {}
        for line in Path(cache_path).read_text(encoding="utf-8", errors="replace").splitlines():
            if "=" not in line or line.startswith(("#", "//")):
                continue
            key_with_type, value = line.split("=", 1)
            key = key_with_type.split(":", 1)[0]
            cache_values[key] = value
        for key, value in required_config.items():
            if cache_values.get(key) != value:
                errors.append(f"build manifest CMake cache {key} mismatch")
        if cache_values.get("CMAKE_INSTALL_PREFIX") != config.get("CMAKE_INSTALL_PREFIX"):
            errors.append("build manifest CMake cache install prefix mismatch")
        if cache_values.get("CMAKE_GENERATOR") != environment.get("cmake_generator"):
            errors.append("build manifest CMake cache generator mismatch")
        if environment.get("cmake_generator", "").startswith("Ninja"):
            errors.extend(executable_errors(toolchain.get("ninja"), "toolchain.ninja"))
            ninja_path = toolchain.get("ninja", {}).get("path") if isinstance(
                toolchain.get("ninja"), dict
            ) else None
            cache_ninja = cache_values.get("CMAKE_MAKE_PROGRAM")
            if isinstance(ninja_path, str) and isinstance(cache_ninja, str):
                if Path(cache_ninja).resolve() != Path(ninja_path).resolve():
                    errors.append("build manifest CMake cache Ninja binary mismatch")
            elif ninja_path:
                errors.append("build manifest CMake cache does not bind Ninja binary")
        elif toolchain.get("ninja") is not None:
            errors.append("build manifest must omit Ninja identity when Ninja is not used")

    oracle = manifest.get("oracle")
    if not isinstance(oracle, dict):
        errors.append("build manifest Oracle identity is missing")
        oracle = {}
    errors.extend(artifact_errors(oracle.get("source"), "oracle.source", ORACLE_SOURCE))
    errors.extend(artifact_errors(oracle.get("input"), "oracle.input", expected_path))
    errors.extend(artifact_errors(oracle.get("compile_identity"), "oracle.compile_identity"))
    errors.extend(artifact_errors(oracle.get("program"), "oracle.program", oracle_program))
    errors.extend(artifact_errors(oracle.get("output"), "oracle.output", output_path))
    compile_command = oracle.get("compile_command")
    if not isinstance(compile_command, list) or not compile_command or not all(
        isinstance(item, str) and item for item in compile_command
    ):
        errors.append("build manifest Oracle compile command identity is missing")
    compile_identity = oracle.get("compile_identity")
    compile_identity_path = compile_identity.get("path") if isinstance(compile_identity, dict) else None
    if isinstance(compile_identity_path, str) and Path(compile_identity_path).is_file():
        compile_payload = load_json(Path(compile_identity_path), "compile identity", errors)
        if compile_payload is not None and compile_payload != compile_command:
            errors.append("build manifest Oracle compile identity mismatch")
    if isinstance(compile_command, list):
        source_path = oracle.get("source", {}).get("path") if isinstance(oracle.get("source"), dict) else None
        program_path = oracle.get("program", {}).get("path") if isinstance(oracle.get("program"), dict) else None
        if isinstance(source_path, str) and source_path not in compile_command:
            errors.append("build manifest Oracle compile command does not bind Oracle source")
        if isinstance(program_path, str) and program_path not in compile_command:
            errors.append("build manifest Oracle compile command does not bind Oracle program")
        library_path = build.get("installed_library", {}).get("path") if isinstance(
            build.get("installed_library"), dict
        ) else None
        if isinstance(library_path, str) and library_path not in compile_command:
            errors.append("build manifest Oracle compile command does not bind installed library")
        compiler_path = toolchain.get("compiler", {}).get("path") if isinstance(
            toolchain.get("compiler"), dict
        ) else None
        if compiler_path and compile_command and compile_command[0] != compiler_path:
            errors.append("build manifest Oracle compiler path differs from compile command")
        for required_flag in ("-std=c++20", "-O2", "-o"):
            if required_flag not in compile_command:
                errors.append(f"build manifest Oracle compile command missing {required_flag}")

    aggregate = manifest.get("aggregate_manifest")
    errors.extend(artifact_errors(aggregate, "aggregate_manifest"))
    aggregate_path = aggregate.get("path") if isinstance(aggregate, dict) else None
    if isinstance(aggregate_path, str) and Path(aggregate_path).is_file():
        projection = {key: value for key, value in manifest.items() if key != "aggregate_manifest"}
        aggregate_bytes = Path(aggregate_path).read_bytes()
        if aggregate_bytes != canonical_json_bytes(projection):
            errors.append("build manifest canonical aggregate identity mismatch")
    return errors, manifest


def verify_output(output, expected):
    errors = []
    frozen_metadata = {
        "schema": OUTPUT_SCHEMA,
        "quantlib_version": QUANTLIB_VERSION,
        "case_count": 12,
        "candidate_id": expected.get("candidate_id"),
        "convention": expected.get("convention"),
        "calendar": expected.get("calendar"),
        "market_timezone": expected.get("market_timezone"),
        "valuation_at": expected.get("valuation_at"),
        "settlement_date": expected.get("settlement_date"),
        "oracle_identity": expected.get("oracle_identity"),
        "cashflow_semantics": expected.get("cashflow_semantics"),
    }
    for key, value in frozen_metadata.items():
        if output.get(key) != value:
            errors.append(f"output frozen metadata mismatch: {key}")

    actual_results = output.get("results")
    expected_results = expected.get("results", {})
    if not isinstance(actual_results, dict):
        return errors + ["output results must be an object"]
    if set(actual_results) != set(expected_results):
        errors.append("output case keys mismatch")
        return errors

    tolerances = expected["tolerances"]
    price_tolerance = Decimal(tolerances["price_accrued_abs"])
    absolute_fields = {
        "clean_price": price_tolerance,
        "dirty_price": price_tolerance,
        "accrued_interest": price_tolerance,
        "yield_to_maturity": Decimal(tolerances["ytm_abs"]),
        "dv01": Decimal(tolerances["dv01_abs"]),
    }
    relative_fields = {"macaulay_duration", "modified_duration", "convexity"}
    exact_case_fields = {
        "bond_id", "mode", "settlement_date", "identity",
        "cashflow_semantics", "units",
    }
    exact_flow_fields = {"sequence", "nominal_date", "payment_date", "components"}
    failures = []
    for key, actual in actual_results.items():
        target = expected_results[key]
        if not isinstance(actual, dict):
            failures.append(f"{key}: result must be an object")
            continue
        for field in exact_case_fields:
            if actual.get(field) != target.get(field):
                failures.append(f"{key}.{field}")
        if actual.get("cashflow_count") != target.get("cashflow_count"):
            failures.append(f"{key}.cashflow_count")
        target_flows = target.get("cashflows", [])
        actual_flows = actual.get("cashflows")
        if not isinstance(actual_flows, list):
            failures.append(f"{key}.cashflows missing exact cashflow identity")
        elif len(actual_flows) != len(target_flows):
            failures.append(f"{key}.cashflows length")
        else:
            for index, (actual_flow, target_flow) in enumerate(zip(actual_flows, target_flows)):
                if not isinstance(actual_flow, dict):
                    failures.append(f"{key}.cashflows[{index}] identity")
                    continue
                for field in exact_flow_fields:
                    if actual_flow.get(field) != target_flow.get(field):
                        failures.append(f"{key}.cashflows[{index}].{field}")
                for field in ("coupon", "principal", "total"):
                    if not within(actual_flow.get(field), target_flow.get(field), absolute=price_tolerance):
                        failures.append(f"{key}.cashflows[{index}].{field}")
                try:
                    component_total = (
                        Decimal(str(actual_flow.get("coupon")))
                        + Decimal(str(actual_flow.get("principal")))
                    )
                except (InvalidOperation, TypeError, ValueError):
                    component_total = Decimal("NaN")
                if not within(component_total, actual_flow.get("total"), absolute=price_tolerance):
                    failures.append(f"{key}.cashflows[{index}].total_semantics")
                if not within(
                    actual_flow.get("time_years"), target_flow.get("time_years"),
                    relative=Decimal(tolerances["duration_convexity_rel"]),
                    floor=Decimal(tolerances["duration_convexity_abs_floor"]),
                ):
                    failures.append(f"{key}.cashflows[{index}].time_years")
        input_tolerance = (
            Decimal(tolerances["ytm_abs"])
            if target["mode"] == "YIELD_IN" else price_tolerance
        )
        if not within(actual.get("input_value"), target.get("input_value"), absolute=input_tolerance):
            failures.append(f"{key}.input_value")
        for field, tolerance in absolute_fields.items():
            if not within(actual.get(field), target.get(field), absolute=tolerance):
                failures.append(f"{key}.{field}")
        for field in relative_fields:
            if not within(
                actual.get(field), target.get(field),
                relative=Decimal(tolerances["duration_convexity_rel"]),
                floor=Decimal(tolerances["duration_convexity_abs_floor"]),
            ):
                failures.append(f"{key}.{field}")
        actual_round_trip = actual.get("round_trip")
        target_round_trip = target.get("round_trip", {})
        if not isinstance(actual_round_trip, dict):
            failures.append(f"{key}.round_trip")
        else:
            if not within(
                actual_round_trip.get("yield_to_maturity"),
                target_round_trip.get("yield_to_maturity"),
                absolute=Decimal(tolerances["ytm_abs"]),
            ):
                failures.append(f"{key}.round_trip.yield_to_maturity")
            if not within(
                actual_round_trip.get("absolute_difference"),
                target_round_trip.get("absolute_difference"),
                absolute=Decimal(tolerances["ytm_abs"]),
            ):
                failures.append(f"{key}.round_trip.absolute_difference")
        actual_finite = actual.get("finite_difference")
        target_finite = target.get("finite_difference", {})
        if not isinstance(actual_finite, dict):
            failures.append(f"{key}.finite_difference")
        else:
            finite_checks = {
                "bump_decimal": Decimal(tolerances["ytm_abs"]),
                "price_minus_1bp": price_tolerance,
                "price_plus_1bp": price_tolerance,
                "dv01": Decimal(tolerances["dv01_abs"]),
            }
            for field, tolerance in finite_checks.items():
                if not within(actual_finite.get(field), target_finite.get(field), absolute=tolerance):
                    failures.append(f"{key}.finite_difference.{field}")
            if not within(
                actual_finite.get("convexity"), target_finite.get("convexity"),
                relative=Decimal(tolerances["finite_difference_rel"]),
                floor=Decimal(tolerances["duration_convexity_abs_floor"]),
            ):
                failures.append(f"{key}.finite_difference.convexity")
            relationship_tolerance = Decimal(tolerances["finite_difference_rel"])
            for field in ("dv01_relative_difference", "convexity_relative_difference"):
                if not within(
                    actual_finite.get(field), target_finite.get(field),
                    absolute=relationship_tolerance,
                ):
                    failures.append(f"{key}.finite_difference.{field}")
    if failures:
        errors.append("QuantLib disagreement: " + ", ".join(failures))
    return errors


def manifest_artifacts(manifest, manifest_path):
    artifacts = {
        "source_archive": manifest["source"]["archive"],
        "version_header": manifest["source"]["version_header"],
        "compiler_binary": {
            key: manifest["toolchain"]["compiler"][key] for key in ("path", "sha256")
        },
        "cmake_binary": {
            key: manifest["toolchain"]["cmake"][key] for key in ("path", "sha256")
        },
        "toolchain_lock": manifest["repository"]["toolchain_lock"],
        "os_environment_manifest": manifest["environment"]["os_environment_manifest"],
        "cmake_cache": manifest["build"]["cmake_cache"],
        "installed_library": manifest["build"]["installed_library"],
        "oracle_source": manifest["oracle"]["source"],
        "oracle_input": manifest["oracle"]["input"],
        "compile_identity": manifest["oracle"]["compile_identity"],
        "oracle_program": manifest["oracle"]["program"],
        "output": manifest["oracle"]["output"],
        "aggregate_manifest": manifest["aggregate_manifest"],
        "build_manifest": {"path": str(Path(manifest_path).resolve()), "sha256": sha256(Path(manifest_path))},
    }
    ninja = manifest["toolchain"].get("ninja")
    if ninja is not None:
        artifacts["ninja_binary"] = {key: ninja[key] for key in ("path", "sha256")}
    return artifacts


def verify_evidence_bundle(evidence_path, expected_path=EXPECTED):
    errors = []
    evidence_path = Path(evidence_path).resolve()
    repository_root = ROOT.resolve()
    if evidence_path == repository_root or repository_root in evidence_path.parents:
        errors.append("QuantLib evidence must be outside the repository")
    evidence = load_json(evidence_path, "QuantLib evidence", errors)
    if evidence is None:
        return errors
    required_values = {
        "schema": EVIDENCE_SCHEMA,
        "quantlib_version": QUANTLIB_VERSION,
        "status": "executed",
        "agreement_status": "within_frozen_tolerances",
        "case_count": 12,
    }
    for key, value in required_values.items():
        if evidence.get(key) != value:
            errors.append(f"QuantLib evidence {key}: expected {value!r}, got {evidence.get(key)!r}")
    expected_path = Path(expected_path).resolve()
    if evidence.get("expected_candidate") != str(expected_path):
        errors.append("QuantLib evidence expected candidate path mismatch")
    if evidence.get("expected_candidate_sha256") != sha256(expected_path):
        errors.append("QuantLib evidence expected candidate hash mismatch")

    artifacts = evidence.get("artifacts")
    if not isinstance(artifacts, dict):
        errors.append("QuantLib evidence artifacts are missing")
        return errors
    required_artifacts = {
        "source_archive", "version_header", "compiler_binary", "cmake_binary",
        "toolchain_lock", "os_environment_manifest", "cmake_cache",
        "installed_library", "oracle_source", "oracle_input", "compile_identity",
        "oracle_program", "output", "aggregate_manifest", "build_manifest",
    }
    if "ninja_binary" in artifacts:
        required_artifacts.add("ninja_binary")
    if set(artifacts) != required_artifacts:
        errors.append("QuantLib evidence artifact keys mismatch")
        return errors
    work_root = evidence_path.parent
    mutable_artifacts = {
        "source_archive", "version_header", "os_environment_manifest", "cmake_cache",
        "installed_library", "compile_identity", "oracle_program", "output",
        "aggregate_manifest", "build_manifest",
    }
    for label, reference in artifacts.items():
        errors.extend(artifact_errors(reference, f"evidence.{label}"))
        if label in mutable_artifacts and isinstance(reference, dict):
            value = reference.get("path")
            if isinstance(value, str):
                artifact_path = Path(value).resolve()
                if artifact_path == repository_root or repository_root in artifact_path.parents:
                    errors.append(f"QuantLib evidence {label} must be outside the repository")
                if artifact_path != work_root and work_root not in artifact_path.parents:
                    errors.append(f"QuantLib evidence {label} must remain below evidence workdir")
    if errors:
        return errors

    manifest_path = Path(artifacts["build_manifest"]["path"])
    build_errors, manifest = validate_build_manifest(
        manifest_path,
        expected_path=expected_path,
        source_archive=Path(artifacts["source_archive"]["path"]),
        oracle_program=Path(artifacts["oracle_program"]["path"]),
        output_path=Path(artifacts["output"]["path"]),
    )
    errors.extend(build_errors)
    if manifest is None or build_errors:
        return errors
    if artifacts != manifest_artifacts(manifest, manifest_path):
        errors.append("QuantLib evidence artifact identities differ from build manifest")
        return errors
    expected = load_json(expected_path, "expected candidate", errors)
    output = load_json(Path(artifacts["output"]["path"]), "QuantLib output", errors)
    if expected is not None:
        frozen_metadata = {
            "candidate_id": expected.get("candidate_id"),
            "convention": expected.get("convention"),
            "calendar": expected.get("calendar"),
            "market_timezone": expected.get("market_timezone"),
            "valuation_at": expected.get("valuation_at"),
            "settlement_date": expected.get("settlement_date"),
            "acceptance_ids": expected.get("acceptance_ids"),
        }
        if evidence.get("frozen_metadata") != frozen_metadata:
            errors.append("QuantLib evidence frozen metadata mismatch")
    if expected is not None and output is not None:
        errors.extend(verify_output(output, expected))
    return errors


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-archive", type=Path, required=True)
    parser.add_argument("--build-manifest", type=Path, required=True)
    parser.add_argument("--oracle-program", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    args = parser.parse_args(argv)

    build_errors, manifest = validate_build_manifest(
        args.build_manifest,
        source_archive=args.source_archive,
        oracle_program=args.oracle_program,
        output_path=args.output,
    )
    if build_errors:
        raise SystemExit("build manifest verification failed: " + "; ".join(build_errors))
    expected = json.loads(EXPECTED.read_text(encoding="utf-8"))
    output = json.loads(args.output.read_text(encoding="utf-8"))
    output_errors = verify_output(output, expected)
    if output_errors:
        raise SystemExit("; ".join(output_errors))

    evidence = {
        "schema": EVIDENCE_SCHEMA,
        "quantlib_version": QUANTLIB_VERSION,
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
        "artifacts": manifest_artifacts(manifest, args.build_manifest),
    }
    args.evidence.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    evidence_errors = verify_evidence_bundle(args.evidence)
    if evidence_errors:
        args.evidence.unlink(missing_ok=True)
        raise SystemExit("written evidence failed re-verification: " + "; ".join(evidence_errors))
    print("official QuantLib 1.42.1 agreement verified for 12 cases")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
