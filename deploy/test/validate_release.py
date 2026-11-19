#!/usr/bin/env python3
"""Fail-closed validation for the resolved ficant single-server test Compose model."""

from __future__ import annotations

import json
import os
import re
import sys

EXPECTED_SERVICES = {"ficant-server"}
SHA_PATTERN = re.compile(r"[0-9a-f]{40}")


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

    server = services["ficant-server"]
    if "build" in server:
        fail("ficant-server must pull an immutable image, not build on the server")
    if server.get("privileged"):
        fail("ficant-server cannot be privileged")
    if server.get("read_only") is not True:
        fail("ficant-server must use a read-only root filesystem")
    if server.get("init") is not True:
        fail("ficant-server must use an init process")
    if "ALL" not in server.get("cap_drop", []):
        fail("ficant-server must drop all capabilities")
    if server.get("cap_add"):
        fail("ficant-server cannot add capabilities")
    if "no-new-privileges:true" not in server.get("security_opt", []):
        fail("ficant-server must set no-new-privileges")
    user = str(server.get("user", ""))
    if not user or user.split(":", 1)[0] in {"0", "root"}:
        fail("ficant-server must use an explicit non-root user")
    for port in server.get("ports", []):
        if port.get("host_ip") != "127.0.0.1":
            fail("ficant-server publishes a non-loopback port")

    image = server.get("image", "")
    expected_image = f"ghcr.io/kayz/ficant-server:sha-{deploy_sha}"
    if image != expected_image:
        fail(f"ficant-server does not resolve to the expected immutable GHCR tag: {image}")

    serialized = json.dumps(model, sort_keys=True).lower()
    if "latest" in serialized or '"network_mode": "host"' in serialized:
        fail("forbidden latest tag or host network found")

    print("release-compose: PASS")


if __name__ == "__main__":
    main()
