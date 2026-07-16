from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import tomllib
import unittest
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from compose_security_gate import (  # noqa: E402
    EXPECTED_SERVICES,
    is_non_root,
    validate_resolved,
    validate_runtime,
)


PROJECT = "ficant-dev"

PERSISTENCE_SERVICES = {"postgres", "minio"}
INIT_SERVICES = {"minio-init", "migration"}
RUST_SERVICES = {"ficant-server", "ficant-worker", "ficant-web"}
POSTGRES_IMAGE = "postgres@sha256:38471f330eb885e04de130b768d6db4e10469e2311879c7e5c699f6d2d8a1c74"
MINIO_IMAGE = "minio/minio@sha256:a1ea29fa28355559ef137d71fc570e508a214ec84ff8083e39bc5428980b015e"
MC_IMAGE = "minio/mc@sha256:09f93f534cde415d192bb6084dd0e0ddd1715fb602f8a922ad121fd2bf0f8b44"
MINIO_RUNTIME_IMAGE = "ficant/minio-runtime:dev"
MINIO_RUNTIME_DOCKERFILE = "deploy/dev/Minio.Dockerfile"
MINIO_AMD64_MANIFEST = "sha256:3f97c5651cb6662b880c787a232b6b34fec8d8922e08d6617b25d241a21164bb"


def resolved_document() -> dict[str, object]:
    service = {
        "user": "1654:1654",
        "read_only": True,
        "cap_drop": ["ALL"],
        "security_opt": ["no-new-privileges:true"],
        "cpus": 0.5,
        "mem_limit": 128 * 1024 * 1024,
        "pids_limit": 64,
        "tmpfs": ["/tmp"],
    }
    document = {
        "name": PROJECT,
        "volumes": {"postgres-data": {}, "minio-data": {}},
        "services": {
            service_name: copy.deepcopy(service)
            for service_name in EXPECTED_SERVICES
        },
    }
    for service_name, value in document["services"].items():
        value["depends_on"] = {
            dependency: {"condition": condition}
            for dependency, condition in {
                "postgres": {},
                "minio": {},
                "minio-init": {"minio": "service_healthy"},
                "migration": {"postgres": "service_healthy"},
                "ficant-server": {
                    "migration": "service_completed_successfully",
                    "minio-init": "service_completed_successfully",
                },
                "ficant-worker": {
                    "migration": "service_completed_successfully",
                    "minio-init": "service_completed_successfully",
                    "ficant-server": "service_healthy",
                },
                "ficant-web": {
                    "migration": "service_completed_successfully",
                    "minio-init": "service_completed_successfully",
                    "ficant-server": "service_healthy",
                },
            }[service_name].items()
        }
        if service_name in PERSISTENCE_SERVICES | RUST_SERVICES:
            value["ports"] = [{"host_ip": "127.0.0.1"}]
        if service_name in RUST_SERVICES:
            value["volumes"] = [{"target": "/etc/ficant/ficant.toml", "read_only": True}]
            value["healthcheck"] = {"test": ["CMD", "/usr/local/bin/ficant", "--health-check"]}
        elif service_name in INIT_SERVICES:
            value["restart"] = "no"
        else:
            value["healthcheck"] = {"test": ["CMD", "true"]}
    document["services"]["postgres"].update({
        "image": POSTGRES_IMAGE,
        "environment": {"POSTGRES_PASSWORD": "fixture-only"},
        "volumes": [{"target": "/var/lib/postgresql/data", "read_only": False}],
    })
    document["services"]["migration"].update({
        "image": POSTGRES_IMAGE,
        "environment": {"PGPASSWORD": "fixture-only"},
        "volumes": [{"target": "/migrations", "read_only": True}],
        "command": "ON_ERROR_STOP=1 ficant_schema_migrations BEGIN; COMMIT;",
    })
    document["services"]["minio"].update({
        "image": MINIO_RUNTIME_IMAGE,
        "user": "1000:1000",
        "build": {
            "context": "../..",
            "dockerfile": MINIO_RUNTIME_DOCKERFILE,
            "args": {"MINIO_IMAGE": MINIO_IMAGE},
        },
        "environment": {"MINIO_ROOT_USER": "fixture-user", "MINIO_ROOT_PASSWORD": "fixture-only"},
        "volumes": [{"target": "/data", "read_only": False}],
    })
    document["services"]["minio-init"].update({
        "image": MC_IMAGE,
        "environment": {
            "MINIO_ROOT_USER": "fixture-user",
            "MINIO_ROOT_PASSWORD": "fixture-only",
            "MC_CONFIG_DIR": "/tmp/.mc",
        },
        "command": "mc alias set local http://minio:9000 user password; mc mb --ignore-existing local/ficant",
    })
    document["services"]["ficant-server"]["environment"] = {
        "FICANT_CONFIG": "/etc/ficant/ficant.toml",
        "FICANT_GRPC_BIND": "0.0.0.0:8080",
        "FICANT_GRPC_WEB_ALLOWED_ORIGINS": "http://127.0.0.1:18082",
        "FICANT_PLATFORM_SIGNING_KEY_HEX": "test-only-signing-key",
        "FICANT_PLATFORM_TRACE_KEY_HEX": "test-only-trace-key",
    }
    return document


def runtime_document() -> list[dict[str, object]]:
    containers = []
    for service_name in EXPECTED_SERVICES:
        containers.append(
            {
                "Config": {
                    "User": "1000:1000" if service_name == "minio" else "1654:1654",
                    "Image": ({
                        "postgres": POSTGRES_IMAGE,
                        "migration": POSTGRES_IMAGE,
                        "minio": MINIO_RUNTIME_IMAGE,
                        "minio-init": MC_IMAGE,
                    }.get(service_name)),
                    "Healthcheck": ({
                        "Test": ["CMD", "/usr/local/bin/ficant", "--health-check"],
                    } if service_name in RUST_SERVICES else {"Test": ["CMD", "true"]}),
                    "Labels": {
                        "com.docker.compose.project": PROJECT,
                        "com.docker.compose.service": service_name,
                        **({
                            "org.opencontainers.image.base.name": MINIO_IMAGE,
                            "org.opencontainers.image.licenses": "AGPL-3.0-only",
                        } if service_name == "minio" else {}),
                    },
                },
                "State": ({"Status": "exited", "ExitCode": 0} if service_name in INIT_SERVICES else {"Health": {"Status": "healthy"}}),
                "HostConfig": {
                    "ReadonlyRootfs": True,
                    "CapDrop": ["ALL"],
                    "SecurityOpt": ["no-new-privileges:true"],
                    "NanoCpus": 500_000_000,
                    "Memory": 128 * 1024 * 1024,
                    "PidsLimit": 64,
                    "Tmpfs": {"/tmp": "rw"},
                    "PortBindings": ({"8080/tcp": [{"HostIp": "127.0.0.1"}]} if service_name in PERSISTENCE_SERVICES | RUST_SERVICES else {}),
                },
                "Mounts": (
                    [{"Destination": "/etc/ficant/ficant.toml", "RW": False}]
                    if service_name in RUST_SERVICES
                    else ([{"Destination": "/var/lib/postgresql/data", "Type": "volume", "RW": True}]
                          if service_name == "postgres"
                          else ([{"Destination": "/data", "Type": "volume", "RW": True}]
                                if service_name == "minio"
                                else ([{"Destination": "/migrations", "Type": "bind", "RW": False}]
                                      if service_name == "migration" else [])))
                ),
            }
        )
    server = next(
        container
        for container in containers
        if container["Config"]["Labels"]["com.docker.compose.service"]
        == "ficant-server"
    )
    server["Config"]["Env"] = [
        "FICANT_GRPC_BIND=0.0.0.0:8080",
        "FICANT_GRPC_WEB_ALLOWED_ORIGINS=http://127.0.0.1:18082",
        "FICANT_PLATFORM_SIGNING_KEY_HEX=test-only-signing-key",
        "FICANT_PLATFORM_TRACE_KEY_HEX=test-only-trace-key",
    ]
    return containers


class ComposeSecurityGateTests(unittest.TestCase):
    @unittest.skipUnless(
        os.environ.get("FICANT_LIVE_MINIO_IMAGE_TEST") == "1",
        "set FICANT_LIVE_MINIO_IMAGE_TEST=1 for the targeted Docker gate",
    )
    def test_minio_runtime_image_owns_and_writes_fresh_data_volume_as_uid_1000(self) -> None:
        dockerfile = Path(MINIO_RUNTIME_DOCKERFILE)
        self.assertTrue(dockerfile.is_file(), "missing hardened MinIO runtime Dockerfile")

        suffix = f"{os.getpid()}-{uuid.uuid4().hex[:8]}"
        image = f"ficant-minio-runtime-test:{suffix}"
        volume = f"ficant-minio-runtime-test-{suffix}"
        container = f"ficant-minio-runtime-test-{suffix}"
        try:
            subprocess.run(
                ["docker", "build", "--pull=false", "--file", str(dockerfile), "--tag", image, "."],
                check=True,
            )
            inspected = json.loads(
                subprocess.run(
                    ["docker", "image", "inspect", image],
                    check=True,
                    capture_output=True,
                    text=True,
                ).stdout
            )[0]["Config"]
            self.assertEqual(inspected["User"], "1000:1000")
            self.assertEqual(inspected["Labels"]["org.opencontainers.image.base.name"], MINIO_IMAGE)
            self.assertEqual(inspected["Labels"]["org.opencontainers.image.licenses"], "AGPL-3.0-only")

            subprocess.run(["docker", "volume", "create", volume], check=True, capture_output=True)
            subprocess.run(
                [
                    "docker", "run", "--rm", "--read-only", "--cap-drop", "ALL",
                    "--security-opt", "no-new-privileges:true", "--volume", f"{volume}:/data",
                    "--entrypoint", "/bin/sh", image, "-ec",
                    "test \"$(id -u):$(id -g)\" = 1000:1000; "
                    "test \"$(stat -c '%u:%g' /data)\" = 1000:1000; "
                    "printf smoke > /data/.ficant-write-smoke; rm /data/.ficant-write-smoke",
                ],
                check=True,
            )
            subprocess.run(
                [
                    "docker", "run", "--detach", "--name", container, "--read-only",
                    "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true",
                    "--tmpfs", "/tmp:rw,noexec,nosuid,nodev,size=32m",
                    "--volume", f"{volume}:/data", "--env", "MINIO_ROOT_USER=fixtureadmin",
                    "--env", "MINIO_ROOT_PASSWORD=fixturepassword", image, "server", "/data",
                ],
                check=True,
                capture_output=True,
            )
            for _ in range(40):
                probe = subprocess.run(
                    ["docker", "exec", container, "curl", "--fail", "--silent", "http://127.0.0.1:9000/minio/health/live"],
                    check=False,
                    capture_output=True,
                )
                if probe.returncode == 0:
                    break
                time.sleep(0.25)
            else:
                logs = subprocess.run(
                    ["docker", "logs", container], check=False, capture_output=True, text=True
                ).stderr[-2000:]
                self.fail(f"hardened MinIO did not become live: {logs}")
            identity = subprocess.run(
                ["docker", "exec", container, "/bin/sh", "-ec", "id -u; id -g; stat -c '%u:%g' /data"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.splitlines()
            self.assertEqual(identity, ["1000", "1000", "1000:1000"])
        finally:
            subprocess.run(["docker", "rm", "--force", container], check=False, capture_output=True)
            subprocess.run(["docker", "volume", "rm", "--force", volume], check=False, capture_output=True)
            subprocess.run(["docker", "image", "rm", "--force", image], check=False, capture_output=True)

    @unittest.skipUnless(
        os.environ.get("FICANT_LIVE_COMPOSE_ENV_TEST") == "1",
        "set FICANT_LIVE_COMPOSE_ENV_TEST=1 for the targeted Compose environment gate",
    )
    def test_ficant_server_omits_unset_bootstrap_environment_and_accepts_explicit_pair(self) -> None:
        suffix = f"{os.getpid()}-{uuid.uuid4().hex[:8]}"
        project = f"ficant-optional-env-test-{suffix}"
        compose = [
            "docker", "compose", "--project-name", project,
            "--project-directory", "deploy/dev", "--file", "deploy/dev/docker-compose.yml",
            "--profile", "dev",
        ]
        base_environment = os.environ.copy()
        for key in (
            "FICANT_BOOTSTRAP_SUBJECT", "FICANT_BOOTSTRAP_BEARER_TOKEN",
            "FICANT_BOOTSTRAP_SCOPES", "FICANT_LOOPBACK_SUBJECT", "FICANT_LOOPBACK_SCOPES",
        ):
            base_environment.pop(key, None)
        base_environment.update({
            "FICANT_POSTGRES_PASSWORD": "fixture-postgres-password",
            "FICANT_MINIO_ROOT_USER": "fixtureadmin",
            "FICANT_MINIO_ROOT_PASSWORD": "fixture-minio-password",
            "FICANT_PLATFORM_SIGNING_KEY_HEX": "01" * 32,
            "FICANT_PLATFORM_TRACE_KEY_HEX": "02" * 32,
        })
        containers: list[str] = []
        image = f"{project}-ficant-server"
        try:
            unset_config = json.loads(subprocess.run(
                [*compose, "config", "--format", "json"], check=True,
                capture_output=True, text=True, env=base_environment,
            ).stdout)
            unset_environment = unset_config["services"]["ficant-server"]["environment"]
            for key in ("FICANT_BOOTSTRAP_SUBJECT", "FICANT_BOOTSTRAP_BEARER_TOKEN", "FICANT_BOOTSTRAP_SCOPES"):
                self.assertIsNone(unset_environment.get(key), f"{key} must resolve to omission")

            subprocess.run([*compose, "build", "ficant-server"], check=True, env=base_environment)
            scenarios = [
                ("unset", base_environment, {}),
                ("configured", {
                    **base_environment,
                    "FICANT_BOOTSTRAP_SUBJECT": "fixture-subject",
                    "FICANT_BOOTSTRAP_BEARER_TOKEN": "fixture-bearer-token",
                    "FICANT_BOOTSTRAP_SCOPES": "apps:read,rates:read",
                }, {
                    "FICANT_BOOTSTRAP_SUBJECT": "fixture-subject",
                    "FICANT_BOOTSTRAP_BEARER_TOKEN": "fixture-bearer-token",
                    "FICANT_BOOTSTRAP_SCOPES": "apps:read,rates:read",
                }),
            ]
            for label, environment, expected in scenarios:
                configured = json.loads(subprocess.run(
                    [*compose, "config", "--format", "json"], check=True,
                    capture_output=True, text=True, env=environment,
                ).stdout)["services"]["ficant-server"]["environment"]
                for key, value in expected.items():
                    self.assertEqual(configured.get(key), value)
                name = f"{project}-{label}"
                containers.append(name)
                subprocess.run(
                    [*compose, "run", "--no-deps", "--detach", "--name", name, "ficant-server"],
                    check=True, capture_output=True, env=environment,
                )
                for _ in range(40):
                    inspected = json.loads(subprocess.run(
                        ["docker", "container", "inspect", name], check=True,
                        capture_output=True, text=True,
                    ).stdout)[0]
                    state = inspected["State"]
                    if not state["Running"]:
                        break
                    if (state.get("Health") or {}).get("Status") == "healthy":
                        break
                    time.sleep(0.25)
                self.assertTrue(
                    inspected["State"]["Running"]
                    and (inspected["State"].get("Health") or {}).get("Status") == "healthy",
                    subprocess.run(
                        ["docker", "logs", name], check=False, capture_output=True, text=True,
                    ).stderr[-2000:],
                )
                actual = dict(entry.split("=", 1) for entry in inspected["Config"]["Env"] if "=" in entry)
                for key in ("FICANT_BOOTSTRAP_SUBJECT", "FICANT_BOOTSTRAP_BEARER_TOKEN", "FICANT_BOOTSTRAP_SCOPES"):
                    if key in expected:
                        self.assertEqual(actual.get(key), expected[key])
                    else:
                        self.assertNotIn(key, actual)
        finally:
            for container in containers:
                subprocess.run(["docker", "rm", "--force", container], check=False, capture_output=True)
            subprocess.run(
                [*compose, "down", "--volumes", "--remove-orphans"],
                check=False,
                capture_output=True,
                env=base_environment,
            )
            subprocess.run(["docker", "image", "rm", "--force", image], check=False, capture_output=True)

    def test_resolved_requires_persistence_init_migration_and_rust_dag(self) -> None:
        document = resolved_document()
        del document["services"]["postgres"]
        del document["services"]["minio-init"]

        failures = validate_resolved(document, PROJECT)

        self.assertTrue(any("postgres: missing resolved service" in item for item in failures))
        self.assertTrue(any("minio-init: missing resolved service" in item for item in failures))

    def test_resolved_rejects_services_outside_the_frozen_graph(self) -> None:
        document = resolved_document()
        document["services"]["debug-shell"] = {}

        failures = validate_resolved(document, PROJECT)

        self.assertTrue(any("resolved services must be" in item for item in failures))

    def test_compose_source_locks_storage_images_and_persistent_volumes(self) -> None:
        compose = Path("deploy/dev/docker-compose.yml").read_text(encoding="utf-8")

        for image in (POSTGRES_IMAGE, MINIO_IMAGE, MINIO_RUNTIME_IMAGE, MC_IMAGE):
            self.assertIn(image, compose)
        self.assertRegex(compose, r"(?m)^  postgres-data:\s*$")
        self.assertRegex(compose, r"(?m)^  minio-data:\s*$")
        self.assertIn("context: ../..", compose)
        self.assertIn(f"dockerfile: {MINIO_RUNTIME_DOCKERFILE}", compose)

    def test_delivery_lock_records_verified_source_tags_and_repo_digests(self) -> None:
        lock = tomllib.loads(Path("deploy/dev/toolchain.lock.toml").read_text(encoding="utf-8"))
        images = lock["docker"]["images"]

        self.assertEqual(images["postgres"], {
            "source_tag": "postgres:16.10-bookworm",
            "repo_digest": POSTGRES_IMAGE,
        })
        self.assertEqual(images["minio"], {
            "source_tag": "minio/minio:RELEASE.2025-04-22T22-12-26Z",
            "repo_digest": MINIO_IMAGE,
            "linux_amd64_manifest": MINIO_AMD64_MANIFEST,
            "runtime_image": MINIO_RUNTIME_IMAGE,
            "dockerfile": MINIO_RUNTIME_DOCKERFILE,
            "runtime_user": "1000:1000",
            "license": "AGPL-3.0-only",
        })
        self.assertEqual(images["minio_client"], {
            "source_tag": "minio/mc:RELEASE.2025-05-21T01-59-54Z",
            "repo_digest": MC_IMAGE,
        })

    def test_compose_source_has_health_to_init_migrate_to_readiness_dag(self) -> None:
        compose = Path("deploy/dev/docker-compose.yml").read_text(encoding="utf-8")

        for marker in (
            "condition: service_healthy",
            "condition: service_completed_successfully",
            "restart: \"no\"",
            "/migrations:ro",
        ):
            self.assertIn(marker, compose)

    def test_compose_source_fails_closed_for_all_uncommitted_credentials(self) -> None:
        compose = Path("deploy/dev/docker-compose.yml").read_text(encoding="utf-8")

        for key in (
            "FICANT_POSTGRES_PASSWORD",
            "FICANT_MINIO_ROOT_USER",
            "FICANT_MINIO_ROOT_PASSWORD",
            "FICANT_PLATFORM_SIGNING_KEY_HEX",
            "FICANT_PLATFORM_TRACE_KEY_HEX",
        ):
            self.assertRegex(compose, rf"\$\{{{key}:\?[^}}]+\}}")
        self.assertNotIn("test-only", compose)

    def test_expected_services_cover_the_complete_runtime_graph(self) -> None:
        self.assertEqual(
            EXPECTED_SERVICES,
            PERSISTENCE_SERVICES | INIT_SERVICES | RUST_SERVICES,
        )

    def test_resolved_rejects_minio_base_drift_and_bypassed_dag(self) -> None:
        document = resolved_document()
        document["services"]["minio"]["build"]["args"]["MINIO_IMAGE"] = "minio/minio:latest"
        document["services"]["ficant-server"]["depends_on"].pop("migration")

        failures = validate_resolved(document, PROJECT)

        self.assertIn("minio: build must use the locked base RepoDigest", failures)
        self.assertIn("ficant-server: dependency conditions must match the frozen runtime DAG", failures)

    def test_resolved_minio_requires_hardened_build_contract(self) -> None:
        mutations = {
            "runtime image": ("image", "minio/minio:latest"),
            "Dockerfile": ("dockerfile", "deploy/dev/RustService.Dockerfile"),
            "build context": ("context", "https://example.invalid/context.git"),
        }
        for expected, (field, value) in mutations.items():
            document = resolved_document()
            if field == "image":
                document["services"]["minio"][field] = value
            else:
                document["services"]["minio"]["build"][field] = value
            failures = validate_resolved(document, PROJECT)
            self.assertTrue(any(expected in item for item in failures), failures)

        document = resolved_document()
        document["services"]["minio"]["user"] = "1654:1654"
        self.assertIn(
            "minio: runtime user must be exactly 1000:1000",
            validate_resolved(document, PROJECT),
        )

    def test_minio_init_config_directory_must_be_on_tmpfs(self) -> None:
        for invalid in (None, "/etc/mc", "/var/lib/mc"):
            document = resolved_document()
            environment = document["services"]["minio-init"]["environment"]
            if invalid is None:
                environment.pop("MC_CONFIG_DIR", None)
            else:
                environment["MC_CONFIG_DIR"] = invalid

            failures = validate_resolved(document, PROJECT)

            self.assertIn(
                "minio-init: MC_CONFIG_DIR must use the /tmp tmpfs",
                failures,
            )

        document = resolved_document()
        document["services"]["minio-init"]["environment"]["MC_CONFIG_DIR"] = "/tmp/.mc"
        self.assertNotIn(
            "minio-init: MC_CONFIG_DIR must use the /tmp tmpfs",
            validate_resolved(document, PROJECT),
        )

    def test_runtime_rejects_minio_provenance_drift_and_failed_init(self) -> None:
        document = runtime_document()
        minio = next(item for item in document if item["Config"]["Labels"]["com.docker.compose.service"] == "minio")
        minio["Config"]["Labels"]["org.opencontainers.image.base.name"] = "minio/minio:latest"
        minio["Config"]["Labels"]["org.opencontainers.image.licenses"] = "NOASSERTION"
        migration = next(item for item in document if item["Config"]["Labels"]["com.docker.compose.service"] == "migration")
        migration["State"]["ExitCode"] = 1

        failures = validate_runtime(document, PROJECT)

        self.assertIn("minio: runtime base image provenance must match the locked RepoDigest", failures)
        self.assertIn("minio: runtime license label must be AGPL-3.0-only", failures)
        self.assertIn("migration: runtime init must have exited successfully", failures)

    def test_minio_dockerfile_preserves_base_license_and_non_root_contract(self) -> None:
        dockerfile = Path(MINIO_RUNTIME_DOCKERFILE).read_text(encoding="utf-8")
        self.assertIn(f"ARG MINIO_IMAGE={MINIO_IMAGE}", dockerfile)
        self.assertIn("mkdir -p /data", dockerfile)
        self.assertIn("chown 1000:1000 /data", dockerfile)
        self.assertIn('org.opencontainers.image.licenses="AGPL-3.0-only"', dockerfile)
        self.assertRegex(dockerfile, r"(?m)^USER 1000:1000$")

    def test_resolved_server_requires_exact_grpc_web_runtime_contract(self) -> None:
        mutations = {
            "public listener": ("FICANT_GRPC_BIND", "127.0.0.1:8080"),
            "exact CORS origin": ("FICANT_GRPC_WEB_ALLOWED_ORIGINS", "*"),
            "signing key injection": ("FICANT_PLATFORM_SIGNING_KEY_HEX", None),
            "trace key injection": ("FICANT_PLATFORM_TRACE_KEY_HEX", None),
        }

        for expected, (key, value) in mutations.items():
            document = resolved_document()
            environment = document["services"]["ficant-server"]["environment"]
            if value is None:
                environment.pop(key)
            else:
                environment[key] = value

            failures = validate_resolved(document, PROJECT)

            self.assertTrue(
                any(expected in failure for failure in failures),
                f"missing {expected} failure: {failures}",
            )

    def test_resolved_server_rejects_unsafe_optional_identity_combinations(self) -> None:
        unsafe_environments = [
            {"FICANT_BOOTSTRAP_SUBJECT": "dev-user"},
            {"FICANT_BOOTSTRAP_BEARER_TOKEN": "test-only-token"},
            {"FICANT_BOOTSTRAP_SCOPES": "rates:read"},
            {"FICANT_LOOPBACK_SUBJECT": "dev-user"},
            {"FICANT_LOOPBACK_SCOPES": "rates:read"},
        ]

        for unsafe in unsafe_environments:
            document = resolved_document()
            document["services"]["ficant-server"]["environment"].update(unsafe)

            failures = validate_resolved(document, PROJECT)

            self.assertTrue(
                any("identity configuration" in failure for failure in failures),
                f"unsafe identity configuration was accepted: {unsafe}",
            )

        document = resolved_document()
        document["services"]["ficant-server"]["environment"].update({
            "FICANT_BOOTSTRAP_SUBJECT": "",
            "FICANT_BOOTSTRAP_BEARER_TOKEN": "",
            "FICANT_BOOTSTRAP_SCOPES": "",
        })
        failures = validate_resolved(document, PROJECT)
        self.assertTrue(
            any("must be omitted or non-empty" in failure for failure in failures),
            f"explicit empty bootstrap environment was accepted: {failures}",
        )

    def test_resolved_services_require_the_binary_readiness_probe(self) -> None:
        document = resolved_document()
        document["services"]["ficant-server"]["healthcheck"]["test"] = [
            "CMD-SHELL",
            "exit 0",
        ]

        failures = validate_resolved(document, PROJECT)

        self.assertIn(
            "ficant-server: healthcheck must use the binary readiness probe",
            failures,
        )

    def test_compose_source_injects_secrets_without_values_and_keeps_exact_origin(self) -> None:
        compose = Path("deploy/dev/docker-compose.yml").read_text(encoding="utf-8")
        dockerfile = Path("deploy/dev/RustService.Dockerfile").read_text(encoding="utf-8")

        self.assertIn('FICANT_GRPC_BIND: "0.0.0.0:8080"', compose)
        self.assertIn(
            'FICANT_GRPC_WEB_ALLOWED_ORIGINS: "http://127.0.0.1:${FICANT_WEB_PORT:-18082}"',
            compose,
        )
        for key in ("FICANT_PLATFORM_SIGNING_KEY_HEX", "FICANT_PLATFORM_TRACE_KEY_HEX"):
            self.assertRegex(compose, rf"(?m)^[ \t]+{re.escape(key)}:[ \t]+\"\$\{{{key}:\?[^}}]+\}}\"$")
        for key in ("FICANT_BOOTSTRAP_SUBJECT", "FICANT_BOOTSTRAP_BEARER_TOKEN", "FICANT_BOOTSTRAP_SCOPES"):
            self.assertRegex(compose, rf"(?m)^[ \t]+{re.escape(key)}:[ \t]*$")
        self.assertNotIn("FICANT_LOOPBACK_SUBJECT", compose)
        self.assertNotIn("FICANT_LOOPBACK_SCOPES", compose)
        self.assertIn(
            'CMD ["/usr/local/bin/ficant", "--health-check"]',
            dockerfile,
        )

    def test_root_equivalent_users_are_rejected(self) -> None:
        for uid in ("0", "00", "+0", "root"):
            for user in (uid, f"{uid}:1654"):
                with self.subTest(user=user):
                    self.assertFalse(is_non_root(user))

    def test_nonzero_numeric_user_is_accepted(self) -> None:
        self.assertTrue(is_non_root("1654"))
        self.assertTrue(is_non_root("1654:1654"))

    def test_complete_resolved_and_runtime_fixtures_pass(self) -> None:
        self.assertEqual(validate_resolved(resolved_document(), PROJECT), [])
        self.assertEqual(validate_runtime(runtime_document(), PROJECT), [])

    def test_resolved_cap_add_cannot_bypass_drop_all(self) -> None:
        document = resolved_document()
        document["services"]["ficant-server"]["cap_add"] = ["SYS_ADMIN"]

        failures = validate_resolved(document, PROJECT)

        self.assertIn("ficant-server: cap_add must be empty", failures)

    def test_runtime_cap_add_cannot_bypass_drop_all(self) -> None:
        document = runtime_document()
        document[0]["HostConfig"]["CapAdd"] = ["SYS_ADMIN"]
        service_name = document[0]["Config"]["Labels"][
            "com.docker.compose.service"
        ]

        failures = validate_runtime(document, PROJECT)

        self.assertIn(f"{service_name}: runtime CapAdd must be empty", failures)

    def test_runtime_server_contract_and_binary_health_are_verified(self) -> None:
        document = runtime_document()
        server = next(
            container
            for container in document
            if container["Config"]["Labels"]["com.docker.compose.service"]
            == "ficant-server"
        )
        server["Config"]["Env"] = [
            "FICANT_GRPC_BIND=127.0.0.1:8080",
            "FICANT_GRPC_WEB_ALLOWED_ORIGINS=*",
            "FICANT_PLATFORM_SIGNING_KEY_HEX=do-not-leak",
        ]
        server["Config"]["Healthcheck"]["Test"] = ["CMD-SHELL", "exit 0"]
        server["State"]["Health"]["Status"] = "unhealthy"

        failures = validate_runtime(document, PROJECT)

        self.assertTrue(any("public listener" in failure for failure in failures))
        self.assertTrue(any("exact CORS origin" in failure for failure in failures))
        self.assertTrue(any("trace key injection" in failure for failure in failures))
        self.assertIn(
            "ficant-server: runtime healthcheck must use the binary readiness probe",
            failures,
        )
        self.assertIn("ficant-server: runtime health must be healthy", failures)
        self.assertNotIn("do-not-leak", "\n".join(failures))


if __name__ == "__main__":
    unittest.main()
