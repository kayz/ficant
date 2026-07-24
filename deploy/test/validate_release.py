#!/usr/bin/env python3
"""Fail-closed validation for the resolved ficant test Compose model."""

from __future__ import annotations

import json
import pathlib
import sys


EXPECTED_SERVICES = {
    "postgres",
    "ceph-rgw",
    "migration",
    "ficant-server",
    "ficant-worker",
    "ficant-web",
    "ficant-ui",
}
APP_SERVICES = {"ficant-server", "ficant-worker", "ficant-web", "ficant-ui"}
CEPH_SERVICE = "ceph-rgw"
POSTGRES_IMAGE = (
    "postgres@sha256:"
    "38471f330eb885e04de130b768d6db4e10469e2311879c7e5c699f6d2d8a1c74"
)


def fail(message: str) -> None:
    raise SystemExit(f"release-compose: {message}")


def main() -> None:
    model = json.load(sys.stdin)
    services = model.get("services")
    if not isinstance(services, dict) or set(services) != EXPECTED_SERVICES:
        fail(f"unexpected services: {sorted(services or {})}")

    for name, service in services.items():
        if "build" in service:
            fail(f"{name} must pull an immutable image, not build on the server")
        if service.get("privileged"):
            fail(f"{name} cannot be privileged")
        if service.get("read_only") is not True:
            fail(f"{name} must use a read-only root filesystem")
        if service.get("init") is not True:
            fail(f"{name} must use an init process")
        if "ALL" not in service.get("cap_drop", []):
            fail(f"{name} must drop all capabilities")
        if service.get("cap_add"):
            fail(f"{name} cannot add capabilities")
        if "no-new-privileges:true" not in service.get("security_opt", []):
            fail(f"{name} must set no-new-privileges")
        user = str(service.get("user", ""))
        if not user or user.split(":", 1)[0] in {"0", "root"}:
            fail(f"{name} must use an explicit non-root user")
        for port in service.get("ports", []):
            if port.get("host_ip") != "127.0.0.1":
                fail(f"{name} publishes a non-loopback port")

    for name in ("postgres", "migration"):
        if services[name].get("image") != POSTGRES_IMAGE:
            fail(f"{name} must use the pinned PostgreSQL image")

    for name in APP_SERVICES:
        image = services[name].get("image", "")
        suffix = f"-{name.removeprefix('ficant-')}:sha-" + "0" * 40
        if not image.startswith("ghcr.io/kayz/ficant") or not image.endswith(suffix):
            fail(f"{name} does not resolve to the expected immutable GHCR tag: {image}")

    ceph_image = services[CEPH_SERVICE].get("image", "")
    if not ceph_image.startswith("ghcr.io/kayz/ficant") or not ceph_image.endswith(
        "-ceph-rgw:sha-" + "0" * 40
    ):
        fail(f"ceph-rgw does not resolve to the expected immutable GHCR tag: {ceph_image}")
    if str(services[CEPH_SERVICE].get("user")) != "167:167":
        fail("ceph-rgw must run as 167:167")

    worker = services["ficant-worker"]
    worker_environment = worker.get("environment", {})
    required_worker_environment = {
        "FICANT_WORKER_DATABASE_URL",
        "FICANT_WORKER_S3_ENDPOINT",
        "FICANT_WORKER_S3_BUCKET",
        "FICANT_WORKER_S3_ACCESS_KEY",
        "FICANT_WORKER_S3_SECRET_KEY",
        "FICANT_WORKER_ID",
        "FICANT_WORKER_RUNTIME_IMAGE_DIGEST",
        "FICANT_WORKER_ENVIRONMENT_ATTESTATION",
        "FICANT_WORKER_NATIVE_SOURCE_DIGEST",
    }
    if not required_worker_environment.issubset(worker_environment):
        fail("ficant-worker is missing its production database/S3/identity environment")
    if worker_environment["FICANT_WORKER_S3_ENDPOINT"] != "http://ceph-rgw:9000":
        fail("ficant-worker must use the managed ceph-rgw endpoint")
    worker_dependencies = worker.get("depends_on", {})
    if worker_dependencies.get("ceph-rgw", {}).get("condition") != "service_healthy":
        fail("ficant-worker must wait for healthy ceph-rgw")

    server_environment = services["ficant-server"].get("environment", {})
    required_server_environment = {
        "FICANT_BOOTSTRAP_BEARER_TOKEN",
        "FICANT_EXPERIMENT_DATABASE_URL",
        "FICANT_EXPERIMENT_S3_ENDPOINT",
        "FICANT_EXPERIMENT_S3_BUCKET",
        "FICANT_EXPERIMENT_S3_ACCESS_KEY",
        "FICANT_EXPERIMENT_S3_SECRET_KEY",
        "FICANT_EXPERIMENT_CURSOR_KEY_HEX",
        "FICANT_EXPERIMENT_TENANT_ID",
        "FICANT_EXPERIMENT_OWNER_ID",
        "FICANT_EXPERIMENT_ACTOR_ID",
        "FICANT_EXPERIMENT_RUNTIME_IMAGE_DIGEST",
        "FICANT_EXPERIMENT_ENVIRONMENT_ATTESTATION",
        "FICANT_EXPERIMENT_NATIVE_SOURCE_DIGEST",
    }
    if not required_server_environment.issubset(server_environment):
        fail("ficant-server is missing its authenticated experiment environment")
    if (
        server_environment["FICANT_EXPERIMENT_RUNTIME_IMAGE_DIGEST"]
        != worker_environment["FICANT_WORKER_RUNTIME_IMAGE_DIGEST"]
        or server_environment["FICANT_EXPERIMENT_NATIVE_SOURCE_DIGEST"]
        != worker_environment["FICANT_WORKER_NATIVE_SOURCE_DIGEST"]
        or server_environment["FICANT_EXPERIMENT_ENVIRONMENT_ATTESTATION"]
        != worker_environment["FICANT_WORKER_ENVIRONMENT_ATTESTATION"]
    ):
        fail("server and worker execution identities must match exactly")

    expected_ui_healthcheck = [
        "CMD",
        "wget",
        "--quiet",
        "--spider",
        "http://127.0.0.1:8080/health",
    ]
    if services["ficant-ui"].get("healthcheck", {}).get("test") != expected_ui_healthcheck:
        fail("ficant-ui must use the executable BusyBox wget readiness probe")

    serialized = json.dumps(model, sort_keys=True).lower()
    if "minio" in serialized or "latest" in serialized or '"network_mode": "host"' in serialized:
        fail("forbidden MinIO, latest tag, or host network found")

    root = pathlib.PurePosixPath("/srv/ficant-test")
    for service in services.values():
        for volume in service.get("volumes", []):
            if volume.get("type") != "bind":
                continue
            source = pathlib.PurePosixPath(volume.get("source", "/"))
            if source != root and root not in source.parents:
                fail(f"bind mount escapes managed root: {source}")

    print("release-compose: PASS")


if __name__ == "__main__":
    main()
