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

PERSISTENCE_SERVICES = {"postgres", "ceph-rgw"}
INIT_SERVICES = {"migration"}
RUST_SERVICES = {"ficant-server", "ficant-worker", "ficant-web"}
POSTGRES_IMAGE = "postgres@sha256:38471f330eb885e04de130b768d6db4e10469e2311879c7e5c699f6d2d8a1c74"
CEPH_IMAGE = "quay.io/ceph/ceph@sha256:6b4b5ae33acd3d736eb26d2a19238bce71a22f9cfb99cca887ba6312d0957644"
CEPH_RUNTIME_IMAGE = "ficant/ceph-rgw-runtime:dev"
CEPH_RUNTIME_DOCKERFILE = "deploy/dev/Ceph.Dockerfile"
CEPH_AMD64_MANIFEST = "sha256:55a5c2014b4db34589ad8886606409727f633319131bb663d37e0d489e350703"


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
        "volumes": {"postgres-data": {}, "ceph-data": {}},
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
                "ficant-web": {
                    "migration": "service_completed_successfully",
                    "ceph-rgw": "service_healthy",
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
    document["services"]["ceph-rgw"].update({
        "image": CEPH_RUNTIME_IMAGE,
        "user": "167:167",
        "build": {
            "context": "../..",
            "dockerfile": CEPH_RUNTIME_DOCKERFILE,
            "args": {"CEPH_IMAGE": CEPH_IMAGE},
        },
        "environment": {
            "FICANT_S3_ACCESS_KEY": "fixture-user",
            "FICANT_S3_SECRET_KEY": "fixture-only",
            "FICANT_S3_BUCKET": "ficant",
        },
        "volumes": [{"target": "/var/lib/ceph", "read_only": False}],
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
                    "User": "167:167" if service_name == "ceph-rgw" else "1654:1654",
                    "Image": ({
                        "postgres": POSTGRES_IMAGE,
                        "migration": POSTGRES_IMAGE,
                        "ceph-rgw": CEPH_RUNTIME_IMAGE,
                    }.get(service_name)),
                    "Healthcheck": ({
                        "Test": ["CMD", "/usr/local/bin/ficant", "--health-check"],
                    } if service_name in RUST_SERVICES else {"Test": ["CMD", "true"]}),
                    "Labels": {
                        "com.docker.compose.project": PROJECT,
                        "com.docker.compose.service": service_name,
                        **({
                            "org.opencontainers.image.base.name": CEPH_IMAGE,
                            "org.opencontainers.image.licenses": "LGPL-2.1-only OR LGPL-3.0-only",
                        } if service_name == "ceph-rgw" else {}),
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
                          else ([{"Destination": "/var/lib/ceph", "Type": "volume", "RW": True}]
                                if service_name == "ceph-rgw"
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
        os.environ.get("FICANT_LIVE_CEPH_IMAGE_TEST") == "1",
        "set FICANT_LIVE_CEPH_IMAGE_TEST=1 for the targeted Docker gate",
    )
    def test_ceph_runtime_image_owns_and_writes_fresh_data_volume_as_uid_167(self) -> None:
        dockerfile = Path(CEPH_RUNTIME_DOCKERFILE)
        self.assertTrue(dockerfile.is_file(), "missing hardened Ceph runtime Dockerfile")

        suffix = f"{os.getpid()}-{uuid.uuid4().hex[:8]}"
        image = f"ficant-ceph-runtime-test:{suffix}"
        volume = f"ficant-ceph-runtime-test-{suffix}"
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
            self.assertEqual(inspected["User"], "167:167")
            self.assertEqual(inspected["Labels"]["org.opencontainers.image.base.name"], CEPH_IMAGE)
            self.assertEqual(
                inspected["Labels"]["org.opencontainers.image.licenses"],
                "LGPL-2.1-only OR LGPL-3.0-only",
            )

            subprocess.run(["docker", "volume", "create", volume], check=True, capture_output=True)
            subprocess.run(
                [
                    "docker", "run", "--rm", "--read-only", "--cap-drop", "ALL",
                    "--security-opt", "no-new-privileges:true",
                    "--volume", f"{volume}:/var/lib/ceph",
                    "--entrypoint", "/bin/bash", image, "-ec",
                    "test \"$(id -u):$(id -g)\" = 167:167; "
                    "test \"$(stat -c '%u:%g' /var/lib/ceph)\" = 167:167; "
                    "printf smoke > /var/lib/ceph/.ficant-write-smoke; "
                    "rm /var/lib/ceph/.ficant-write-smoke",
                ],
                check=True,
            )
        finally:
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
            "FICANT_S3_ACCESS_KEY": "fixtureadmin",
            "FICANT_S3_SECRET_KEY": "fixture-s3-password",
            "FICANT_S3_BUCKET": "ficant",
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
        del document["services"]["ceph-rgw"]

        failures = validate_resolved(document, PROJECT)

        self.assertTrue(any("postgres: missing resolved service" in item for item in failures))
        self.assertTrue(any("ceph-rgw: missing resolved service" in item for item in failures))

    def test_resolved_rejects_services_outside_the_frozen_graph(self) -> None:
        document = resolved_document()
        document["services"]["debug-shell"] = {}

        failures = validate_resolved(document, PROJECT)

        self.assertTrue(any("resolved services must be" in item for item in failures))

    def test_compose_source_locks_storage_images_and_persistent_volumes(self) -> None:
        compose = Path("deploy/dev/docker-compose.yml").read_text(encoding="utf-8")

        for image in (POSTGRES_IMAGE, CEPH_IMAGE, CEPH_RUNTIME_IMAGE):
            self.assertIn(image, compose)
        self.assertRegex(compose, r"(?m)^  postgres-data:\s*$")
        self.assertRegex(compose, r"(?m)^  ceph-data:\s*$")
        self.assertIn("context: ../..", compose)
        self.assertIn(f"dockerfile: {CEPH_RUNTIME_DOCKERFILE}", compose)

    def test_delivery_lock_records_verified_source_tags_and_repo_digests(self) -> None:
        lock = tomllib.loads(Path("deploy/dev/toolchain.lock.toml").read_text(encoding="utf-8"))
        images = lock["docker"]["images"]

        self.assertEqual(images["postgres"], {
            "source_tag": "postgres:16.10-bookworm",
            "repo_digest": POSTGRES_IMAGE,
        })
        self.assertEqual(images["ceph"], {
            "source_tag": "quay.io/ceph/ceph:v20.2.2",
            "repo_digest": CEPH_IMAGE,
            "linux_amd64_manifest": CEPH_AMD64_MANIFEST,
            "runtime_image": CEPH_RUNTIME_IMAGE,
            "dockerfile": CEPH_RUNTIME_DOCKERFILE,
            "runtime_user": "167:167",
            "license": "LGPL-2.1-only OR LGPL-3.0-only",
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
            "FICANT_S3_ACCESS_KEY",
            "FICANT_S3_SECRET_KEY",
            "FICANT_PLATFORM_SIGNING_KEY_HEX",
            "FICANT_PLATFORM_TRACE_KEY_HEX",
            "FICANT_WORKER_RUNTIME_IMAGE_DIGEST",
            "FICANT_WORKER_NATIVE_SOURCE_DIGEST",
        ):
            self.assertRegex(compose, rf"\$\{{{key}:\?[^}}]+\}}")
        self.assertNotIn("test-only", compose)

    def test_ceph_readiness_matches_tentacle_anonymous_root_status(self) -> None:
        compose = Path("deploy/dev/docker-compose.yml").read_text(encoding="utf-8")
        entrypoint = Path("deploy/dev/ceph-entrypoint.sh").read_text(encoding="utf-8")

        self.assertIn('test \\"$${code}\\" = 200', compose)
        self.assertEqual(entrypoint.count('== "200"'), 2)

    def test_ceph_fixture_aligns_zonegroup_and_sigv4_region(self) -> None:
        entrypoint = Path("deploy/dev/ceph-entrypoint.sh").read_text(encoding="utf-8")

        self.assertIn('readonly s3_region="us-east-1"', entrypoint)
        self.assertIn('zonegroup modify --rgw-zonegroup default --api-name "$s3_region"', entrypoint)
        self.assertIn("create_bucket_sigv4()", entrypoint)
        self.assertIn("AWS4-HMAC-SHA256", entrypoint)
        self.assertIn('SignedHeaders=host;x-amz-content-sha256;x-amz-date', entrypoint)
        self.assertIn('--header "x-amz-content-sha256: ${empty_sha256}"', entrypoint)
        self.assertNotIn("--aws-sigv4", entrypoint)

    def test_expected_services_cover_the_complete_runtime_graph(self) -> None:
        self.assertEqual(
            EXPECTED_SERVICES,
            PERSISTENCE_SERVICES | INIT_SERVICES | RUST_SERVICES,
        )

    def test_resolved_rejects_ceph_base_drift_and_bypassed_dag(self) -> None:
        document = resolved_document()
        document["services"]["ceph-rgw"]["build"]["args"]["CEPH_IMAGE"] = "quay.io/ceph/ceph:latest"
        document["services"]["ficant-server"]["depends_on"].pop("migration")

        failures = validate_resolved(document, PROJECT)

        self.assertIn("ceph-rgw: build must use the locked base RepoDigest", failures)
        self.assertIn("ficant-server: dependency conditions must match the frozen runtime DAG", failures)

    def test_resolved_ceph_requires_hardened_build_contract(self) -> None:
        mutations = {
            "runtime image": ("image", "quay.io/ceph/ceph:latest"),
            "Dockerfile": ("dockerfile", "deploy/dev/RustService.Dockerfile"),
            "build context": ("context", "https://example.invalid/context.git"),
        }
        for expected, (field, value) in mutations.items():
            document = resolved_document()
            if field == "image":
                document["services"]["ceph-rgw"][field] = value
            else:
                document["services"]["ceph-rgw"]["build"][field] = value
            failures = validate_resolved(document, PROJECT)
            self.assertTrue(any(expected in item for item in failures), failures)

        document = resolved_document()
        document["services"]["ceph-rgw"]["user"] = "1654:1654"
        self.assertIn(
            "ceph-rgw: runtime user must be exactly 167:167",
            validate_resolved(document, PROJECT),
        )

    def test_runtime_rejects_ceph_provenance_drift_and_failed_init(self) -> None:
        document = runtime_document()
        ceph = next(item for item in document if item["Config"]["Labels"]["com.docker.compose.service"] == "ceph-rgw")
        ceph["Config"]["Labels"]["org.opencontainers.image.base.name"] = "quay.io/ceph/ceph:latest"
        ceph["Config"]["Labels"]["org.opencontainers.image.licenses"] = "NOASSERTION"
        migration = next(item for item in document if item["Config"]["Labels"]["com.docker.compose.service"] == "migration")
        migration["State"]["ExitCode"] = 1

        failures = validate_runtime(document, PROJECT)

        self.assertIn("ceph-rgw: runtime base image provenance must match the locked RepoDigest", failures)
        self.assertIn(
            "ceph-rgw: runtime license label must match the frozen Ceph dual-license expression",
            failures,
        )
        self.assertIn("migration: runtime init must have exited successfully", failures)

    def test_ceph_dockerfile_preserves_base_license_and_non_root_contract(self) -> None:
        dockerfile = Path(CEPH_RUNTIME_DOCKERFILE).read_text(encoding="utf-8")
        self.assertIn(f"ARG CEPH_IMAGE={CEPH_IMAGE}", dockerfile)
        self.assertIn("mkdir -p /var/lib/ceph/etc", dockerfile)
        self.assertIn("chown -R 167:167 /var/lib/ceph", dockerfile)
        self.assertIn(
            'org.opencontainers.image.licenses="LGPL-2.1-only OR LGPL-3.0-only"',
            dockerfile,
        )
        self.assertRegex(dockerfile, r"(?m)^USER 167:167$")
        for vulnerable_path in (
            "/usr/lib/python3.9/site-packages/setuptools",
            "/usr/lib/python3.9/site-packages/setuptools-69.2.0.dist-info",
            "/usr/lib/python3.9/site-packages/pkg_resources",
        ):
            self.assertIn(vulnerable_path, dockerfile)

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
            'FICANT_GRPC_WEB_ALLOWED_ORIGINS: "http://127.0.0.1:${FICANT_UI_PORT:-18083}"',
            compose,
        )
        for key in ("FICANT_PLATFORM_SIGNING_KEY_HEX", "FICANT_PLATFORM_TRACE_KEY_HEX"):
            self.assertRegex(compose, rf"(?m)^[ \t]+{re.escape(key)}:[ \t]+\"\$\{{{key}:\?[^}}]+\}}\"$")
        for key in ("FICANT_BOOTSTRAP_SUBJECT", "FICANT_BOOTSTRAP_BEARER_TOKEN", "FICANT_BOOTSTRAP_SCOPES"):
            self.assertRegex(compose, rf"(?m)^[ \t]+{re.escape(key)}:[ \t]+\"\$\{{{key}:\?[^}}]+\}}\"$")
        self.assertNotIn("FICANT_LOOPBACK_SUBJECT", compose)
        self.assertNotIn("FICANT_LOOPBACK_SCOPES", compose)
        self.assertIn(
            'CMD ["/usr/local/bin/ficant", "--health-check"]',
            dockerfile,
        )

    def test_optional_ui_profile_proxies_real_grpc_web_without_baking_credentials(self) -> None:
        compose = Path("deploy/dev/docker-compose.yml").read_text(encoding="utf-8")
        nginx = Path("deploy/test/ui/nginx.conf").read_text(encoding="utf-8")
        dockerfile = Path("deploy/test/FicantUi.Dockerfile").read_text(encoding="utf-8")

        self.assertRegex(compose, r"(?m)^  ficant-ui:\s*$")
        self.assertIn("profiles: [ui]", compose)
        self.assertIn("dockerfile: deploy/test/FicantUi.Dockerfile", compose)
        self.assertIn(
            'FICANT_UI_BEARER_TOKEN: "${FICANT_BOOTSTRAP_BEARER_TOKEN:?FICANT_BOOTSTRAP_BEARER_TOKEN is required}"',
            compose,
        )
        self.assertIn("location ^~ /ficant-api/", nginx)
        self.assertIn("proxy_pass http://ficant-server:8080/;", nginx)
        self.assertIn(
            'proxy_set_header Authorization "Bearer ${FICANT_UI_BEARER_TOKEN}";',
            nginx,
        )
        self.assertNotIn("local-platform-user", nginx)
        self.assertIn(
            "envsubst '$FICANT_UI_BEARER_TOKEN'",
            dockerfile,
        )
        self.assertNotIn("ARG FICANT_UI_BEARER_TOKEN", dockerfile)

    def test_dev_entrypoints_preserve_volumes_and_verify_grpc_status_zero(self) -> None:
        up = Path("scripts/dev-up.ps1").read_text(encoding="utf-8")
        down = Path("scripts/dev-down.ps1").read_text(encoding="utf-8")

        self.assertIn("Join-Path $repoRoot 'deploy\\dev'", up)
        self.assertIn("Join-Path $composeDirectory '.env.local'", up)
        self.assertIn("[System.Security.Cryptography.RandomNumberGenerator]::Fill", up)
        self.assertIn("'--env-file', $environmentFile", up)
        self.assertNotIn("FICANT_BOOTSTRAP_BEARER_TOKEN=$(", up.split("$entries = @(", 1)[0])
        self.assertIn("/ficant-api/ficant.app.v1.PlatformService/GetCurrentSession", up)
        self.assertIn("grpc-status:\\s*0", up)
        self.assertIn("'--profile', 'ui'", up)
        self.assertIn("docker image inspect --format '{{.Id}}' $workerImage", up)
        self.assertIn("'--print-native-source-digest'", up)
        self.assertIn("RuntimeDigest = $runtimeDigest", up)
        self.assertIn("SourceDigest = $sourceDigest", up)
        entries = up.split("$entries = @(", 1)[1].split(")", 1)[0]
        self.assertNotIn("FICANT_WORKER_RUNTIME_IMAGE_DIGEST", entries)
        self.assertNotIn("FICANT_WORKER_NATIVE_SOURCE_DIGEST", entries)
        self.assertIn("'down'", down)
        self.assertNotIn("'--volumes'", down)
        self.assertNotIn("Remove-Item", down)

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


class ReleaseDeploymentContractTests(unittest.TestCase):
    @staticmethod
    def resolved_release_document() -> dict[str, object]:
        environment = {
            **os.environ,
            "FICANT_DEPLOY_SHA": "0" * 40,
            "FICANT_STORAGE_SHA": "0" * 40,
            "FICANT_IMAGE_PREFIX": "ghcr.io/kayz/ficant",
            "FICANT_ROOT": "/srv/ficant-test",
            "FICANT_POSTGRES_PASSWORD": "validation-only",
            "FICANT_S3_ACCESS_KEY": "validation-access",
            "FICANT_S3_SECRET_KEY": "validation-only-secret-key-00000000",
            "FICANT_S3_BUCKET": "ficant",
            "FICANT_PLATFORM_SIGNING_KEY_HEX": "00" * 32,
            "FICANT_PLATFORM_TRACE_KEY_HEX": "00" * 32,
            "FICANT_BOOTSTRAP_BEARER_TOKEN": "validation-bootstrap-token-00000000",
            "FICANT_EXPERIMENT_CURSOR_KEY_HEX": "11" * 32,
            "FICANT_WORKER_RUNTIME_IMAGE_DIGEST": "sha256:" + "22" * 32,
            "FICANT_WORKER_NATIVE_SOURCE_DIGEST": "sha256:" + "33" * 32,
            "FICANT_GRPC_WEB_ALLOWED_ORIGINS": "https://greatquant.com",
        }
        output = subprocess.run(
            [
                "docker",
                "compose",
                "--file",
                "deploy/test/compose.test.yml",
                "config",
                "--format",
                "json",
            ],
            check=True,
            capture_output=True,
            text=True,
            env=environment,
        ).stdout
        return json.loads(output)

    @staticmethod
    def validate_release(document: dict[str, object]) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "deploy/test/validate_release.py"],
            input=json.dumps(document),
            capture_output=True,
            text=True,
            check=False,
        )

    def test_release_topology_includes_real_worker_and_ceph_contract(self) -> None:
        document = self.resolved_release_document()
        result = self.validate_release(document)

        self.assertEqual(result.returncode, 0, result.stderr)
        worker = document["services"]["ficant-worker"]
        self.assertEqual(
            worker["environment"]["FICANT_WORKER_S3_ENDPOINT"],
            "http://ceph-rgw:9000",
        )
        self.assertEqual(
            worker["depends_on"]["ceph-rgw"]["condition"],
            "service_healthy",
        )
        self.assertEqual(
            worker["environment"]["FICANT_WORKER_RUNTIME_IMAGE_DIGEST"],
            "sha256:" + "22" * 32,
        )
        self.assertEqual(
            document["services"]["ficant-server"]["environment"][
                "FICANT_EXPERIMENT_NATIVE_SOURCE_DIGEST"
            ],
            "sha256:" + "33" * 32,
        )
        self.assertEqual(
            document["services"]["ficant-ui"]["environment"][
                "FICANT_UI_BEARER_TOKEN"
            ],
            "validation-bootstrap-token-00000000",
        )
        self.assertIn("ceph-data", document["volumes"])
        self.assertEqual(
            document["services"]["ficant-ui"]["healthcheck"]["test"],
            [
                "CMD",
                "wget",
                "--quiet",
                "--spider",
                "http://127.0.0.1:8080/health",
            ],
        )

    def test_release_validator_rejects_missing_ceph_and_worker_credentials(self) -> None:
        without_ceph = self.resolved_release_document()
        without_ceph["services"].pop("ceph-rgw")
        result = self.validate_release(without_ceph)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unexpected services", result.stderr)

        without_worker_secret = self.resolved_release_document()
        without_worker_secret["services"]["ficant-worker"]["environment"].pop(
            "FICANT_WORKER_S3_SECRET_KEY"
        )
        result = self.validate_release(without_worker_secret)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing its production database/S3/identity environment", result.stderr)

    def test_rollback_smoke_accepts_forward_only_migration_supersets(self) -> None:
        smoke = Path("deploy/test/bin/smoke-test.sh").read_text(encoding="utf-8")

        self.assertIn('missing=$(comm -23 "$expected_file" "$applied_file")', smoke)
        self.assertIn("required_migrations=$required applied_migrations=$applied", smoke)
        self.assertNotIn('"$applied" -eq "$expected"', smoke)

    def test_rollback_keeps_a_available_storage_runtime_across_legacy_app_shas(
        self,
    ) -> None:
        compose = Path("deploy/test/compose.test.yml").read_text(encoding="utf-8")
        deploy = Path("deploy/test/bin/deploy.sh").read_text(encoding="utf-8")
        rollback = Path("deploy/test/bin/rollback.sh").read_text(encoding="utf-8")

        self.assertIn("-ceph-rgw:sha-${FICANT_STORAGE_SHA:", compose)
        self.assertIn("storage_sha=${FICANT_STORAGE_SHA:-$sha}", deploy)
        self.assertIn(
            "FICANT_DEPLOY_SHA=%s\\nFICANT_STORAGE_SHA=%s\\n",
            deploy,
        )
        self.assertIn("storage_sha=$current_storage", rollback)
        self.assertIn("FICANT_STORAGE_SHA=$storage_sha", rollback)

    def test_release_workflow_builds_scans_promotes_and_configures_ceph(self) -> None:
        workflow = Path(".github/workflows/release-test.yml").read_text(encoding="utf-8")

        for marker in (
            "build-ceph:",
            "file: deploy/dev/Ceph.Dockerfile",
            "package: [ficant-server, ficant-worker, ficant-web, ficant-ui, ficant-ceph-rgw]",
            "for package in ficant-server ficant-worker ficant-web ficant-ui ficant-ceph-rgw",
            "Configure test object-store credentials",
            "Preload exact Ceph SHA image through the runner",
            'docker save "$image" | gzip -1 | ssh',
            '"$USER@$HOST" "gzip -d | docker load"',
            "FICANT_TEST_S3_ACCESS_KEY",
            "FICANT_TEST_S3_SECRET_KEY",
            "FICANT_TEST_EXPERIMENT_CURSOR_KEY_HEX",
            "FICANT_TEST_BOOTSTRAP_BEARER_TOKEN",
        ):
            self.assertIn(marker, workflow)

        deploy = Path("deploy/test/bin/deploy.sh").read_text(encoding="utf-8")
        self.assertIn("--print-native-source-digest", deploy)
        self.assertIn("FICANT_WORKER_RUNTIME_IMAGE_DIGEST", deploy)

        self.assertIn(
            'if [[ "${{ github.event_name }}" == workflow_run ]]; then\n'
            '            [[ "$sha" == $(git rev-parse origin/main) ]]',
            workflow,
        )

    def test_release_rust_build_reuses_locked_cargo_and_target_caches(self) -> None:
        dockerfile = Path("deploy/dev/RustService.Dockerfile").read_text(
            encoding="utf-8"
        )

        self.assertIn(
            "id=ficant-cargo-registry-v1,target=/usr/local/cargo/registry,sharing=locked",
            dockerfile,
        )
        self.assertIn(
            "id=ficant-release-target-v1,target=/workspace/target,sharing=locked",
            dockerfile,
        )


if __name__ == "__main__":
    unittest.main()
