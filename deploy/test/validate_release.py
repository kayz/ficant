#!/usr/bin/env python3
"""Fail-closed validation for the resolved ficant test Compose model."""

from __future__ import annotations

import json
import pathlib
import sys


EXPECTED_SERVICES = {
    "postgres",
    "migration",
    "ficant-server",
    "ficant-worker",
    "ficant-web",
}
APP_SERVICES = {"ficant-server", "ficant-worker", "ficant-web"}
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

