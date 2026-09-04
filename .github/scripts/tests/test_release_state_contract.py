#!/usr/bin/env python3
"""Regression tests for immutable version concurrency and atomic deploy state."""

from __future__ import annotations

import json
import pathlib
import re
import shlex
import subprocess
import tempfile
import unittest


REPO = pathlib.Path(__file__).resolve().parents[3]
DEPLOY = REPO / "deploy/test/bin/deploy.sh"
ROLLBACK = REPO / "deploy/test/bin/rollback.sh"
CICD = REPO / "cicd.yml"
CI_WORKFLOW = REPO / ".github/workflows/ci.yml"
RELEASE_WORKFLOW = REPO / ".github/workflows/release-test.yml"


def deployment_state_function(source: str) -> str:
    match = re.search(
        r"(?ms)^write_deployment_state\(\) \(\n.*?^\)\n",
        source,
    )
    if match is None:
        raise AssertionError("write_deployment_state must remain a subshell function")
    return match.group(0)


def execution_identity_function(source: str) -> str:
    match = re.search(
        r"(?ms)^configure_execution_identity\(\) \{\n.*?^\}\n",
        source,
    )
    if match is None:
        raise AssertionError("configure_execution_identity must remain a function")
    return match.group(0)


def write_executable(path: pathlib.Path, source: str) -> None:
    path.write_bytes(source.encode("utf-8"))
    path.chmod(0o755)


class ReleaseStateContractTests(unittest.TestCase):
    def test_version_runs_are_never_cancelled_automatically(self) -> None:
        cicd = json.loads(CICD.read_text(encoding="utf-8"))
        self.assertIs(cicd["ci"]["cancel_outdated_runs"], False)

        workflow = CI_WORKFLOW.read_text(encoding="utf-8")
        self.assertRegex(
            workflow,
            re.compile(
                r"(?m)^concurrency:\n"
                r"  group: ci-\$\{\{ github\.ref \}\}\n"
                r"  cancel-in-progress: false$"
            ),
        )

    def test_state_writes_use_same_directory_atomic_replacement(self) -> None:
        source = DEPLOY.read_text(encoding="utf-8")
        function = deployment_state_function(source)

        self.assertIn(
            'temporary=$(mktemp "$directory/.${filename}.tmp.XXXXXX")',
            function,
        )
        self.assertIn('trap cleanup_state_temporary EXIT', function)
        self.assertIn('chmod 0600 "$temporary"', function)
        self.assertIn('mv -f -- "$temporary" "$destination"', function)
        self.assertNotRegex(
            source,
            r'>"\$root/state/(?:current|previous)\.env"',
        )

        previous = source.index('"$root/state/previous.env"')
        current = source.index('"$root/state/current.env"', previous)
        record = source.index("record success false", current)
        self.assertLess(previous, current)
        self.assertLess(current, record)

    def test_deploy_and_rollback_bind_the_authorized_commit_and_tree(self) -> None:
        deploy = DEPLOY.read_text(encoding="utf-8")
        rollback = ROLLBACK.read_text(encoding="utf-8")
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")

        self.assertIn("tree=${2:-}", deploy)
        self.assertIn(
            "Usage: deploy.sh <40-character-commit-sha> <40-character-tree-sha>",
            deploy,
        )
        self.assertIn("export FICANT_CODE_COMMIT_SHA=$deploy_sha", deploy)
        self.assertIn("export FICANT_CODE_TREE_SHA=$deploy_tree", deploy)
        self.assertIn("export FICANT_SERVER_RUNTIME_IMAGE_DIGEST=$server_runtime", deploy)
        self.assertIn('configure_execution_identity "$sha" "$tree" "$allow_legacy"', deploy)
        self.assertIn("A zero tree identity is reserved for legacy rollback.", deploy)

        self.assertIn("previous_tree=${FICANT_CODE_TREE_SHA:-}", rollback)
        self.assertIn("previous_tree=$zero_sha", rollback)
        self.assertIn("FICANT_ALLOW_LEGACY_ROLLBACK=$legacy_rollback", rollback)
        self.assertIn(
            '"$root/bin/deploy.sh" "$previous" "$previous_tree"',
            rollback,
        )

        self.assertIn(
            'FICANT_CODE_TREE_SHA: "0000000000000000000000000000000000000000"',
            workflow,
        )
        self.assertIn(
            "FICANT_SERVER_RUNTIME_IMAGE_DIGEST: sha256:" + "4" * 64,
            workflow,
        )
        self.assertIn("TREE: ${{ needs.authorize.outputs.tree }}", workflow)
        self.assertIn("'$ROOT/bin/deploy.sh' '$SHA' '$TREE'", workflow)

    def test_execution_identity_comes_from_pulled_server_and_worker_images(self) -> None:
        source = DEPLOY.read_text(encoding="utf-8")
        identity = execution_identity_function(source)

        self.assertIn('server_image="$image_prefix-server:sha-$deploy_sha"', identity)
        self.assertIn('worker_image="$image_prefix-worker:sha-$deploy_sha"', identity)
        self.assertIn(
            "server_runtime=$(docker image inspect --format '{{.Id}}' \"$server_image\")",
            identity,
        )
        self.assertIn(
            "worker_runtime=$(docker image inspect --format '{{.Id}}' \"$worker_image\")",
            identity,
        )
        self.assertGreaterEqual(identity.count("^sha256:[0-9a-f]{64}$"), 3)
        self.assertIn('"$worker_image" --print-native-source-digest', identity)

    def test_legacy_source_introspection_failure_resets_inherited_digest(self) -> None:
        identity = execution_identity_function(DEPLOY.read_text(encoding="utf-8"))
        zero_digest = "sha256:" + "0" * 64
        inherited_digest = "sha256:" + "f" * 64
        runtime_digest = "sha256:" + "a" * 64
        script = f"""set -euo pipefail
zero_digest={zero_digest}
docker() {{
  if [[ "$1" == image && "$2" == inspect ]]; then
    printf '%s\n' {runtime_digest}
    return 0
  fi
  if [[ "$1" == run ]]; then
    return 75
  fi
  return 90
}}
{identity}
export FICANT_WORKER_NATIVE_SOURCE_DIGEST={inherited_digest}
configure_execution_identity {'a' * 40} {'0' * 40} true
printf '%s\n' "$FICANT_WORKER_NATIVE_SOURCE_DIGEST"
"""
        completed = subprocess.run(
            ["bash"],
            cwd=REPO,
            input=script.encode("utf-8"),
            capture_output=True,
            check=False,
        )
        self.assertEqual(
            completed.returncode,
            0,
            f"stdout:\n{completed.stdout.decode()}\nstderr:\n{completed.stderr.decode()}",
        )
        self.assertEqual(completed.stdout.decode("utf-8"), f"{zero_digest}\n")
        self.assertIn(b"using compatibility placeholders", completed.stderr)

    def test_new_deployments_cannot_use_the_legacy_zero_tree(self) -> None:
        deploy_path = DEPLOY.relative_to(REPO).as_posix()
        missing_tree = subprocess.run(
            [
                "bash",
                "-c",
                "unset GHCR_USER FICANT_ALLOW_LEGACY_ROLLBACK; exec bash "
                f"{shlex.quote(deploy_path)} {'a' * 40}",
            ],
            cwd=REPO,
            input=b"",
            capture_output=True,
            check=False,
        )
        self.assertEqual(missing_tree.returncode, 2)
        self.assertIn(b"<40-character-tree-sha>", missing_tree.stderr)

        zero_tree = subprocess.run(
            [
                "bash",
                "-c",
                "unset GHCR_USER FICANT_ALLOW_LEGACY_ROLLBACK; exec bash "
                f"{shlex.quote(deploy_path)} {'a' * 40} {'0' * 40}",
            ],
            cwd=REPO,
            input=b"",
            capture_output=True,
            check=False,
        )
        self.assertEqual(zero_tree.returncode, 2)
        self.assertIn(b"reserved for legacy rollback", zero_tree.stderr)

        valid_tree = subprocess.run(
            [
                "bash",
                "-c",
                "unset GHCR_USER FICANT_ALLOW_LEGACY_ROLLBACK; exec bash "
                f"{shlex.quote(deploy_path)} {'a' * 40} {'b' * 40}",
            ],
            cwd=REPO,
            input=b"",
            capture_output=True,
            check=False,
        )
        self.assertEqual(valid_tree.returncode, 2)
        self.assertIn(b"GHCR_USER is required", valid_tree.stderr)

        legacy_tree = subprocess.run(
            [
                "bash",
                "-c",
                "unset GHCR_USER; FICANT_ALLOW_LEGACY_ROLLBACK=true exec bash "
                f"{shlex.quote(deploy_path)} {'a' * 40} {'0' * 40}",
            ],
            cwd=REPO,
            input=b"",
            capture_output=True,
            check=False,
        )
        self.assertEqual(legacy_tree.returncode, 2)
        self.assertIn(b"GHCR_USER is required", legacy_tree.stderr)

        downgraded_tree = subprocess.run(
            [
                "bash",
                "-c",
                "unset GHCR_USER; FICANT_ALLOW_LEGACY_ROLLBACK=true exec bash "
                f"{shlex.quote(deploy_path)} {'a' * 40} {'b' * 40}",
            ],
            cwd=REPO,
            input=b"",
            capture_output=True,
            check=False,
        )
        self.assertEqual(downgraded_tree.returncode, 2)
        self.assertIn(b"requires the zero tree identity", downgraded_tree.stderr)

    def test_successful_state_write_is_complete_and_private(self) -> None:
        function = deployment_state_function(DEPLOY.read_text(encoding="utf-8"))
        script = f"""set -euo pipefail
{function}
state_root=$(mktemp -d)
trap 'rm -rf -- "$state_root"' EXIT
destination="$state_root/current.env"
write_deployment_state \
  "$destination" \
  {'a' * 40} \
  {'f' * 40} \
  ghcr.io/kayz/ficant-ceph-rgw@sha256:{'b' * 64} \
  sha256:{'c' * 64} \
  sha256:{'6' * 64} \
  sha256:{'d' * 64} \
  sha256:{'e' * 64}
cat "$destination"
printf '__FICANT_MODE__=%s\n' "$(stat -c '%a' "$destination")"
shopt -s nullglob
temporary_files=("$state_root"/.current.env.tmp.*)
printf '__FICANT_TEMP_COUNT__=%s\n' "${{#temporary_files[@]}}"
"""
        completed = subprocess.run(
            ["bash"],
            cwd=REPO,
            input=script.encode("utf-8"),
            capture_output=True,
            check=False,
        )
        stdout = completed.stdout.decode("utf-8")
        stderr = completed.stderr.decode("utf-8")
        self.assertEqual(
            completed.returncode,
            0,
            f"stdout:\n{stdout}\nstderr:\n{stderr}",
        )
        self.assertEqual(
            stdout,
            "\n".join(
                (
                    f"FICANT_DEPLOY_SHA={'a' * 40}",
                    f"FICANT_CODE_TREE_SHA={'f' * 40}",
                    "FICANT_STORAGE_RUNTIME_IMAGE="
                    f"ghcr.io/kayz/ficant-ceph-rgw@sha256:{'b' * 64}",
                    f"FICANT_STORAGE_RUNTIME_CONFIG_DIGEST=sha256:{'c' * 64}",
                    f"FICANT_SERVER_RUNTIME_IMAGE_DIGEST=sha256:{'6' * 64}",
                    f"FICANT_WORKER_RUNTIME_IMAGE_DIGEST=sha256:{'d' * 64}",
                    f"FICANT_WORKER_NATIVE_SOURCE_DIGEST=sha256:{'e' * 64}",
                    "__FICANT_MODE__=600",
                    "__FICANT_TEMP_COUNT__=0",
                    "",
                )
            ),
        )

    def test_failed_rename_preserves_old_state_and_cleans_temporary(self) -> None:
        function = deployment_state_function(DEPLOY.read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory(dir=REPO) as temporary:
            state_root = pathlib.Path(temporary)
            destination = state_root / "current.env"
            destination.write_text("old-state\n", encoding="utf-8")
            bash_destination = destination.relative_to(REPO).as_posix()
            script = f"""set -euo pipefail
{function}
mv() {{ return 73; }}
if write_deployment_state \
  {shlex.quote(bash_destination)} \
  {'a' * 40} \
  {'f' * 40} \
  ghcr.io/kayz/ficant-ceph-rgw@sha256:{'b' * 64} \
  sha256:{'c' * 64} \
  sha256:{'6' * 64} \
  sha256:{'d' * 64} \
  sha256:{'e' * 64}; then
  exit 90
fi
"""
            completed = subprocess.run(
                ["bash"],
                cwd=REPO,
                input=script.encode("utf-8"),
                capture_output=True,
                check=False,
            )
            stdout = completed.stdout.decode("utf-8")
            stderr = completed.stderr.decode("utf-8")
            self.assertEqual(
                completed.returncode,
                0,
                f"stdout:\n{stdout}\nstderr:\n{stderr}",
            )
            self.assertEqual(destination.read_text(encoding="utf-8"), "old-state\n")
            self.assertEqual(list(state_root.glob(".current.env.tmp.*")), [])

    def test_record_failure_after_state_replacement_restores_runtime_and_state(self) -> None:
        candidate = "a" * 40
        candidate_tree = "b" * 40
        current = "c" * 40
        current_tree = "d" * 40
        storage_image = "ghcr.io/kayz/ficant-ceph-rgw@sha256:" + "1" * 64
        storage_config = "sha256:" + "2" * 64
        stale_server_runtime = "sha256:" + "3" * 64
        stale_worker_runtime = "sha256:" + "4" * 64
        stale_worker_source = "sha256:" + "5" * 64
        candidate_server_runtime = "sha256:" + "6" * 64
        candidate_worker_runtime = "sha256:" + "7" * 64
        candidate_worker_source = "sha256:" + "8" * 64
        restored_server_runtime = "sha256:" + "9" * 64
        restored_worker_runtime = "sha256:" + "a" * 64
        restored_worker_source = "sha256:" + "b" * 64

        docker_mock = r"""#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == login || "$1" == logout ]]; then
  exit 0
fi

if [[ "$1" == image && "$2" == inspect ]]; then
  format=$4
  image=${@: -1}
  if [[ "$format" == *RepoDigests* ]]; then
    printf '%s\n' "$image"
    exit 0
  fi
  case "$image" in
    *-server:sha-"$FICANT_MOCK_CANDIDATE_SHA")
      printf '%s\n' "$FICANT_MOCK_CANDIDATE_SERVER_RUNTIME"
      ;;
    *-worker:sha-"$FICANT_MOCK_CANDIDATE_SHA")
      printf '%s\n' "$FICANT_MOCK_CANDIDATE_WORKER_RUNTIME"
      ;;
    *-server:sha-"$FICANT_MOCK_CURRENT_SHA")
      printf '%s\n' "$FICANT_MOCK_RESTORED_SERVER_RUNTIME"
      ;;
    *-worker:sha-"$FICANT_MOCK_CURRENT_SHA")
      printf '%s\n' "$FICANT_MOCK_RESTORED_WORKER_RUNTIME"
      ;;
    ghcr.io/*@sha256:*)
      printf '%s\n' "$FICANT_MOCK_STORAGE_CONFIG"
      ;;
    *)
      exit 91
      ;;
  esac
  exit 0
fi

if [[ "$1" == run ]]; then
  worker_image=''
  for argument in "$@"; do
    case "$argument" in
      *-worker:sha-*) worker_image=$argument ;;
    esac
  done
  case "$worker_image" in
    *-worker:sha-"$FICANT_MOCK_CANDIDATE_SHA")
      printf '%s\n' "$FICANT_MOCK_CANDIDATE_WORKER_SOURCE"
      ;;
    *-worker:sha-"$FICANT_MOCK_CURRENT_SHA")
      printf '%s\n' "$FICANT_MOCK_RESTORED_WORKER_SOURCE"
      ;;
    *)
      exit 92
      ;;
  esac
  exit 0
fi

if [[ "$1" == compose ]]; then
  for argument in "$@"; do
    if [[ "$argument" == up ]]; then
      printf '%s\n' "$FICANT_DEPLOY_SHA" >"$FICANT_MOCK_RUNNING_FILE"
      break
    fi
  done
  exit 0
fi

exit 93
"""
        record_failure_mock = r"""printf() {
  if [[ ${1:-} == '{"commit_sha":"%s"'* && ${7:-} == success ]]; then
    local state_line
    IFS= read -r state_line <"$FICANT_ROOT/state/current.env"
    builtin printf '%s\n' "${state_line#*=}" >"$FICANT_MOCK_STATE_BEFORE_RECORD_FILE"
    return 73
  fi
  builtin printf "$@"
}
"""
        probe_mock = r"""#!/usr/bin/env bash
set -euo pipefail
IFS= read -r running <"$FICANT_MOCK_RUNNING_FILE"
[[ "$running" == "$FICANT_DEPLOY_SHA" ]]
"""

        with tempfile.TemporaryDirectory(dir=REPO) as temporary:
            temporary_root = pathlib.Path(temporary)
            deployment_root = temporary_root / "root"
            fake_bin = temporary_root / "bin"
            state_root = deployment_root / "state"
            deployments = state_root / "deployments"
            release_migrations = deployment_root / "releases" / candidate / "migrations"
            script_bin = deployment_root / "bin"
            for directory in (
                fake_bin,
                deployments,
                release_migrations,
                script_bin,
            ):
                directory.mkdir(parents=True, exist_ok=True)

            (deployment_root / ".env").write_text("# mock\n", encoding="utf-8")
            (deployment_root / "compose.test.yml").write_text(
                "services: {}\n", encoding="utf-8"
            )
            current_state = "\n".join(
                (
                    f"FICANT_DEPLOY_SHA={current}",
                    f"FICANT_CODE_TREE_SHA={current_tree}",
                    f"FICANT_STORAGE_RUNTIME_IMAGE={storage_image}",
                    f"FICANT_STORAGE_RUNTIME_CONFIG_DIGEST={storage_config}",
                    f"FICANT_SERVER_RUNTIME_IMAGE_DIGEST={stale_server_runtime}",
                    f"FICANT_WORKER_RUNTIME_IMAGE_DIGEST={stale_worker_runtime}",
                    f"FICANT_WORKER_NATIVE_SOURCE_DIGEST={stale_worker_source}",
                    "",
                )
            )
            (state_root / "current.env").write_bytes(current_state.encode("utf-8"))
            old_record = deployments / f"{current}.json"
            old_record.write_text('{"status":"old-success"}\n', encoding="utf-8")

            write_executable(fake_bin / "docker", docker_mock)
            write_executable(fake_bin / "record-failure.bash", record_failure_mock)
            write_executable(script_bin / "healthcheck.sh", probe_mock)
            write_executable(script_bin / "smoke-test.sh", probe_mock)

            running_file = temporary_root / "running"
            state_before_record_file = temporary_root / "state-before-record"
            relative = lambda path: path.relative_to(REPO).as_posix()
            assignments = {
                "BASH_ENV": relative(fake_bin / "record-failure.bash"),
                "FICANT_ROOT": relative(deployment_root),
                "GHCR_USER": "mock-user",
                "FICANT_STORAGE_RUNTIME_IMAGE": storage_image,
                "FICANT_STORAGE_RUNTIME_CONFIG_DIGEST": storage_config,
                "FICANT_IMAGE_PREFIX": "ghcr.io/kayz/ficant",
                "FICANT_MOCK_CANDIDATE_SHA": candidate,
                "FICANT_MOCK_CURRENT_SHA": current,
                "FICANT_MOCK_STORAGE_CONFIG": storage_config,
                "FICANT_MOCK_CANDIDATE_SERVER_RUNTIME": candidate_server_runtime,
                "FICANT_MOCK_CANDIDATE_WORKER_RUNTIME": candidate_worker_runtime,
                "FICANT_MOCK_CANDIDATE_WORKER_SOURCE": candidate_worker_source,
                "FICANT_MOCK_RESTORED_SERVER_RUNTIME": restored_server_runtime,
                "FICANT_MOCK_RESTORED_WORKER_RUNTIME": restored_worker_runtime,
                "FICANT_MOCK_RESTORED_WORKER_SOURCE": restored_worker_source,
                "FICANT_MOCK_RUNNING_FILE": relative(running_file),
                "FICANT_MOCK_STATE_BEFORE_RECORD_FILE": relative(
                    state_before_record_file
                ),
            }
            environment = "\n".join(
                (
                    f"export PATH={shlex.quote(relative(fake_bin))}:\"$PATH\"",
                    *(f"export {name}={shlex.quote(value)}" for name, value in assignments.items()),
                )
            )
            deploy_path = DEPLOY.relative_to(REPO).as_posix()
            command = (
                f"{environment}\nprintf '%s\\n' mock-token | "
                f"bash {shlex.quote(deploy_path)} "
                f"{candidate} {candidate_tree}"
            )
            completed = subprocess.run(
                ["bash"],
                cwd=REPO,
                input=(command + "\n").encode("utf-8"),
                capture_output=True,
                check=False,
            )
            stdout = completed.stdout.decode("utf-8")
            stderr = completed.stderr.decode("utf-8")
            self.assertEqual(
                completed.returncode,
                73,
                f"stdout:\n{stdout}\nstderr:\n{stderr}",
            )
            self.assertEqual(state_before_record_file.read_text().strip(), candidate)
            self.assertEqual(
                running_file.read_text().strip(),
                current,
                f"stdout:\n{stdout}\nstderr:\n{stderr}",
            )

            restored_state = dict(
                line.split("=", 1)
                for line in (state_root / "current.env").read_text().splitlines()
            )
            self.assertEqual(
                restored_state,
                {
                    "FICANT_DEPLOY_SHA": current,
                    "FICANT_CODE_TREE_SHA": current_tree,
                    "FICANT_STORAGE_RUNTIME_IMAGE": storage_image,
                    "FICANT_STORAGE_RUNTIME_CONFIG_DIGEST": storage_config,
                    "FICANT_SERVER_RUNTIME_IMAGE_DIGEST": restored_server_runtime,
                    "FICANT_WORKER_RUNTIME_IMAGE_DIGEST": restored_worker_runtime,
                    "FICANT_WORKER_NATIVE_SOURCE_DIGEST": restored_worker_source,
                },
            )
            self.assertEqual(old_record.read_text(), '{"status":"old-success"}\n')
            candidate_record = json.loads(
                (deployments / f"{candidate}.json").read_text(encoding="utf-8")
            )
            self.assertEqual(candidate_record["commit_sha"], candidate)
            self.assertEqual(candidate_record["status"], "failed")
            self.assertIs(candidate_record["automatic_rollback"], True)


if __name__ == "__main__":
    unittest.main()
