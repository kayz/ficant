#!/usr/bin/env python3

"""Validate resolved Compose security policy or live container inspection JSON."""

from __future__ import annotations

import argparse
import json
import posixpath
import sys
from typing import Any
from urllib.parse import urlsplit


RUST_SERVICES = {"ficant-server", "ficant-worker"}
PERSISTENCE_SERVICES = {"postgres", "ceph-rgw"}
INIT_SERVICES = {"migration"}
EXPECTED_SERVICES = RUST_SERVICES | PERSISTENCE_SERVICES | INIT_SERVICES
PUBLISHED_SERVICES = RUST_SERVICES | PERSISTENCE_SERVICES
EXPECTED_IMAGES = {
    "postgres": "postgres@sha256:38471f330eb885e04de130b768d6db4e10469e2311879c7e5c699f6d2d8a1c74",
    "migration": "postgres@sha256:38471f330eb885e04de130b768d6db4e10469e2311879c7e5c699f6d2d8a1c74",
}
CEPH_BASE_IMAGE = "quay.io/ceph/ceph@sha256:6b4b5ae33acd3d736eb26d2a19238bce71a22f9cfb99cca887ba6312d0957644"
CEPH_RUNTIME_IMAGE = "ficant/ceph-rgw-runtime:dev"
CEPH_RUNTIME_DOCKERFILE = "deploy/dev/Ceph.Dockerfile"
CEPH_RUNTIME_USER = "167:167"
CEPH_LICENSE = "LGPL-2.1-only OR LGPL-3.0-only"
EXPECTED_DEPENDENCIES = {
    "postgres": {},
    "ceph-rgw": {},
    "migration": {"postgres": "service_healthy"},
    "ficant-server": {
        "migration": "service_completed_successfully",
        "ceph-rgw": "service_healthy",
    },
    "ficant-worker": {
        "migration": "service_completed_successfully",
        "ceph-rgw": "service_healthy",
        "ficant-server": "service_healthy",
    },
}
CONFIG_TARGET = "/etc/ficant/ficant.toml"
SERVER_BIND = "0.0.0.0:8080"
BINARY_HEALTHCHECK = ["CMD", "/usr/local/bin/ficant", "--health-check"]
SERVER_REQUIRED_ENV = {
    "FICANT_PLATFORM_SIGNING_KEY_HEX": "signing key injection",
    "FICANT_PLATFORM_TRACE_KEY_HEX": "trace key injection",
}
BOOTSTRAP_IDENTITY_ENV = {
    "FICANT_BOOTSTRAP_SUBJECT",
    "FICANT_BOOTSTRAP_BEARER_TOKEN",
    "FICANT_BOOTSTRAP_SCOPES",
}
LOOPBACK_IDENTITY_ENV = {
    "FICANT_LOOPBACK_SUBJECT",
    "FICANT_LOOPBACK_SCOPES",
}
ADJACENT_WEB_ORIGIN = "http://127.0.0.1:5173"


def record(condition: bool, message: str, failures: list[str]) -> None:
    if not condition:
        failures.append(message)


def is_non_root(user: object) -> bool:
    value = str(user or "").strip()
    uid = value.split(":", maxsplit=1)[0]
    if not uid:
        return False
    try:
        return int(uid, 10) != 0
    except ValueError:
        return False


def positive_number(value: object) -> bool:
    try:
        return float(value) > 0
    except (TypeError, ValueError):
        return False


def is_exact_origin(value: object) -> bool:
    if not isinstance(value, str) or not value or "," in value or "*" in value:
        return False
    parsed = urlsplit(value)
    return (
        parsed.scheme in {"http", "https"}
        and bool(parsed.hostname)
        and parsed.username is None
        and parsed.password is None
        and parsed.path == ""
        and parsed.query == ""
        and parsed.fragment == ""
        and value == f"{parsed.scheme}://{parsed.netloc}"
    )


def is_exact_loopback_http_origin_with_port(value: object) -> bool:
    if not is_exact_origin(value):
        return False
    parsed = urlsplit(value)
    try:
        port = parsed.port
    except ValueError:
        return False
    return (
        parsed.scheme == "http"
        and parsed.hostname == "127.0.0.1"
        and port is not None
        and 1 <= port <= 65535
        and value == f"http://127.0.0.1:{port}"
    )


def is_exact_dev_origin_allowlist(value: object) -> bool:
    if not isinstance(value, str) or not value:
        return False
    origins = value.split(",")
    return (
        len(origins) == 2
        and len(origins) == len(set(origins))
        and is_exact_loopback_http_origin_with_port(origins[0])
        and origins[1] == ADJACENT_WEB_ORIGIN
    )


def configured(environment: dict[str, Any], key: str) -> bool:
    value = environment.get(key)
    return isinstance(value, str) and bool(value)


def dependency_conditions(value: object) -> dict[str, str]:
    if not isinstance(value, dict):
        return {}
    return {
        name: details.get("condition", "")
        for name, details in value.items()
        if isinstance(name, str) and isinstance(details, dict)
    }


def command_text(value: object) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return "\n".join(str(item) for item in value)
    return ""


def tmpfs_targets(value: object) -> set[str]:
    if isinstance(value, list):
        return {
            item.split(":", maxsplit=1)[0]
            for item in value
            if isinstance(item, str)
        }
    if isinstance(value, dict):
        return {str(item) for item in value}
    return set()


def is_tmp_config_directory(value: object, tmpfs: object) -> bool:
    if not isinstance(value, str) or not value.startswith("/tmp/"):
        return False
    return posixpath.normpath(value) == value and "/tmp" in tmpfs_targets(tmpfs)


def environment_map(entries: object) -> dict[str, str]:
    if not isinstance(entries, list):
        return {}
    environment: dict[str, str] = {}
    for entry in entries:
        if isinstance(entry, str) and "=" in entry:
            key, value = entry.split("=", maxsplit=1)
            environment[key] = value
    return environment


def validate_server_environment(service: dict[str, Any], failures: list[str]) -> None:
    environment = service.get("environment")
    record(isinstance(environment, dict), "ficant-server: environment must be an object", failures)
    if not isinstance(environment, dict):
        return

    record(
        environment.get("FICANT_GRPC_BIND") == SERVER_BIND,
        f"ficant-server: public listener must be {SERVER_BIND}",
        failures,
    )
    record(
        is_exact_dev_origin_allowlist(
            environment.get("FICANT_GRPC_WEB_ALLOWED_ORIGINS")
        ),
        "ficant-server: exact CORS origin allowlist is required",
        failures,
    )
    for key, description in SERVER_REQUIRED_ENV.items():
        record(
            configured(environment, key),
            f"ficant-server: {description} is required via environment",
            failures,
        )

    for key in BOOTSTRAP_IDENTITY_ENV:
        record(
            environment.get(key) is None or configured(environment, key),
            f"ficant-server: {key} must be omitted or non-empty",
            failures,
        )

    bootstrap_subject = configured(environment, "FICANT_BOOTSTRAP_SUBJECT")
    bootstrap_token = configured(environment, "FICANT_BOOTSTRAP_BEARER_TOKEN")
    bootstrap_scopes = configured(environment, "FICANT_BOOTSTRAP_SCOPES")
    record(
        bootstrap_subject == bootstrap_token
        and (not bootstrap_scopes or (bootstrap_subject and bootstrap_token)),
        "ficant-server: unsafe bootstrap identity configuration",
        failures,
    )
    record(
        not any(configured(environment, key) for key in LOOPBACK_IDENTITY_ENV),
        "ficant-server: unsafe loopback identity configuration on public listener",
        failures,
    )


def validate_resolved(document: dict[str, Any], project: str) -> list[str]:
    failures: list[str] = []
    record(document.get("name") == project, f"resolved project name must be {project}", failures)
    services = document.get("services")
    record(isinstance(services, dict), "resolved services must be an object", failures)
    if not isinstance(services, dict):
        return failures
    record(
        set(services) == EXPECTED_SERVICES,
        f"resolved services must be {sorted(EXPECTED_SERVICES)}",
        failures,
    )

    named_volumes = document.get("volumes")
    record(isinstance(named_volumes, dict), "resolved named volumes must be an object", failures)
    if isinstance(named_volumes, dict):
        record("postgres-data" in named_volumes, "postgres-data named volume is required", failures)
        record("ceph-data" in named_volumes, "ceph-data named volume is required", failures)

    for service_name in sorted(EXPECTED_SERVICES):
        service = services.get(service_name)
        record(isinstance(service, dict), f"{service_name}: missing resolved service", failures)
        if not isinstance(service, dict):
            continue

        ports = service.get("ports")
        if service_name in PUBLISHED_SERVICES:
            record(isinstance(ports, list) and bool(ports), f"{service_name}: missing published port", failures)
        else:
            record(ports in (None, []), f"{service_name}: init service must not publish ports", failures)
        if isinstance(ports, list):
            for port in ports:
                record(
                    isinstance(port, dict) and port.get("host_ip") == "127.0.0.1",
                    f"{service_name}: every published port must bind 127.0.0.1",
                    failures,
                )

        record(is_non_root(service.get("user")), f"{service_name}: user must be non-root", failures)
        record(service.get("read_only") is True, f"{service_name}: root filesystem must be read-only", failures)
        cap_drop = service.get("cap_drop")
        record(
            isinstance(cap_drop, list) and "ALL" in cap_drop,
            f"{service_name}: cap_drop must contain ALL",
            failures,
        )
        record(
            service.get("cap_add") in (None, []),
            f"{service_name}: cap_add must be empty",
            failures,
        )
        security_opt = service.get("security_opt")
        record(
            isinstance(security_opt, list) and "no-new-privileges:true" in security_opt,
            f"{service_name}: no-new-privileges:true is required",
            failures,
        )
        record(positive_number(service.get("cpus")), f"{service_name}: positive CPU limit is required", failures)
        record(
            positive_number(service.get("mem_limit")),
            f"{service_name}: positive memory limit is required",
            failures,
        )
        record(
            positive_number(service.get("pids_limit")),
            f"{service_name}: positive PID limit is required",
            failures,
        )
        tmpfs = service.get("tmpfs")
        record(isinstance(tmpfs, list) and bool(tmpfs), f"{service_name}: tmpfs is required", failures)
        record(
            dependency_conditions(service.get("depends_on")) == EXPECTED_DEPENDENCIES[service_name],
            f"{service_name}: dependency conditions must match the frozen runtime DAG",
            failures,
        )

        if service_name in EXPECTED_IMAGES:
            record(
                service.get("image") == EXPECTED_IMAGES[service_name],
                f"{service_name}: image must match the locked RepoDigest",
                failures,
            )
        elif service_name == "ceph-rgw":
            build = service.get("build")
            record(
                service.get("image") == CEPH_RUNTIME_IMAGE,
                "ceph-rgw: runtime image must match the frozen local image name",
                failures,
            )
            record(isinstance(build, dict), "ceph-rgw: hardened image build is required", failures)
            if isinstance(build, dict):
                context = build.get("context")
                record(
                    isinstance(context, str) and bool(context) and "://" not in context,
                    "ceph-rgw: build context must be local",
                    failures,
                )
                record(
                    build.get("dockerfile") == CEPH_RUNTIME_DOCKERFILE,
                    "ceph-rgw: Dockerfile must match the hardened runtime contract",
                    failures,
                )
                args = build.get("args")
                record(
                    isinstance(args, dict) and args.get("CEPH_IMAGE") == CEPH_BASE_IMAGE,
                    "ceph-rgw: build must use the locked base RepoDigest",
                    failures,
                )
            record(
                service.get("user") == CEPH_RUNTIME_USER,
                "ceph-rgw: runtime user must be exactly 167:167",
                failures,
            )

        healthcheck = service.get("healthcheck")
        if service_name in RUST_SERVICES:
            record(
                isinstance(healthcheck, dict) and healthcheck.get("test") == BINARY_HEALTHCHECK,
                f"{service_name}: healthcheck must use the binary readiness probe",
                failures,
            )
        elif service_name in PERSISTENCE_SERVICES:
            record(isinstance(healthcheck, dict) and bool(healthcheck.get("test")), f"{service_name}: healthcheck is required", failures)
        else:
            record(healthcheck is None, f"{service_name}: one-shot init must not declare healthcheck", failures)
            record(service.get("restart") == "no", f"{service_name}: one-shot init restart must be no", failures)

        volumes = service.get("volumes")
        if service_name in RUST_SERVICES:
            config_mounts = [
                volume
                for volume in volumes or []
                if isinstance(volume, dict) and volume.get("target") == CONFIG_TARGET
            ]
            record(len(config_mounts) == 1, f"{service_name}: exactly one config mount is required", failures)
            if len(config_mounts) == 1:
                record(
                    config_mounts[0].get("read_only") is True,
                    f"{service_name}: config mount must be read-only",
                    failures,
                )
        elif service_name == "postgres":
            record(
                any(isinstance(volume, dict) and volume.get("target") == "/var/lib/postgresql/data" for volume in volumes or []),
                "postgres: persistent data volume is required",
                failures,
            )
        elif service_name == "ceph-rgw":
            record(
                any(isinstance(volume, dict) and volume.get("target") == "/var/lib/ceph" for volume in volumes or []),
                "ceph-rgw: persistent data volume is required",
                failures,
            )
        elif service_name == "migration":
            migration_mounts = [
                volume for volume in volumes or []
                if isinstance(volume, dict) and volume.get("target") == "/migrations"
            ]
            record(len(migration_mounts) == 1 and migration_mounts[0].get("read_only") is True, "migration: read-only migration source is required", failures)

        environment = service.get("environment")
        if service_name in {"postgres", "migration"}:
            record(isinstance(environment, dict) and configured(environment, "PGPASSWORD" if service_name == "migration" else "POSTGRES_PASSWORD"), f"{service_name}: injected PostgreSQL credential is required", failures)
        elif service_name == "ceph-rgw":
            record(
                isinstance(environment, dict)
                and configured(environment, "FICANT_S3_ACCESS_KEY")
                and configured(environment, "FICANT_S3_SECRET_KEY")
                and configured(environment, "FICANT_S3_BUCKET"),
                "ceph-rgw: injected S3 credentials and bucket are required",
                failures,
            )

        if service_name == "migration":
            migration_command = command_text(service.get("command"))
            for marker in ("ficant_schema_migrations", "ON_ERROR_STOP=1", "BEGIN;", "COMMIT;"):
                record(marker in migration_command, f"migration: idempotent migration command must contain {marker}", failures)

        if service_name == "ficant-server":
            validate_server_environment(service, failures)

    return failures


def validate_runtime(document: list[dict[str, Any]], project: str) -> list[str]:
    failures: list[str] = []
    record(len(document) == len(EXPECTED_SERVICES), "runtime inspection must contain exactly five containers", failures)
    inspected_services: set[str] = set()

    for container in document:
        config = container.get("Config") or {}
        host_config = container.get("HostConfig") or {}
        labels = config.get("Labels") or {}
        service_name = labels.get("com.docker.compose.service", "<unknown>")
        inspected_services.add(service_name)

        record(
            labels.get("com.docker.compose.project") == project,
            f"{service_name}: runtime project label must be {project}",
            failures,
        )
        record(is_non_root(config.get("User")), f"{service_name}: runtime user must be non-root", failures)
        state = container.get("State") or {}
        if service_name in INIT_SERVICES:
            record(state.get("Status") == "exited" and state.get("ExitCode") == 0, f"{service_name}: runtime init must have exited successfully", failures)
        else:
            healthcheck = config.get("Healthcheck")
            if service_name in RUST_SERVICES:
                record(
                    isinstance(healthcheck, dict)
                    and healthcheck.get("Test") == BINARY_HEALTHCHECK,
                    f"{service_name}: runtime healthcheck must use the binary readiness probe",
                    failures,
                )
            health = state.get("Health") or {}
            record(
                health.get("Status") == "healthy",
                f"{service_name}: runtime health must be healthy",
                failures,
            )
        record(
            host_config.get("ReadonlyRootfs") is True,
            f"{service_name}: runtime root filesystem must be read-only",
            failures,
        )
        cap_drop = host_config.get("CapDrop")
        record(
            isinstance(cap_drop, list) and "ALL" in cap_drop,
            f"{service_name}: runtime CapDrop must contain ALL",
            failures,
        )
        record(
            host_config.get("CapAdd") in (None, []),
            f"{service_name}: runtime CapAdd must be empty",
            failures,
        )
        security_opt = host_config.get("SecurityOpt")
        record(
            isinstance(security_opt, list) and "no-new-privileges:true" in security_opt,
            f"{service_name}: runtime no-new-privileges:true is required",
            failures,
        )
        record(positive_number(host_config.get("NanoCpus")), f"{service_name}: runtime CPU limit is required", failures)
        record(positive_number(host_config.get("Memory")), f"{service_name}: runtime memory limit is required", failures)
        record(positive_number(host_config.get("PidsLimit")), f"{service_name}: runtime PID limit is required", failures)
        record(bool(host_config.get("Tmpfs")), f"{service_name}: runtime tmpfs is required", failures)

        if service_name in EXPECTED_IMAGES:
            record(config.get("Image") == EXPECTED_IMAGES[service_name], f"{service_name}: runtime image must match the locked RepoDigest", failures)
        elif service_name == "ceph-rgw":
            record(
                config.get("Image") == CEPH_RUNTIME_IMAGE,
                "ceph-rgw: runtime image must match the frozen local image name",
                failures,
            )
            record(
                config.get("User") == CEPH_RUNTIME_USER,
                "ceph-rgw: runtime user must be exactly 167:167",
                failures,
            )
            record(
                labels.get("org.opencontainers.image.base.name") == CEPH_BASE_IMAGE,
                "ceph-rgw: runtime base image provenance must match the locked RepoDigest",
                failures,
            )
            record(
                labels.get("org.opencontainers.image.licenses") == CEPH_LICENSE,
                "ceph-rgw: runtime license label must match the frozen Ceph dual-license expression",
                failures,
            )

        mounts = container.get("Mounts") or []
        if service_name in RUST_SERVICES:
            config_mounts = [mount for mount in mounts if mount.get("Destination") == CONFIG_TARGET]
            record(len(config_mounts) == 1, f"{service_name}: runtime config mount is required", failures)
            if len(config_mounts) == 1:
                record(config_mounts[0].get("RW") is False, f"{service_name}: runtime config mount must be read-only", failures)
        elif service_name == "postgres":
            record(any(mount.get("Destination") == "/var/lib/postgresql/data" and mount.get("Type") == "volume" for mount in mounts), "postgres: runtime persistent data volume is required", failures)
        elif service_name == "ceph-rgw":
            record(any(mount.get("Destination") == "/var/lib/ceph" and mount.get("Type") == "volume" for mount in mounts), "ceph-rgw: runtime persistent data volume is required", failures)
        elif service_name == "migration":
            record(any(mount.get("Destination") == "/migrations" and mount.get("RW") is False for mount in mounts), "migration: runtime read-only migration source is required", failures)

        port_bindings = host_config.get("PortBindings") or {}
        bindings = [binding for values in port_bindings.values() for binding in values or []]
        if service_name in PUBLISHED_SERVICES:
            record(bool(bindings), f"{service_name}: runtime published port is required", failures)
        else:
            record(not bindings, f"{service_name}: runtime init must not publish ports", failures)
        for binding in bindings:
            record(
                binding.get("HostIp") == "127.0.0.1",
                f"{service_name}: runtime published ports must bind 127.0.0.1",
                failures,
            )

        if service_name == "ficant-server":
            validate_server_environment(
                {"environment": environment_map(config.get("Env"))},
                failures,
            )

    record(
        inspected_services == EXPECTED_SERVICES,
        f"runtime services must be {sorted(EXPECTED_SERVICES)}",
        failures,
    )
    return failures


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("resolved", "runtime"))
    parser.add_argument("--project", required=True)
    arguments = parser.parse_args()

    try:
        document = json.load(sys.stdin)
    except (json.JSONDecodeError, OSError) as error:
        print(f"compose-security ({arguments.mode}): invalid JSON: {error}", file=sys.stderr)
        return 2

    if arguments.mode == "resolved":
        if not isinstance(document, dict):
            print("compose-security (resolved): expected a JSON object", file=sys.stderr)
            return 2
        failures = validate_resolved(document, arguments.project)
    else:
        if not isinstance(document, list):
            print("compose-security (runtime): expected a JSON array", file=sys.stderr)
            return 2
        failures = validate_runtime(document, arguments.project)

    if failures:
        print(f"compose-security ({arguments.mode}): FAIL ({len(failures)} violation(s))", file=sys.stderr)
        for failure in failures:
            print(f" - {failure}", file=sys.stderr)
        return 1

    print(f"compose-security ({arguments.mode}): PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
