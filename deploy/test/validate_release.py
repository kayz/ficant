#!/usr/bin/env python3
"""Fail-closed validation for the resolved ficant test Compose model."""

from __future__ import annotations

import json
import os
import pathlib
import re
import sys


EXPECTED_SERVICES = {
    "postgres",
    "ceph-rgw",
    "migration",
    "ficant-server",
    "ficant-worker",
    "ficant-ui",
}
APP_SERVICES = {"ficant-server", "ficant-worker", "ficant-ui"}
CEPH_SERVICE = "ceph-rgw"
SHA_PATTERN = re.compile(r"[0-9a-f]{40}")
DIGEST_PATTERN = re.compile(r"sha256:[0-9a-f]{64}")
SERVER_ENVIRONMENT_DIGEST = (
    "sha256:5610d256cb433afb90595e430f00ff53953dd199d0fed5826498fc8e87870734"
)
WORKER_ENVIRONMENT_ATTESTATION = (
    "ficant.worker.environment.v1\narch=amd64\nos=linux\nprofile=test"
)
POSTGRES_IMAGE = (
    "postgres@sha256:"
    "38471f330eb885e04de130b768d6db4e10469e2311879c7e5c699f6d2d8a1c74"
)
STORAGE_LOCK = pathlib.Path(__file__).resolve().parents[1] / "storage-runtime.lock.json"


def fail(message: str) -> None:
    raise SystemExit(f"release-compose: {message}")


def main() -> None:
    model = json.load(sys.stdin)
    services = model.get("services")
    if not isinstance(services, dict) or set(services) != EXPECTED_SERVICES:
        fail(f"unexpected services: {sorted(services or {})}")

    deploy_sha = os.environ.get("FICANT_DEPLOY_SHA", "")
    if SHA_PATTERN.fullmatch(deploy_sha) is None:
        fail("FICANT_DEPLOY_SHA must be one 40-character lowercase SHA")
    code_tree_sha = os.environ.get("FICANT_CODE_TREE_SHA", "")
    if SHA_PATTERN.fullmatch(code_tree_sha) is None:
        fail("FICANT_CODE_TREE_SHA must be one 40-character lowercase SHA")
    server_runtime_digest = os.environ.get("FICANT_SERVER_RUNTIME_IMAGE_DIGEST", "")
    if DIGEST_PATTERN.fullmatch(server_runtime_digest) is None:
        fail("FICANT_SERVER_RUNTIME_IMAGE_DIGEST must be one canonical SHA-256 digest")
    worker_runtime_digest = os.environ.get("FICANT_WORKER_RUNTIME_IMAGE_DIGEST", "")
    if DIGEST_PATTERN.fullmatch(worker_runtime_digest) is None:
        fail("FICANT_WORKER_RUNTIME_IMAGE_DIGEST must be one canonical SHA-256 digest")
    worker_source_digest = os.environ.get("FICANT_WORKER_NATIVE_SOURCE_DIGEST", "")
    if DIGEST_PATTERN.fullmatch(worker_source_digest) is None:
        fail("FICANT_WORKER_NATIVE_SOURCE_DIGEST must be one canonical SHA-256 digest")

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
        expected_image = (
            f"ghcr.io/kayz/ficant-{name.removeprefix('ficant-')}:sha-{deploy_sha}"
        )
        if image != expected_image:
            fail(f"{name} does not resolve to the expected immutable GHCR tag: {image}")

    storage_lock = json.loads(STORAGE_LOCK.read_text(encoding="utf-8"))
    expected_ceph_image = storage_lock["image"] + "@" + storage_lock["oci"]["index_digest"]
    ceph_image = services[CEPH_SERVICE].get("image", "")
    if ceph_image != expected_ceph_image:
        fail(f"ceph-rgw does not resolve to the locked immutable OCI index: {ceph_image}")
    if str(services[CEPH_SERVICE].get("user")) != "167:167":
        fail("ceph-rgw must run as 167:167")

    worker = services["ficant-worker"]
    worker_environment = worker.get("environment", {})
    required_worker_environment = {
        "FICANT_CODE_COMMIT_SHA",
        "FICANT_CODE_TREE_SHA",
        "FICANT_WORKER_DATABASE_URL",
        "FICANT_WORKER_S3_ENDPOINT",
        "FICANT_WORKER_S3_BUCKET",
        "FICANT_WORKER_S3_ACCESS_KEY",
        "FICANT_WORKER_S3_SECRET_KEY",
        "FICANT_WORKER_ID",
        "FICANT_WORKER_RUNTIME_IMAGE_DIGEST",
        "FICANT_WORKER_ENVIRONMENT_ATTESTATION",
        "FICANT_WORKER_NATIVE_SOURCE_DIGEST",
        "FICANT_WORKER_ORPHAN_GRACE_SECONDS",
        "FICANT_WORKER_ORPHAN_INTERVAL_SECONDS",
    }
    if not required_worker_environment.issubset(worker_environment):
        fail("ficant-worker is missing its production database/S3/identity environment")
    if worker_environment["FICANT_WORKER_S3_ENDPOINT"] != "http://ceph-rgw:9000":
        fail("ficant-worker must use the managed ceph-rgw endpoint")
    if (
        worker_environment["FICANT_CODE_COMMIT_SHA"] != deploy_sha
        or worker_environment["FICANT_CODE_TREE_SHA"] != code_tree_sha
    ):
        fail("ficant-worker Code identity does not match the authorized candidate")
    if worker_environment["FICANT_WORKER_RUNTIME_IMAGE_DIGEST"] != worker_runtime_digest:
        fail("ficant-worker Runtime image does not match the inspected image")
    if worker_environment["FICANT_WORKER_NATIVE_SOURCE_DIGEST"] != worker_source_digest:
        fail("ficant-worker native source does not match the inspected image")
    if (
        worker_environment["FICANT_WORKER_ENVIRONMENT_ATTESTATION"]
        != WORKER_ENVIRONMENT_ATTESTATION
    ):
        fail("ficant-worker environment attestation is not the fixed test profile")
    if (
        str(worker_environment["FICANT_WORKER_ORPHAN_GRACE_SECONDS"]) != "3600"
        or str(worker_environment["FICANT_WORKER_ORPHAN_INTERVAL_SECONDS"]) != "300"
    ):
        fail("ficant-worker orphan maintenance intervals are not the test contract")
    worker_dependencies = worker.get("depends_on", {})
    if worker_dependencies.get("ceph-rgw", {}).get("condition") != "service_healthy":
        fail("ficant-worker must wait for healthy ceph-rgw")

    server_environment = services["ficant-server"].get("environment", {})
    required_server_environment = {
        "FICANT_CODE_COMMIT_SHA",
        "FICANT_CODE_TREE_SHA",
        "FICANT_SERVER_RUNTIME_IMAGE_DIGEST",
        "FICANT_SERVER_ENVIRONMENT_ATTESTATION",
        "FICANT_BOOTSTRAP_SUBJECT",
        "FICANT_BOOTSTRAP_BEARER_TOKEN",
        "FICANT_BOOTSTRAP_ACTOR_ID",
        "FICANT_BOOTSTRAP_TENANT_ID",
        "FICANT_BOOTSTRAP_ALLOWED_OWNER_IDS",
        "FICANT_BOOTSTRAP_ACTIVE_ROLE",
        "FICANT_BOOTSTRAP_SCOPES",
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
        "FICANT_INPUT_FILE_NDJSON_ROOT",
        "FICANT_INPUT_FILE_CONNECTION_BINDING",
        "FICANT_INPUT_POSTGRES_CONNECTION_BINDING",
    }
    if not required_server_environment.issubset(server_environment):
        fail("ficant-server is missing its production runtime environment")
    if (
        server_environment["FICANT_CODE_COMMIT_SHA"] != deploy_sha
        or server_environment["FICANT_CODE_TREE_SHA"] != code_tree_sha
    ):
        fail("ficant-server Code identity does not match the authorized candidate")
    if server_environment["FICANT_SERVER_RUNTIME_IMAGE_DIGEST"] != server_runtime_digest:
        fail("ficant-server Runtime image does not match the inspected image")
    if (
        server_environment["FICANT_SERVER_ENVIRONMENT_ATTESTATION"]
        != SERVER_ENVIRONMENT_DIGEST
    ):
        fail("ficant-server environment attestation is not the fixed test profile")
    expected_server_identity = {
        "FICANT_BOOTSTRAP_SUBJECT": "ficant-test-user",
        "FICANT_BOOTSTRAP_ACTOR_ID": "01J00000000000000000000012",
        "FICANT_BOOTSTRAP_TENANT_ID": "01J00000000000000000000010",
        "FICANT_BOOTSTRAP_ALLOWED_OWNER_IDS": "01J00000000000000000000011",
        "FICANT_BOOTSTRAP_ACTIVE_ROLE": "RESEARCHER",
        "FICANT_BOOTSTRAP_SCOPES": "apps:read,experiment:read,experiment:write",
        "FICANT_INPUT_FILE_NDJSON_ROOT": "/var/lib/ficant/input",
        "FICANT_INPUT_FILE_CONNECTION_BINDING": "test-file-ndjson",
        "FICANT_INPUT_POSTGRES_CONNECTION_BINDING": "test-postgres",
    }
    for name, expected in expected_server_identity.items():
        if server_environment[name] != expected:
            fail(f"ficant-server has an unexpected test runtime value for {name}")
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
