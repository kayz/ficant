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
CICD = REPO / "cicd.yml"
CI_WORKFLOW = REPO / ".github/workflows/ci.yml"


def deployment_state_function(source: str) -> str:
    match = re.search(
        r"(?ms)^write_deployment_state\(\) \(\n.*?^\)\n",
        source,
    )
    if match is None:
        raise AssertionError("write_deployment_state must remain a subshell function")
    return match.group(0)


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
  ghcr.io/kayz/ficant-ceph-rgw@sha256:{'b' * 64} \
  sha256:{'c' * 64} \
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
                    "FICANT_STORAGE_RUNTIME_IMAGE="
                    f"ghcr.io/kayz/ficant-ceph-rgw@sha256:{'b' * 64}",
                    f"FICANT_STORAGE_RUNTIME_CONFIG_DIGEST=sha256:{'c' * 64}",
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
  ghcr.io/kayz/ficant-ceph-rgw@sha256:{'b' * 64} \
  sha256:{'c' * 64} \
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


if __name__ == "__main__":
    unittest.main()
