#!/usr/bin/env bash

set -euo pipefail

repo=$(git rev-parse --show-toplevel)
gate="$repo/.github/scripts/verify-repo-policy.sh"
workflow="$repo/.github/workflows/ci.yml"
toolchain_lock="$repo/deploy/dev/toolchain.lock.toml"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if python3 -c 'pass' >/dev/null 2>&1; then
  python=python3
else
  python=python
fi

"$python" "$repo/.github/scripts/tests/test_release_state_contract.py" -v

expect_fail() {
  local label=$1
  shift
  if "$@" >"$tmp/output" 2>&1; then
    printf 'repo-policy-tests: expected failure: %s\n' "$label" >&2
    exit 1
  fi
}

expect_ignored() {
  local path=$1 expected=$2
  # Match the Ubuntu 24.04 release gate even when this fixture runs on a
  # case-insensitive developer worktree.
  if git -c core.ignoreCase=false check-ignore --no-index -q -- "$path"; then
    actual=true
  else
    actual=false
  fi
  if [[ $actual != "$expected" ]]; then
    printf 'repo-policy-tests: gitignore mismatch path=%s expected=%s actual=%s\n' "$path" "$expected" "$actual" >&2
    exit 1
  fi
}

check_runtime_digest() {
  local candidate=$1
  "$python" - "$candidate" <<'PY'
import pathlib
import re
import sys

text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
match = re.search(r"(?ms)^  business-loop:\n(.*?)(?=^  [a-z0-9-]+:\n|\Z)", text)
if match is None:
    raise SystemExit(1)
block = match.group(0)
comment = "# Derived OCI manifest verified at dbcff347; runtime inputs are unchanged through this SHA; this job does not rebuild it."
assignment = "export FICANT_TEST_RUNTIME_IMAGE_DIGEST=sha256:8e97031468b2ad51ab8484d06d8af9d63f1b73f8c04654f17be40ac629076cd9"
if block.count(comment) != 1 or block.count(assignment) != 1:
    raise SystemExit(1)
if not re.search(rf"(?m)^\s*{re.escape(comment)}\n\s*{re.escape(assignment)}$", block):
    raise SystemExit(1)
PY
}

workflow_job() {
  local candidate=$1 job=$2
  awk -v job="$job" '
    $0 == "  " job ":" { inside=1; print; next }
    inside && $0 ~ /^  [a-z0-9-]+:$/ { exit }
    inside { print }
  ' "$candidate"
}

check_ci_recovery_contracts() {
  local candidate=$1 web rust supply
  web=$(workflow_job "$candidate" web)
  rust=$(workflow_job "$candidate" rust)
  supply=$(workflow_job "$candidate" supply-chain)
  grep -Fq 'corepack enable' <<<"$web" || return 1
  grep -Fq 'corepack prepare pnpm@10.12.4 --activate' <<<"$web" || return 1
  grep -Fq 'cargo test --workspace --locked --exclude ficant-acceptance --exclude ficant-data --exclude ficant-storage' <<<"$rust" || return 1
  grep -Fq -- '--exclude ficant-contract-tests' <<<"$rust" || return 1
  grep -Fq 'cargo test --locked -p ficant-data --test canonical_ingestion' <<<"$rust" || return 1
  grep -Fq 'cargo test --locked -p ficant-data --test snapshot_codec' <<<"$rust" || return 1
  grep -Fq 'cargo test --locked -p ficant-storage --lib' <<<"$rust" || return 1
  local business
  business=$(workflow_job "$candidate" business-loop)
  grep -Fq 'cargo test --locked -p ficant-data --test snapshot_publication_sit -- --test-threads=1' <<<"$business" || return 1
  grep -Fq 'ref: ${{ github.sha }}' <<<"$supply" || return 1
  grep -Fq 'FICANT_TRUSTED_BASE: ${{ github.event.before }}' <<<"$supply" || return 1
  grep -Fq 'FICANT_DEFAULT_BRANCH: ${{ github.event.repository.default_branch }}' <<<"$supply" || return 1
  grep -Fq 'FICANT_EVENT_NAME: ${{ github.event_name }}' <<<"$supply" || return 1
  grep -Fq 'FICANT_REF_NAME: ${{ github.ref_name }}' <<<"$supply" || return 1
  grep -Fq 'FICANT_GATE_OUTPUT_DIR: ${{ runner.temp }}/ficant-supply-evidence' <<<"$supply" || return 1
  grep -Fq 'actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02' <<<"$supply" || return 1
  grep -Fq 'if-no-files-found: error' <<<"$supply" || return 1
  [[ $(grep -Fc '${{ runner.temp }}/ficant-supply-evidence' <<<"$supply") -ge 2 ]]
}

check_version_trigger_contract() {
  local candidate=$1
  "$python" - "$candidate" <<'PY'
import pathlib
import re
import sys

text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
header, separator, jobs = text.partition("\njobs:\n")
if not separator:
    raise SystemExit(1)
if not re.search(r'(?m)^on:\n  push:\n    tags:\n      - "v\*"\n$', header):
    raise SystemExit(1)
if "pull_request:" in header or "workflow_dispatch:" in header:
    raise SystemExit(1)
if "cancel-in-progress: false" not in header:
    raise SystemExit(1)

authorize = re.search(r"(?ms)^  authorize-version:\n(.*?)(?=^  [a-z0-9-]+:\n|\Z)", jobs)
if authorize is None:
    raise SystemExit(1)
for marker in (
    '[[ "${{ github.ref_type }}" == tag ]]',
    '^v[0-9]+\\.[0-9]+\\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$',
    'sha=$(git rev-parse "refs/tags/$version^{commit}")',
    '[[ "$GITHUB_SHA" == "$sha" ]]',
    '[[ "$GITHUB_SHA" == $(git rev-parse origin/main) ]]',
):
    if marker not in authorize.group(0):
        raise SystemExit(1)

for job in (
    "repo-policy", "contract", "rust", "python", "cpp", "web",
    "migration", "business-loop", "supply-chain", "reproducibility",
):
    match = re.search(rf"(?ms)^  {re.escape(job)}:\n(.*?)(?=^  [a-z0-9-]+:\n|\Z)", jobs)
    if match is None or not re.search(r"(?m)^    needs: authorize-version$", match.group(0)):
        raise SystemExit(1)
PY
}

check_ci_source_identity_contract() {
  local candidate=$1
  "$python" - "$candidate" <<'PY'
import pathlib
import re
import sys

text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")


def job(name: str) -> str:
    matches = list(
        re.finditer(
            rf"(?ms)^  {re.escape(name)}:\n.*?(?=^  [a-z0-9-]+:\n|\Z)",
            text,
        )
    )
    if len(matches) != 1:
        raise SystemExit(f"expected one {name} job")
    return matches[0].group(0)


authorize = job("authorize-version")
output_block = """    outputs:
      sha: ${{ steps.candidate.outputs.sha }}
      tree: ${{ steps.candidate.outputs.tree }}
    steps:
"""
if authorize.count(output_block) != 1:
    raise SystemExit("authorize-version must expose candidate sha/tree outputs")

candidate_matches = list(
    re.finditer(r"(?ms)^      - id: candidate\n.*?(?=^      - |\Z)", authorize)
)
if len(candidate_matches) != 1:
    raise SystemExit("authorize-version must have one candidate output step")
candidate = candidate_matches[0].group(0)
candidate_lines = (
    '          sha=$(git rev-parse "refs/tags/$version^{commit}")',
    '          [[ "$sha" =~ ^[0-9a-f]{40}$ ]]',
    '          tree=$(git rev-parse "refs/tags/$version^{tree}")',
    '          [[ "$tree" =~ ^[0-9a-f]{40}$ ]]',
    '          [[ "$GITHUB_SHA" == "$sha" ]]',
    '          [[ "$GITHUB_SHA" == $(git rev-parse origin/main) ]]',
    '          printf \'sha=%s\\n\' "$sha" >> "$GITHUB_OUTPUT"',
    '          printf \'tree=%s\\n\' "$tree" >> "$GITHUB_OUTPUT"',
)
positions = []
for line in candidate_lines:
    if candidate.splitlines().count(line) != 1:
        raise SystemExit(f"missing or duplicate candidate identity line: {line}")
    positions.append(candidate.index(line))
if positions != sorted(positions):
    raise SystemExit("candidate identity must be resolved, validated, authorized, then emitted")
github_output_lines = [line for line in candidate.splitlines() if "GITHUB_OUTPUT" in line]
if github_output_lines != list(candidate_lines[-2:]):
    raise SystemExit("candidate must emit only the validated sha/tree identities")

commit_env = "--env FICANT_CODE_COMMIT_SHA=${{ needs['authorize-version'].outputs.sha }}"
tree_env = "--env FICANT_CODE_TREE_SHA=${{ needs['authorize-version'].outputs.tree }}"
rust_image = "rust@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663"


def require_container_identity(label: str, command: str) -> None:
    for marker in (commit_env, tree_env):
        if command.count(marker) != 1:
            raise SystemExit(f"{label} must receive {marker}")
        if command.index(marker) > command.index(rust_image):
            raise SystemExit(f"{label} identity must be passed to the Rust container")


rust = job("rust")
require_container_identity("rust", rust[rust.index("          docker run --rm\n") :])
if rust.count(commit_env) != 1 or rust.count(tree_env) != 1:
    raise SystemExit("rust identity must not be supplied by a decoy")

web = job("web")
worker_start = web.index("          native_source_digest=$(docker run --rm --network host \\\n")
worker_end = web.index('          [[ "$native_source_digest" =~', worker_start)
server_start = web.index(
    "          docker run --rm --name \"$server_container\" --network host \\\n"
)
server_end = web.index("          server_pid=$!", server_start)
require_container_identity("web worker", web[worker_start:worker_end])
require_container_identity("web server", web[server_start:server_end])
if web.count(commit_env) != 2 or web.count(tree_env) != 2:
    raise SystemExit("web worker/server identities must not be supplied by a decoy")

business = job("business-loop")
runtime = business.index("          export FICANT_TEST_RUNTIME_IMAGE_DIGEST=")
business_start = business.index("          docker run --rm --network host \\\n", runtime)
require_container_identity("business-loop", business[business_start:])
if business.count(commit_env) != 1 or business.count(tree_env) != 1:
    raise SystemExit("business-loop identity must not be supplied by a decoy")

reproducibility = job("reproducibility")
host_env = """    runs-on: ubuntu-24.04
    env:
      FICANT_CODE_COMMIT_SHA: ${{ needs['authorize-version'].outputs.sha }}
      FICANT_CODE_TREE_SHA: ${{ needs['authorize-version'].outputs.tree }}
    steps:
"""
if reproducibility.count(host_env) != 1:
    raise SystemExit("reproducibility must inherit candidate sha/tree in its job environment")
for marker in host_env.splitlines()[2:4]:
    if reproducibility.splitlines().count(marker) != 1:
        raise SystemExit("reproducibility identity must not be supplied by a decoy")

if "safe.directory" in text:
    raise SystemExit("CI source identity must not weaken Git safe-directory checks")
PY
}

check_ci_linux_release_parity_contract() {
  local candidate=$1 candidate_lock=$2
  "$python" - "$candidate" "$candidate_lock" <<'PY'
import pathlib
import re
import sys
import tomllib

workflow_path, lock_path = map(pathlib.Path, sys.argv[1:])
text = workflow_path.read_text(encoding="utf-8")
lock = tomllib.loads(lock_path.read_text(encoding="utf-8"))


def job(name: str) -> str:
    matches = list(
        re.finditer(
            rf"(?ms)^  {re.escape(name)}:\n.*?(?=^  [a-z0-9-]+:\n|\Z)",
            text,
        )
    )
    if len(matches) != 1:
        raise SystemExit(f"expected one {name} job")
    return matches[0].group(0)


buf = lock.get("buf", {})
buf_image = "bufbuild/buf@sha256:89fa92931e7873021a75f8233b27f5a1f59f0397a526d2f8d256dde82e21dc26"
if buf.get("version") != "1.56.0" or buf.get("image") != buf_image:
    raise SystemExit("toolchain lock must pin the reviewed Buf 1.56.0 image")

rust = job("rust")
rust_run_start = rust.index("          docker run --rm\n")
rust_run = rust[rust_run_start:]
rust_image = "rust@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663"
rust_setup_matches = list(
    re.finditer(
        r"(?ms)^      - name: Extract frozen Buf for Rust topology tests\n"
        r".*?(?=^      - |\Z)",
        rust,
    )
)
if len(rust_setup_matches) != 1:
    raise SystemExit("Rust job must have one frozen Buf extraction step")
rust_setup = rust_setup_matches[0].group(0)
expected_rust_setup = """      - name: Extract frozen Buf for Rust topology tests
        shell: bash
        run: |
          set -euo pipefail
          buf_image=$(python3 - <<'PY'
          import tomllib

          with open("deploy/dev/toolchain.lock.toml", "rb") as lock_file:
              lock = tomllib.load(lock_file)
          if lock["buf"]["version"] != "1.56.0":
              raise SystemExit("toolchain lock must pin Buf 1.56.0")
          print(lock["buf"]["image"])
          PY
          )
          [[ "$buf_image" == 'bufbuild/buf@sha256:89fa92931e7873021a75f8233b27f5a1f59f0397a526d2f8d256dde82e21dc26' ]]
          buf_container=ficant-rust-buf-tool
          buf_path="${{ runner.temp }}/ficant-buf"
          trap 'docker rm -f "$buf_container" >/dev/null 2>&1 || true' EXIT
          docker create --name "$buf_container" "$buf_image" >/dev/null
          docker cp "${buf_container}:/usr/local/bin/buf" "$buf_path"
          chmod 0755 "$buf_path"
          [[ "$("$buf_path" --version)" == '1.56.0' ]]
          docker rm "$buf_container" >/dev/null
          trap - EXIT
"""
if rust_setup != expected_rust_setup:
    raise SystemExit("Rust frozen Buf extraction step drifted from its reviewed executable text")
rust_setup_lines = rust_setup.splitlines()
rust_lines = rust.splitlines()
buf_assignment = "          buf_image=$(python3 - <<'PY'"
buf_assertion = f"          [[ \"$buf_image\" == '{buf_image}' ]]"
buf_create = '          docker create --name "$buf_container" "$buf_image" >/dev/null'
buf_copy = '          docker cp "${buf_container}:/usr/local/bin/buf" "$buf_path"'
buf_version = '          [[ "$("$buf_path" --version)" == \'1.56.0\' ]]'
rust_setup_lines_required = (
    buf_assignment,
    '          with open("deploy/dev/toolchain.lock.toml", "rb") as lock_file:',
    '          if lock["buf"]["version"] != "1.56.0":',
    '          print(lock["buf"]["image"])',
    buf_assertion,
    buf_create,
    buf_copy,
    '          chmod 0755 "$buf_path"',
    buf_version,
)
for line in rust_setup_lines_required:
    if rust_setup_lines.count(line) != 1 or rust_lines.count(line) != 1:
        raise SystemExit(f"Rust job missing scoped unique frozen Buf line: {line}")
setup_positions = [rust_setup_lines.index(line) for line in rust_setup_lines_required]
if setup_positions != sorted(setup_positions):
    raise SystemExit("Buf lock, digest, extraction, and version checks must remain ordered")
buf_assignments = re.findall(
    r"(?m)^\s*(?:(?:export|readonly)\s+)?buf_image=", rust_setup
)
if len(buf_assignments) != 1:
    raise SystemExit("validated Buf image must have exactly one assignment")
if len(re.findall(r"(?<![A-Za-z0-9_])buf_image(?![A-Za-z0-9_])", rust_setup)) != 3:
    raise SystemExit("validated Buf image token must appear only in assignment, digest check, and create")
if re.search(r"(?m)^\s*(?:function\s+)?[A-Za-z_][A-Za-z0-9_]*\s*\(\)\s*\{", rust_setup):
    raise SystemExit("Buf extraction step must not hide commands in helper functions")
if rust.count('docker create --name "$buf_container"') != 1:
    raise SystemExit("reviewed Buf container create command must be unique")

rust_container_lines = (
    "          --env FICANT_BUF=/usr/local/bin/buf",
    '          --volume "${{ runner.temp }}/ficant-buf:/usr/local/bin/buf:ro"',
    f"          {rust_image}",
)
for line in rust_container_lines:
    if rust_run.splitlines().count(line) != 1 or rust_lines.count(line) != 1:
        raise SystemExit(f"Rust test container missing scoped unique Buf line: {line}")
    if rust_run.index(line) > rust_run.index(rust_image):
        raise SystemExit("Buf must be injected read-only into the fixed Rust test container")
rust_container_positions = [rust_run.splitlines().index(line) for line in rust_container_lines]
if rust_container_positions != sorted(rust_container_positions):
    raise SystemExit("Buf environment and read-only mount must precede the fixed Rust image")
if rust.count("--env FICANT_BUF=") != 1 or rust.count(":/usr/local/bin/buf:") != 1:
    raise SystemExit("Buf environment and mount must not be supplied by a decoy")

web = job("web")
server_start_marker = '          docker run --rm --name "$server_container" --network host'
server_end_marker = '            sh -ec \'cargo run --locked -p ficant-server\' >"$server_log" 2>&1 &'
server_start = web.index(server_start_marker)
server_end = web.index(server_end_marker, server_start) + len(server_end_marker)
server_run = web[server_start:server_end]
if "--detach" in server_run:
    raise SystemExit("Web server must stay attached to the monitored Docker client process")

required_server_env = (
    "RUSTUP_TOOLCHAIN",
    "FICANT_GRPC_BIND",
    "FICANT_GRPC_WEB_ALLOWED_ORIGINS",
    "FICANT_PLATFORM_SIGNING_KEY_HEX",
    "FICANT_PLATFORM_TRACE_KEY_HEX",
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
    "FICANT_INPUT_FILE_NDJSON_ROOT",
    "FICANT_INPUT_FILE_CONNECTION_BINDING",
    "FICANT_INPUT_POSTGRES_CONNECTION_BINDING",
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
)
for name in required_server_env:
    marker = f"--env {name}="
    if server_run.count(marker) != 1:
        raise SystemExit(f"Web server command must receive {name} exactly once")

server_binding_lines = (
    '            --env FICANT_SERVER_RUNTIME_IMAGE_DIGEST="$server_runtime_digest" \\',
    '            --env FICANT_SERVER_ENVIRONMENT_ATTESTATION="$server_environment_attestation" \\',
    "            --env FICANT_BOOTSTRAP_ACTOR_ID=01J00000000000000000000012 \\",
    "            --env FICANT_BOOTSTRAP_TENANT_ID=01J00000000000000000000010 \\",
    "            --env FICANT_BOOTSTRAP_ALLOWED_OWNER_IDS=01J00000000000000000000011 \\",
    "            --env FICANT_BOOTSTRAP_ACTIVE_ROLE=RESEARCHER \\",
    "            --env FICANT_INPUT_FILE_NDJSON_ROOT=/tmp/ficant-ci-input \\",
    "            --env FICANT_INPUT_FILE_CONNECTION_BINDING=ci-file-ndjson \\",
    "            --env FICANT_INPUT_POSTGRES_CONNECTION_BINDING=ci-postgres \\",
)
server_run_lines = server_run.splitlines()
web_lines = web.splitlines()
for line in server_binding_lines:
    if server_run_lines.count(line) != 1 or web_lines.count(line) != 1:
        raise SystemExit(f"Web server value binding missing, drifted, or decoyed: {line}")
server_env_option_count = len(
    re.findall(r"(?<!\S)--env(?=\s|=)", server_run)
)
if server_env_option_count != len(required_server_env):
    raise SystemExit("Web server command must have exactly the reviewed --env options")
if re.search(r"(?<!\S)(?:-e(?=\s|=)|--env=|--env-file(?=\s|=))", server_run):
    raise SystemExit("Web server command must not use alternate Docker environment options")

server_environment_line = (
    "          server_environment=$'ficant.server.environment.v1\\narch=amd64\\nos=linux\\nprofile=ci'"
)
server_attestation_line = (
    '          server_environment_attestation="sha256:$(printf \'%s\' "$server_environment" | sha256sum | awk \'{print $1}\')"'
)
server_attestation_check = (
    '          [[ "$server_environment_attestation" =~ ^sha256:[0-9a-f]{64}$ ]]'
)
server_runtime_derivation = (
    "          server_runtime_digest=$(docker image inspect --format '{{.Id}}' \\\n"
    f"            {rust_image})"
)
server_runtime_lines = server_runtime_derivation.splitlines()
server_runtime_check = '          [[ "$server_runtime_digest" =~ ^sha256:[0-9a-f]{64}$ ]]'
for marker in (
    server_environment_line,
    server_attestation_line,
    server_attestation_check,
    server_runtime_check,
):
    if web_lines.count(marker) != 1:
        raise SystemExit(f"Web server identity derivation missing or duplicated: {marker}")
for line in server_runtime_lines:
    if web_lines.count(line) != 1:
        raise SystemExit(f"Web server runtime derivation missing or decoyed: {line}")
runtime_positions = [web_lines.index(line) for line in server_runtime_lines]
if runtime_positions != sorted(runtime_positions):
    raise SystemExit("Web server runtime image digest derivation order drifted")
for name in ("server_environment", "server_environment_attestation", "server_runtime_digest"):
    assignments = re.findall(
        rf"(?m)^\s*(?:(?:export|readonly)\s+)?{re.escape(name)}=", web
    )
    if len(assignments) != 1:
        raise SystemExit(f"Web server identity {name} must have exactly one assignment")
expected_identity_tokens = {
    "server_environment": 2,
    "server_environment_attestation": 3,
    "server_runtime_digest": 3,
}
for name, expected_count in expected_identity_tokens.items():
    token_count = len(
        re.findall(
            rf"(?<![A-Za-z0-9_]){re.escape(name)}(?![A-Za-z0-9_])",
            web,
        )
    )
    if token_count != expected_count:
        raise SystemExit(f"Web server identity token count drifted for {name}")

cleanup_start_marker = "          cleanup_web_server() {"
cleanup_start = web.index(cleanup_start_marker)
cleanup_end_marker = "          }\n"
cleanup_end = web.index(cleanup_end_marker, cleanup_start) + len(cleanup_end_marker)
cleanup = web[cleanup_start:cleanup_end]
expected_cleanup = """          cleanup_web_server() {
            status=$?
            trap - EXIT
            if (( status != 0 )); then
              printf 'ficant-ci-grpc log:\\n' >&2
              if [[ -f "$server_log" ]]; then
                cat "$server_log" >&2 || true
              fi
            fi
            docker rm -f "$server_container" >/dev/null 2>&1 || true
            if [[ -n "$server_pid" ]]; then
              kill "$server_pid" >/dev/null 2>&1 || true
              wait "$server_pid" >/dev/null 2>&1 || true
            fi
            rm -f "$server_log" || true
            exit "$status"
          }
"""
if cleanup != expected_cleanup:
    raise SystemExit("Web cleanup drifted from its reviewed executable text")
cleanup_and_trap = "          }\n          trap cleanup_web_server EXIT\n"
if web.count(cleanup_and_trap) != 1:
    raise SystemExit("Web cleanup must close immediately before installing its EXIT trap")
failure_log_block = """            if (( status != 0 )); then
              printf 'ficant-ci-grpc log:\\n' >&2
              if [[ -f "$server_log" ]]; then
                cat "$server_log" >&2 || true
              fi
            fi
"""
if cleanup.count(failure_log_block) != 1:
    raise SystemExit("Web cleanup failure log block drifted or was decoyed")
cleanup_required_lines = (
    cleanup_start_marker,
    "            status=$?",
    "            trap - EXIT",
    "            if (( status != 0 )); then",
    '              if [[ -f "$server_log" ]]; then',
    '                cat "$server_log" >&2 || true',
    '            docker rm -f "$server_container" >/dev/null 2>&1 || true',
    '            if [[ -n "$server_pid" ]]; then',
    '              kill "$server_pid" >/dev/null 2>&1 || true',
    '              wait "$server_pid" >/dev/null 2>&1 || true',
    '            rm -f "$server_log" || true',
    '            exit "$status"',
    "          }",
)
cleanup_lines = cleanup.splitlines()
for line in cleanup_required_lines:
    if cleanup_lines.count(line) != 1:
        raise SystemExit(f"Web cleanup missing scoped unique executable line: {line}")
cleanup_positions = [cleanup_lines.index(line) for line in cleanup_required_lines]
if cleanup_positions != sorted(cleanup_positions):
    raise SystemExit("Web cleanup status, diagnostics, cleanup, and exit order drifted")
if re.search(r"(?m)^\s*return(?:\s|$)", cleanup):
    raise SystemExit("Web cleanup must not return before preserving the test status")
cleanup_body = [line.strip() for line in cleanup_lines[1:-1] if line.strip()]
if cleanup_body[:2] != ["status=$?", "trap - EXIT"] or cleanup_body[-1] != 'exit "$status"':
    raise SystemExit("Web cleanup must capture status first and preserve it at the final exit")
if len(re.findall(r"(?m)^\s*exit(?:\s|$)", cleanup)) != 1:
    raise SystemExit("Web cleanup must contain exactly one final status-preserving exit")
if len(re.findall(r"(?m)^\s*(?:function\s+)?[A-Za-z_][A-Za-z0-9_]*\s*\(\)\s*\{", cleanup)) != 1:
    raise SystemExit("Web cleanup must not hide lifecycle commands in a nested function")
for line in cleanup_required_lines[1:-1]:
    if web_lines.count(line) != 1:
        raise SystemExit(f"Web cleanup executable line must not be supplied by a decoy: {line}")

readiness_start_marker = '          if ! SERVER_PID="$server_pid" python3 - <<\'PY\''
readiness_start = web.index(readiness_start_marker)
readiness_end_marker = "          fi\n"
readiness_end = web.index(readiness_end_marker, readiness_start) + len(readiness_end_marker)
readiness = web[readiness_start:readiness_end]
expected_readiness = """          if ! SERVER_PID="$server_pid" python3 - <<'PY'
          import os
          from pathlib import Path
          import socket
          import time

          server_pid = int(os.environ["SERVER_PID"])

          def server_process_is_running() -> bool:
              try:
                  os.kill(server_pid, 0)
              except ProcessLookupError:
                  return False
              except PermissionError:
                  return True
              try:
                  fields = Path(f"/proc/{server_pid}/stat").read_text(encoding="utf-8").split()
              except OSError:
                  return False
              return len(fields) > 2 and fields[2] != "Z"

          for _ in range(600):
              if not server_process_is_running():
                  raise SystemExit("real gRPC-Web service process exited before readiness")
              with socket.socket() as client:
                  client.settimeout(1.0)
                  if client.connect_ex(("127.0.0.1", 50051)) == 0:
                      if server_process_is_running():
                          break
                      raise SystemExit("real gRPC-Web service process exited during readiness")
              time.sleep(1)
          else:
              raise SystemExit("real gRPC-Web service did not become ready")
          PY
          then
            exit 1
          fi
"""
if readiness != expected_readiness:
    raise SystemExit("Web readiness drifted from its reviewed executable text")
process_contract = """          def server_process_is_running() -> bool:
              try:
                  os.kill(server_pid, 0)
              except ProcessLookupError:
                  return False
              except PermissionError:
                  return True
              try:
                  fields = Path(f"/proc/{server_pid}/stat").read_text(encoding="utf-8").split()
              except OSError:
                  return False
              return len(fields) > 2 and fields[2] != "Z"
"""
if readiness.count(process_contract) != 1:
    raise SystemExit("Web readiness process-state function drifted or was decoyed")
if readiness.count("server_process_is_running()") != 3:
    raise SystemExit("Web readiness process checks must not be duplicated or bypassed")
if readiness.count("return True") != 1:
    raise SystemExit("Web readiness contains an early-success return")
if re.search(r"(?m)^\s*server_process_is_running\s*=", readiness):
    raise SystemExit("Web readiness process checker must not be rebound")
readiness_required_lines = (
    readiness_start_marker,
    '          server_pid = int(os.environ["SERVER_PID"])',
    "          def server_process_is_running() -> bool:",
    "                  os.kill(server_pid, 0)",
    '                  fields = Path(f"/proc/{server_pid}/stat").read_text(encoding="utf-8").split()',
    '              return len(fields) > 2 and fields[2] != "Z"',
    "          for _ in range(600):",
    "              if not server_process_is_running():",
    '                  raise SystemExit("real gRPC-Web service process exited before readiness")',
    '                  if client.connect_ex(("127.0.0.1", 50051)) == 0:',
    "                      if server_process_is_running():",
    "                          break",
    '                      raise SystemExit("real gRPC-Web service process exited during readiness")',
    "              time.sleep(1)",
    "          else:",
    '              raise SystemExit("real gRPC-Web service did not become ready")',
    "          PY",
    "          then",
    "            exit 1",
    "          fi",
)
readiness_lines = readiness.splitlines()
for line in readiness_required_lines:
    if readiness_lines.count(line) != 1:
        raise SystemExit(f"Web readiness missing scoped unique executable line: {line}")
readiness_positions = [readiness_lines.index(line) for line in readiness_required_lines]
if readiness_positions != sorted(readiness_positions):
    raise SystemExit(
        "Web process check, socket readiness, and failure propagation order drifted: "
        + repr(list(zip(readiness_required_lines, readiness_positions)))
    )
if len(re.findall(r"(?m)^\s*def\s+", readiness)) != 1:
    raise SystemExit("Web readiness must not hide the process check in a decoy function")
for line in readiness_required_lines[1:17]:
    if web_lines.count(line) != 1:
        raise SystemExit(f"Web readiness executable line must not be supplied by a decoy: {line}")

web_markers = (
    '          server_log=$(mktemp "$RUNNER_TEMP/ficant-ci-grpc.XXXXXX.log")',
    "          trap cleanup_web_server EXIT",
    '            sh -ec \'cargo run --locked -p ficant-server\' >"$server_log" 2>&1 &',
    "          server_pid=$!",
)
for marker in web_markers:
    if web_lines.count(marker) != 1:
        raise SystemExit(f"Web server lifecycle marker missing or duplicated: {marker}")

ordered = (
    server_environment_line,
    server_attestation_line,
    server_runtime_derivation,
    cleanup_start_marker,
    "          trap cleanup_web_server EXIT",
    server_start_marker,
    "          server_pid=$!",
    'if ! SERVER_PID="$server_pid" python3',
)
positions = [web.index(marker) for marker in ordered]
if positions != sorted(positions):
    raise SystemExit("Web server identity, cleanup, launch, and readiness order drifted")
PY
}

check_contract_node_toolchain() {
  local candidate_workflow=$1 candidate_lock=$2 job=${3:-contract}
  "$python" - "$candidate_workflow" "$candidate_lock" "$job" <<'PY'
import pathlib
import re
import sys

workflow = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
lock = pathlib.Path(sys.argv[2]).read_text(encoding="utf-8")
job = sys.argv[3]
match = re.search(rf"(?ms)^  {re.escape(job)}:\n(.*?)(?=^  [a-z0-9-]+:\n|\Z)", workflow)
if match is None:
    raise SystemExit(1)
contract = match.group(0)
required_workflow = (
    "https://nodejs.org/dist/v22.17.0/node-v22.17.0-linux-x64.tar.xz",
    "325c0f1261e0c61bcae369a1274028e9cfb7ab7949c05512c5b1e630f7e80e12",
    "30482736",
    'export PATH="/tmp/node-v22.17.0-linux-x64/bin:$PATH"',
    'echo \'/tmp/node-v22.17.0-linux-x64/bin\' >> "$GITHUB_PATH"',
    "node --version) == 'v22.17.0'",
    "corepack prepare pnpm@10.12.4 --activate",
)
required_lock = (
    'version = "22.17.0"',
    'url = "https://nodejs.org/dist/v22.17.0/node-v22.17.0-linux-x64.tar.xz"',
    'sha256 = "325c0f1261e0c61bcae369a1274028e9cfb7ab7949c05512c5b1e630f7e80e12"',
    'size = 30482736',
)
if any(marker not in contract for marker in required_workflow):
    raise SystemExit(1)
node_section = re.search(r"(?ms)^\[node\]\n(.*?)(?=^\[|\Z)", lock)
if node_section is None or any(marker not in node_section.group(0) for marker in required_lock):
    raise SystemExit(1)
PY
}

check_phase4_surface_coverage() {
  local candidate=$1
  local required=(
    interface/proto/ficant/research/v1/graph.proto
    interface/proto/ficant/research/v1/execution.proto
    migrations/postgresql/0010_graph_journal_events.sql
    migrations/postgresql/0011_execution_lease_queue.sql
    migrations/postgresql/0012_phase4_execution_closure.sql
    migrations/postgresql/0013_generic_artifact_blob_deduplication.sql
  )
  local path
  for path in "${required[@]}"; do
    grep -Fq -- "$path" "$candidate" || return 1
  done
}

printf '中文证据\n' >"$tmp/chinese.md"
printf 'English only\n' >"$tmp/english.md"
: >"$tmp/empty.md"
printf '\377\376\375' >"$tmp/invalid-utf8.md"
LC_ALL=C.UTF-8 "$gate" --check-chinese "$tmp/chinese.md"
expect_fail "missing Chinese text" env LC_ALL=C.UTF-8 "$gate" --check-chinese "$tmp/english.md"
expect_fail "empty Chinese document" env LC_ALL=C.UTF-8 "$gate" --check-chinese "$tmp/empty.md"
expect_fail "invalid UTF-8" env LC_ALL=C.UTF-8 "$gate" --check-chinese "$tmp/invalid-utf8.md"

"$gate" --check-ci "$workflow"
check_version_trigger_contract "$workflow" || {
  printf 'repo-policy-tests: full CI must require an immutable version tag on current main\n' >&2
  exit 1
}
check_ci_source_identity_contract "$workflow" || {
  printf 'repo-policy-tests: CI source identity authorization/consumer contract missing\n' >&2
  exit 1
}
check_ci_linux_release_parity_contract "$workflow" "$toolchain_lock" || {
  printf 'repo-policy-tests: Linux Rust/Web release parity contract missing\n' >&2
  exit 1
}
check_ci_recovery_contracts "$workflow" || {
  printf 'repo-policy-tests: CI recovery ownership/evidence contract missing\n' >&2
  exit 1
}
check_contract_node_toolchain "$workflow" "$toolchain_lock" || {
  printf 'repo-policy-tests: contract Node toolchain is not frozen to the verified 22.17.0 artifact\n' >&2
  exit 1
}
check_contract_node_toolchain "$workflow" "$toolchain_lock" reproducibility || {
  printf 'repo-policy-tests: reproducibility Node toolchain is not frozen to the verified 22.17.0 artifact\n' >&2
  exit 1
}
check_phase4_surface_coverage "$gate" || {
  printf 'repo-policy-tests: Phase 4 proto/migration surface is not required by the final repository gate\n' >&2
  exit 1
}
cp "$gate" "$tmp/repo-policy-without-phase4"
sed -i '/interface\/proto\/ficant\/research\/v1\/execution\.proto/d' "$tmp/repo-policy-without-phase4"
expect_fail \
  "Phase 4 surface omitted from final gate" \
  check_phase4_surface_coverage "$tmp/repo-policy-without-phase4"
"$python" - "$workflow" "$tmp/ci-recovery" <<'PY'
import pathlib
import sys

source = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
root = pathlib.Path(sys.argv[2])
root.mkdir()
mutations = {
    "no-corepack-activation.yml": ("corepack prepare pnpm@10.12.4 --activate", "true"),
    "rust-reruns-integration.yml": (" --exclude ficant-acceptance --exclude ficant-data --exclude ficant-storage", ""),
    "rust-reruns-contract-tests.yml": (" --exclude ficant-contract-tests", ""),
    "unpinned-upload.yml": ("actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02", "actions/upload-artifact@v4"),
    "unbound-output.yml": ("${{ runner.temp }}/ficant-supply-evidence", "/tmp/unbound-supply-evidence"),
}
for name, (old, new) in mutations.items():
    if old not in source:
        raise SystemExit(f"missing fixture mutation marker: {old}")
    (root / name).write_text(source.replace(old, new), encoding="utf-8")
PY
for candidate in "$tmp"/ci-recovery/*.yml; do
  expect_fail "CI recovery mutation $(basename "$candidate")" check_ci_recovery_contracts "$candidate"
done

"$python" - "$workflow" "$tmp/version-trigger" <<'PY'
import pathlib
import sys

source = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
root = pathlib.Path(sys.argv[2])
root.mkdir()
mutations = {
    "pull-request-trigger.yml": ('      - "v*"', '      - "v*"\n  pull_request:'),
    "movable-main-tag.yml": ('          [[ "$GITHUB_SHA" == $(git rev-parse origin/main) ]]', '          true'),
    "unguarded-job.yml": ('  repo-policy:\n    needs: authorize-version', '  repo-policy:'),
}
for name, (old, new) in mutations.items():
    if old not in source:
        raise SystemExit(f"missing version trigger mutation marker: {old}")
    (root / name).write_text(source.replace(old, new, 1), encoding="utf-8")
PY
for candidate in "$tmp"/version-trigger/*.yml; do
  expect_fail "version trigger mutation $(basename "$candidate")" check_version_trigger_contract "$candidate"
done

"$python" - "$workflow" "$tmp/source-identity" <<'PY'
import pathlib
import sys

source = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
root = pathlib.Path(sys.argv[2])
root.mkdir()


def remove_line(name: str, line: str, occurrence: int = 0, expected: int = 1) -> None:
    lines = source.splitlines(keepends=True)
    matches = [index for index, value in enumerate(lines) if value.rstrip("\r\n") == line]
    if len(matches) != expected:
        raise SystemExit(
            f"identity fixture marker count mismatch for {name}: "
            f"expected {expected}, got {len(matches)}"
        )
    del lines[matches[occurrence]]
    (root / f"{name}.yml").write_text("".join(lines), encoding="utf-8")


unique_lines = {
    "authorize-step-id": "      - id: candidate",
    "authorize-sha-job-output": "      sha: ${{ steps.candidate.outputs.sha }}",
    "authorize-tree-job-output": "      tree: ${{ steps.candidate.outputs.tree }}",
    "authorize-sha-resolution": '          sha=$(git rev-parse "refs/tags/$version^{commit}")',
    "authorize-tree-resolution": '          tree=$(git rev-parse "refs/tags/$version^{tree}")',
    "authorize-sha-validation": '          [[ "$sha" =~ ^[0-9a-f]{40}$ ]]',
    "authorize-tree-validation": '          [[ "$tree" =~ ^[0-9a-f]{40}$ ]]',
    "authorize-tag-head-check": '          [[ "$GITHUB_SHA" == "$sha" ]]',
    "authorize-current-main-check": '          [[ "$GITHUB_SHA" == $(git rev-parse origin/main) ]]',
    "authorize-sha-emission": '          printf \'sha=%s\\n\' "$sha" >> "$GITHUB_OUTPUT"',
    "authorize-tree-emission": '          printf \'tree=%s\\n\' "$tree" >> "$GITHUB_OUTPUT"',
    "rust-sha-consumer": "          --env FICANT_CODE_COMMIT_SHA=${{ needs['authorize-version'].outputs.sha }}",
    "rust-tree-consumer": "          --env FICANT_CODE_TREE_SHA=${{ needs['authorize-version'].outputs.tree }}",
    "reproducibility-sha-consumer": "      FICANT_CODE_COMMIT_SHA: ${{ needs['authorize-version'].outputs.sha }}",
    "reproducibility-tree-consumer": "      FICANT_CODE_TREE_SHA: ${{ needs['authorize-version'].outputs.tree }}",
}
for name, line in unique_lines.items():
    remove_line(name, line)

container_lines = {
    "sha": "            --env FICANT_CODE_COMMIT_SHA=${{ needs['authorize-version'].outputs.sha }} \\",
    "tree": "            --env FICANT_CODE_TREE_SHA=${{ needs['authorize-version'].outputs.tree }} \\",
}
consumer_names = ("web-worker", "web-server", "business-loop")
for identity, line in container_lines.items():
    for occurrence, consumer in enumerate(consumer_names):
        remove_line(
            f"{consumer}-{identity}-consumer",
            line,
            occurrence=occurrence,
            expected=len(consumer_names),
        )
PY
for candidate in "$tmp"/source-identity/*.yml; do
  expect_fail "CI source identity mutation $(basename "$candidate")" \
    check_ci_source_identity_contract "$candidate"
done

"$python" - "$workflow" "$tmp/linux-release-parity" <<'PY'
import pathlib
import re
import sys

source = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
root = pathlib.Path(sys.argv[2])
root.mkdir()


def job_span(name: str) -> re.Match[str]:
    match = re.search(
        rf"(?ms)^  {re.escape(name)}:\n.*?(?=^  [a-z0-9-]+:\n|\Z)",
        source,
    )
    if match is None:
        raise SystemExit(f"missing fixture job: {name}")
    return match


def mutate_job_many(
    name: str, job_name: str, replacements: tuple[tuple[str, str], ...]
) -> None:
    match = job_span(job_name)
    block = match.group(0)
    for old, new in replacements:
        if block.count(old) != 1:
            raise SystemExit(
                f"Linux parity fixture marker count mismatch for {name}: "
                f"expected 1, got {block.count(old)}"
            )
        block = block.replace(old, new, 1)
    candidate = source[: match.start()] + block + source[match.end() :]
    (root / f"{name}.yml").write_text(candidate, encoding="utf-8")


def mutate_job(name: str, job_name: str, old: str, new: str) -> None:
    mutate_job_many(name, job_name, ((old, new),))


rust_mutations = {
    "buf-image-drift": (
        "[[ \"$buf_image\" == 'bufbuild/buf@sha256:89fa92931e7873021a75f8233b27f5a1f59f0397a526d2f8d256dde82e21dc26' ]]",
        "[[ \"$buf_image\" == 'bufbuild/buf@sha256:" + "0" * 64 + "' ]]",
    ),
    "buf-create-image-drift": (
        'docker create --name "$buf_container" "$buf_image" >/dev/null',
        'docker create --name "$buf_container" bufbuild/buf:1.56.0 >/dev/null',
    ),
    "buf-create-comment-decoy": (
        '          docker create --name "$buf_container" "$buf_image" >/dev/null',
        '          docker create --name "$buf_container" bufbuild/buf:1.56.0 >/dev/null\n'
        '          # docker create --name "$buf_container" "$buf_image" >/dev/null',
    ),
    "buf-image-reassigned-after-check": (
        "          [[ \"$buf_image\" == 'bufbuild/buf@sha256:89fa92931e7873021a75f8233b27f5a1f59f0397a526d2f8d256dde82e21dc26' ]]",
        "          [[ \"$buf_image\" == 'bufbuild/buf@sha256:89fa92931e7873021a75f8233b27f5a1f59f0397a526d2f8d256dde82e21dc26' ]]\n"
        "          buf_image=bufbuild/buf:1.56.0",
    ),
    "buf-printf-v-reassigned-after-check": (
        "          [[ \"$buf_image\" == 'bufbuild/buf@sha256:89fa92931e7873021a75f8233b27f5a1f59f0397a526d2f8d256dde82e21dc26' ]]",
        "          [[ \"$buf_image\" == 'bufbuild/buf@sha256:89fa92931e7873021a75f8233b27f5a1f59f0397a526d2f8d256dde82e21dc26' ]]\n"
        "          printf -v buf_image %s bufbuild/buf:1.56.0",
    ),
    "buf-copy-source-drift": (
        'docker cp "${buf_container}:/usr/local/bin/buf" "$buf_path"',
        'docker cp "${buf_container}:/tmp/buf" "$buf_path"',
    ),
    "buf-version-drift": (
        '[[ "$("$buf_path" --version)" == \'1.56.0\' ]]',
        '[[ "$("$buf_path" --version)" == \'1.55.0\' ]]',
    ),
    "buf-env-omitted": (
        "--env FICANT_BUF=/usr/local/bin/buf",
        "--env FICANT_BUF_REMOVED=/usr/local/bin/buf",
    ),
    "buf-mount-writable": (
        'ficant-buf:/usr/local/bin/buf:ro"',
        'ficant-buf:/usr/local/bin/buf:rw"',
    ),
}
for name, (old, new) in rust_mutations.items():
    mutate_job(name, "rust", old, new)

required_web_env = (
    "FICANT_SERVER_RUNTIME_IMAGE_DIGEST",
    "FICANT_SERVER_ENVIRONMENT_ATTESTATION",
    "FICANT_BOOTSTRAP_ACTOR_ID",
    "FICANT_BOOTSTRAP_TENANT_ID",
    "FICANT_BOOTSTRAP_ALLOWED_OWNER_IDS",
    "FICANT_BOOTSTRAP_ACTIVE_ROLE",
    "FICANT_INPUT_FILE_NDJSON_ROOT",
    "FICANT_INPUT_FILE_CONNECTION_BINDING",
    "FICANT_INPUT_POSTGRES_CONNECTION_BINDING",
)
for name in required_web_env:
    mutate_job(
        f"web-{name.lower().replace('_', '-')}-omitted",
        "web",
        f"--env {name}=",
        f"--env FICANT_REMOVED_{name}=",
    )

web_binding_drifts = {
    "server-runtime": (
        '            --env FICANT_SERVER_RUNTIME_IMAGE_DIGEST="$server_runtime_digest" \\',
        "            --env FICANT_SERVER_RUNTIME_IMAGE_DIGEST=sha256:" + "0" * 64 + " \\",
    ),
    "server-attestation": (
        '            --env FICANT_SERVER_ENVIRONMENT_ATTESTATION="$server_environment_attestation" \\',
        "            --env FICANT_SERVER_ENVIRONMENT_ATTESTATION=sha256:" + "0" * 64 + " \\",
    ),
    "bootstrap-actor": (
        "            --env FICANT_BOOTSTRAP_ACTOR_ID=01J00000000000000000000012 \\",
        "            --env FICANT_BOOTSTRAP_ACTOR_ID=01J00000000000000000000099 \\",
    ),
    "bootstrap-tenant": (
        "            --env FICANT_BOOTSTRAP_TENANT_ID=01J00000000000000000000010 \\",
        "            --env FICANT_BOOTSTRAP_TENANT_ID=01J00000000000000000000099 \\",
    ),
    "bootstrap-owner": (
        "            --env FICANT_BOOTSTRAP_ALLOWED_OWNER_IDS=01J00000000000000000000011 \\",
        "            --env FICANT_BOOTSTRAP_ALLOWED_OWNER_IDS=01J00000000000000000000099 \\",
    ),
    "bootstrap-role": (
        "            --env FICANT_BOOTSTRAP_ACTIVE_ROLE=RESEARCHER \\",
        "            --env FICANT_BOOTSTRAP_ACTIVE_ROLE=ADMIN \\",
    ),
    "input-root": (
        "            --env FICANT_INPUT_FILE_NDJSON_ROOT=/tmp/ficant-ci-input \\",
        "            --env FICANT_INPUT_FILE_NDJSON_ROOT=ficant-ci-input \\",
    ),
    "input-file-binding": (
        "            --env FICANT_INPUT_FILE_CONNECTION_BINDING=ci-file-ndjson \\",
        "            --env FICANT_INPUT_FILE_CONNECTION_BINDING=wrong-file-binding \\",
    ),
    "input-postgres-binding": (
        "            --env FICANT_INPUT_POSTGRES_CONNECTION_BINDING=ci-postgres \\",
        "            --env FICANT_INPUT_POSTGRES_CONNECTION_BINDING=wrong-postgres-binding \\",
    ),
}
for name, (old, new) in web_binding_drifts.items():
    mutate_job(f"web-{name}-value-drift", "web", old, new)

web_mutations = {
    "web-server-attestation-drift": (
        "server_environment=$'ficant.server.environment.v1\\narch=amd64\\nos=linux\\nprofile=ci'",
        "server_environment=$'ficant.server.environment.v1\\narch=amd64\\nos=linux\\nprofile=staging'",
    ),
    "web-runtime-not-derived": (
        "server_runtime_digest=$(docker image inspect --format '{{.Id}}'",
        "server_runtime_digest=$(printf 'sha256:%064d' 0 #",
    ),
    "web-runtime-comment-decoy": (
        "          server_runtime_digest=$(docker image inspect --format '{{.Id}}' \\",
        "          server_runtime_digest=sha256:" + "0" * 64 + " # "
        "server_runtime_digest=$(docker image inspect --format '{{.Id}}' \\",
    ),
    "web-runtime-reassigned-after-check": (
        '          [[ "$server_runtime_digest" =~ ^sha256:[0-9a-f]{64}$ ]]',
        '          [[ "$server_runtime_digest" =~ ^sha256:[0-9a-f]{64}$ ]]\n'
        + "          server_runtime_digest=sha256:"
        + "0" * 64,
    ),
    "web-runtime-printf-v-reassigned": (
        '          [[ "$server_runtime_digest" =~ ^sha256:[0-9a-f]{64}$ ]]',
        '          [[ "$server_runtime_digest" =~ ^sha256:[0-9a-f]{64}$ ]]\n'
        + "          printf -v server_runtime_digest %s sha256:"
        + "0" * 64,
    ),
    "web-attestation-comment-decoy": (
        '          server_environment_attestation="sha256:$(printf \'%s\' "$server_environment" | sha256sum | awk \'{print $1}\')"',
        '          server_environment_attestation=sha256:' + "0" * 64
        + ' # server_environment_attestation="sha256:$(printf \'%s\' "$server_environment" | sha256sum | awk \'{print $1}\')"',
    ),
    "web-detached-server": (
        'docker run --rm --name "$server_container" --network host',
        'docker run --detach --rm --name "$server_container" --network host',
    ),
    "web-short-env-override": (
        '            --env FICANT_SERVER_RUNTIME_IMAGE_DIGEST="$server_runtime_digest" \\',
        '            --env FICANT_SERVER_RUNTIME_IMAGE_DIGEST="$server_runtime_digest" \\\n'
        + "            -e FICANT_SERVER_RUNTIME_IMAGE_DIGEST=sha256:"
        + "0" * 64
        + " \\",
    ),
    "web-env-file-override": (
        "            --env FICANT_GRPC_BIND=127.0.0.1:50051 \\",
        "            --env FICANT_GRPC_BIND=127.0.0.1:50051 \\\n"
        "            --env-file /tmp/ficant-ci-override.env \\",
    ),
    "web-server-pid-not-captured": ("          server_pid=$!", "          server_pid="),
    "web-process-check-disabled": (
        "if not server_process_is_running():",
        "if False:",
    ),
    "web-failure-log-hidden": ('cat "$server_log" >&2 || true', "true"),
    "web-failure-log-comment-decoy": (
        '                cat "$server_log" >&2 || true',
        '                true # cat "$server_log" >&2 || true',
    ),
    "web-cleanup-early-return": (
        "          cleanup_web_server() {",
        "          cleanup_web_server() {\n            return 0",
    ),
    "web-cleanup-status-lost": ('exit "$status"', "exit 0"),
    "web-cleanup-trap-omitted": ("          trap cleanup_web_server EXIT", "          true"),
    "web-server-log-discarded": (
        '>"$server_log" 2>&1 &',
        ">/dev/null 2>&1 &",
    ),
    "web-process-check-comment-decoy": (
        "              if not server_process_is_running():",
        "              if False: # if not server_process_is_running():",
    ),
    "web-process-check-function-decoy": (
        "              if not server_process_is_running():\n"
        '                  raise SystemExit("real gRPC-Web service process exited before readiness")',
        "              if False:\n"
        '                  raise SystemExit("real gRPC-Web service process exited before readiness")\n'
        "          def unused_process_check() -> None:\n"
        "              if not server_process_is_running():\n"
        '                  raise SystemExit("real gRPC-Web service process exited before readiness")',
    ),
    "web-readiness-early-success": (
        "          def server_process_is_running() -> bool:",
        "          def server_process_is_running() -> bool:\n              return True",
    ),
    "web-readiness-early-break": (
        "          for _ in range(600):",
        "          for _ in range(600):\n              break",
    ),
}
for name, (old, new) in web_mutations.items():
    mutate_job(name, "web", old, new)

mutate_job_many(
    "web-cleanup-false-wrapper",
    "web",
    (
        (
            "            trap - EXIT\n            if (( status != 0 )); then",
            "            trap - EXIT\n            if false; then\n"
            "            if (( status != 0 )); then",
        ),
        (
            '            rm -f "$server_log" || true\n            exit "$status"',
            '            rm -f "$server_log" || true\n            fi\n            exit "$status"',
        ),
    ),
)
PY
for candidate in "$tmp"/linux-release-parity/*.yml; do
  expect_fail "Linux release parity mutation $(basename "$candidate")" \
    check_ci_linux_release_parity_contract "$candidate" "$toolchain_lock"
done

"$python" - "$toolchain_lock" "$tmp/buf-version-drift.toml" "$tmp/buf-image-drift.toml" <<'PY'
import pathlib
import sys

source = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
version = source.replace('version = "1.56.0"', 'version = "1.55.0"', 1)
image = source.replace(
    "89fa92931e7873021a75f8233b27f5a1f59f0397a526d2f8d256dde82e21dc26",
    "09fa92931e7873021a75f8233b27f5a1f59f0397a526d2f8d256dde82e21dc26",
    1,
)
pathlib.Path(sys.argv[2]).write_text(version, encoding="utf-8")
pathlib.Path(sys.argv[3]).write_text(image, encoding="utf-8")
PY
expect_fail "Buf lock version drift" \
  check_ci_linux_release_parity_contract "$workflow" "$tmp/buf-version-drift.toml"
expect_fail "Buf lock image drift" \
  check_ci_linux_release_parity_contract "$workflow" "$tmp/buf-image-drift.toml"

"$python" - "$workflow" "$toolchain_lock" "$tmp/node-toolchain" <<'PY'
import pathlib
import sys

workflow = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
lock = pathlib.Path(sys.argv[2]).read_text(encoding="utf-8")
root = pathlib.Path(sys.argv[3])
root.mkdir()
workflow_mutations = {
    "runner-node.yml": ("          export PATH=\"/tmp/node-v22.17.0-linux-x64/bin:$PATH\"\n", ""),
    "node-url-drift.yml": ("https://nodejs.org/dist/v22.17.0/node-v22.17.0-linux-x64.tar.xz", "https://nodejs.org/dist/latest/node-linux-x64.tar.xz"),
    "node-size-drift.yml": ("30482736", "30482735"),
    "node-hash-drift.yml": ("325c0f1261e0c61bcae369a1274028e9cfb7ab7949c05512c5b1e630f7e80e12", "025c0f1261e0c61bcae369a1274028e9cfb7ab7949c05512c5b1e630f7e80e12"),
    "node-version-drift.yml": ("node --version) == 'v22.17.0'", "node --version) == 'v22.17.1'"),
}
for name, (old, new) in workflow_mutations.items():
    if old not in workflow:
        raise SystemExit(f"missing Node workflow mutation marker: {old}")
    (root / name).write_text(workflow.replace(old, new), encoding="utf-8")
lock_mutations = {
    "lock-url-drift.toml": ("https://nodejs.org/dist/v22.17.0/node-v22.17.0-linux-x64.tar.xz", "https://nodejs.org/dist/latest/node-linux-x64.tar.xz"),
    "lock-size-drift.toml": ("size = 30482736", "size = 30482735"),
    "lock-hash-drift.toml": ("325c0f1261e0c61bcae369a1274028e9cfb7ab7949c05512c5b1e630f7e80e12", "025c0f1261e0c61bcae369a1274028e9cfb7ab7949c05512c5b1e630f7e80e12"),
    "lock-version-drift.toml": ('version = "22.17.0"', 'version = "22.17.1"'),
}
for name, (old, new) in lock_mutations.items():
    if old not in lock:
        raise SystemExit(f"missing Node lock mutation marker: {old}")
    (root / name).write_text(lock.replace(old, new, 1), encoding="utf-8")
PY
for candidate in "$tmp"/node-toolchain/*.yml; do
  expect_fail "contract Node workflow mutation $(basename "$candidate")" check_contract_node_toolchain "$candidate" "$toolchain_lock"
done
for candidate in "$tmp"/node-toolchain/*.toml; do
  expect_fail "contract Node lock mutation $(basename "$candidate")" check_contract_node_toolchain "$workflow" "$candidate"
done

"$python" - "$workflow" "$tmp/repro-node-toolchain" <<'PY'
import pathlib
import re
import sys

workflow = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
root = pathlib.Path(sys.argv[2])
root.mkdir()
match = re.search(r"(?ms)^  reproducibility:\n(.*?)(?=^  [a-z0-9-]+:\n|\Z)", workflow)
if match is None:
    raise SystemExit("missing reproducibility job")
block = match.group(0)
mutations = {
    "runner-node.yml": ('          export PATH="/tmp/node-v22.17.0-linux-x64/bin:$PATH"\n', ""),
    "node-url-drift.yml": ("https://nodejs.org/dist/v22.17.0/node-v22.17.0-linux-x64.tar.xz", "https://nodejs.org/dist/latest/node-linux-x64.tar.xz"),
    "node-size-drift.yml": ("30482736", "30482735"),
    "node-hash-drift.yml": ("325c0f1261e0c61bcae369a1274028e9cfb7ab7949c05512c5b1e630f7e80e12", "025c0f1261e0c61bcae369a1274028e9cfb7ab7949c05512c5b1e630f7e80e12"),
    "node-version-drift.yml": ("node --version) == 'v22.17.0'", "node --version) == 'v22.17.1'"),
    "pnpm-not-activated.yml": ("corepack prepare pnpm@10.12.4 --activate", "corepack enable"),
}
for name, (old, new) in mutations.items():
    if old not in block:
        raise SystemExit(f"missing Repro Node mutation marker: {old}")
    candidate_block = block.replace(old, new, 1)
    candidate = workflow[:match.start()] + candidate_block + workflow[match.end():]
    (root / name).write_text(candidate, encoding="utf-8")
PY
for candidate in "$tmp"/repro-node-toolchain/*.yml; do
  expect_fail "reproducibility Node workflow mutation $(basename "$candidate")" \
    check_contract_node_toolchain "$candidate" "$toolchain_lock" reproducibility
done

derived_runtime='sha256:8e97031468b2ad51ab8484d06d8af9d63f1b73f8c04654f17be40ac629076cd9'
check_runtime_digest "$workflow" || {
  printf 'repo-policy-tests: verified derived runtime digest missing\n' >&2
  exit 1
}
# Reject, in order: the base image reference, its manifest/config identity,
# representative config and tar digests, and a digest missing the sha256 prefix.
for invalid_runtime in \
  'mcr.microsoft.com/dotnet/runtime-deps@sha256:c62d6267bf8f029da10d716163c274b158f5594b6cc7ee125a08efd64e776df6' \
  'sha256:c62d6267bf8f029da10d716163c274b158f5594b6cc7ee125a08efd64e776df6' \
  'sha256:0000000000000000000000000000000000000000000000000000000000000001' \
  'sha256:1111111111111111111111111111111111111111111111111111111111111111' \
  '8e97031468b2ad51ab8484d06d8af9d63f1b73f8c04654f17be40ac629076cd9'; do
  sed "s|$derived_runtime|$invalid_runtime|" "$workflow" >"$tmp/invalid-runtime.yml"
  expect_fail "invalid runtime digest $invalid_runtime" check_runtime_digest "$tmp/invalid-runtime.yml"
done

"$python" - "$workflow" "$tmp/runtime-comment-global.yml" "$tmp/runtime-comment-migration.yml" <<'PY'
import pathlib
import sys

source = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
comment = "# Derived OCI manifest verified at dbcff347; runtime inputs are unchanged through this SHA; this job does not rebuild it."
without_comment = source.replace(f"          {comment}\n", "", 1)
pathlib.Path(sys.argv[2]).write_text(f"{comment}\n{without_comment}", encoding="utf-8")
wrong_job = without_comment.replace("  migration:\n", f"  migration:\n    {comment}\n", 1)
pathlib.Path(sys.argv[3]).write_text(wrong_job, encoding="utf-8")
PY
expect_fail "runtime provenance outside jobs" check_runtime_digest "$tmp/runtime-comment-global.yml"
expect_fail "runtime provenance in migration job" check_runtime_digest "$tmp/runtime-comment-migration.yml"

"$python" - "$workflow" "$tmp/missing-job.yml" <<'PY'
import pathlib, re, sys
text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
text = re.sub(r"(?ms)^  reproducibility:\n.*?(?=^  [a-z0-9-]+:\n|\Z)", "", text)
pathlib.Path(sys.argv[2]).write_text(text, encoding="utf-8")
PY
expect_fail "missing job" "$gate" --check-ci "$tmp/missing-job.yml"

sed -E '0,/@[0-9a-f]{40}/s//@v4/' "$workflow" >"$tmp/unpinned-action.yml"
expect_fail "unpinned action" "$gate" --check-ci "$tmp/unpinned-action.yml"

sed -E '0,/@sha256:[0-9a-f]{64}/s//:latest/' "$workflow" >"$tmp/unpinned-image.yml"
expect_fail "unpinned image" "$gate" --check-ci "$tmp/unpinned-image.yml"

printf '%s\n' Cargo.toml web-dm/package.json web-dm/pnpm-lock.yaml >"$tmp/safe-paths"
"$gate" --check-path-list "$tmp/safe-paths"
printf '%s\n' \
  AGENTS.md \
  scripts/check-fast.ps1 \
  scripts/check.ps1 \
  docs/development.md \
  docs/architecture/adr/0009-opaid-local-development-and-cicd-release-boundary.md \
  docs/history/hoqa/governance/SKILL.md \
  docs/history/hoqa/governance/references/contracts.md \
  docs/history/hoqa/governance/state.toml \
  docs/history/hoqa/governance/migration-map.md \
  docs/history/hoqa/governance/history/proqaid-superseded/README.md >"$tmp/safe-hoqa-paths"
"$gate" --check-path-list "$tmp/safe-hoqa-paths"
printf '%s\n' \
  .github/scripts/tests/test_release_state_contract.py \
  .github/scripts/verify-cargo-reachability.py \
  .github/scripts/verify-license-inventory.py \
  .github/scripts/verify-risk-acceptance.py \
  docs/history/hoqa/deploy-execution/execution-validator.py \
  tests/oracle/china-rates/validator.py \
  tests/oracle/portfolio/test_r8b_portfolio_performance_decimal_oracle.py \
  tests/phase2b/verify_acceptance_matrix.py \
  tests/phase3/verify_acceptance_matrix.py \
  tests/oracle/china-rates/quantlib_oracle.cpp >"$tmp/safe-python-gate-tools"
"$gate" --check-path-list "$tmp/safe-python-gate-tools"
grep -Fq 'git -c core.quotepath=false ls-files' "$gate" || {
  printf 'repo-policy-tests: tracked Unicode paths must not be C-escaped before validation\n' >&2
  exit 1
}
printf '%s\n' 'docs/中国债券市场量化交易可行性研究.md' >"$tmp/safe-unicode-path"
"$gate" --check-path-list "$tmp/safe-unicode-path"
printf '%s\n' crates/ficant-runtime/src/worker_pool.rs docs/secretary-notes.md docs/cache-policy.md >"$tmp/safe-component-names"
"$gate" --check-path-list "$tmp/safe-component-names"
printf '%s\n' .github/scripts/tests/fixtures/secret/packages.syft.json .github/scripts/tests/fixtures/secret/new-controlled-evidence.json >"$tmp/controlled-secret-fixtures"
"$gate" --check-path-list "$tmp/controlled-secret-fixtures"
expect_ignored .github/scripts/tests/fixtures/secret/packages.syft.json false
expect_ignored .github/scripts/tests/fixtures/secret/new-controlled-evidence.json false
expect_ignored docs/releases/secret/value.json true
expect_ignored docs/releases/Secret/value.json true
expect_ignored docs/releases/secrets/value.json true
expect_ignored .github/scripts/tests/fixtures/Secret/value.json true
expect_ignored docs/secretary-notes.md false

for path in '.hoqa/state.toml' '.hoqa/SKILL.md' 'deploy/execution/run.sh' 'deploy/execution/execution-validator.py' '.proqaid/context.md' '.superpowers/plan.md' 'unknown-root/file.txt' '.env.production' 'keys/release.pem' 'docs/worker-notes.tmp' 'docs/releases/KeY/value.txt' 'docs/releases/SECRET/value.txt' '.github/scripts/tests/fixtures/Secret/value.json' 'docs/releases/Workers/value.txt' 'docs/releases/WorkTree/value.txt' 'docs/releases/CACHE/value.txt' 'docs/releases/.Cache/value.txt'; do
  printf '%s\n' "$path" >"$tmp/unsafe-path"
  expect_fail "unsafe path $path" "$gate" --check-path-list "$tmp/unsafe-path"
done
for path in '.github/scripts/foo.py' 'root-tool.py' 'tests/phase2b/helper.py' 'tests/phase20/verify_acceptance_matrix.py'; do
  printf '%s\n' "$path" >"$tmp/unsafe-python-path"
  expect_fail "unsafe Python path $path" "$gate" --check-path-list "$tmp/unsafe-python-path"
done

clang_url='https://apt.llvm.org/noble/pool/main/l/llvm-toolchain-18/clang-18_18.1.8~++20240731025043+3b5b5c1ec4a3-1~exp1~20240731145144.92_amd64.deb'
grep -Fq "$clang_url" "$workflow" || {
  printf 'repo-policy-tests: frozen Clang URL missing from CI\n' >&2
  exit 1
}
if grep -Eq 'llvm-snapshot\.gpg|apt\.llvm\.org/noble/[[:space:]]|apt-get install.*clang-18' "$workflow"; then
  printf 'repo-policy-tests: dynamic LLVM apt trust is forbidden\n' >&2
  exit 1
fi
"$python" - "$repo/deploy/dev/toolchain.lock.toml" "$workflow" <<'PY'
import pathlib
import sys
import tomllib

lock = tomllib.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))["clang"]
workflow = pathlib.Path(sys.argv[2]).read_text(encoding="utf-8")
expected = {
    "url": "https://apt.llvm.org/noble/pool/main/l/llvm-toolchain-18/clang-18_18.1.8~++20240731025043+3b5b5c1ec4a3-1~exp1~20240731145144.92_amd64.deb",
    "size": 119448,
    "sha256": "6b23c30bd68a86e9485cafd0806d3aa46a812089fba74d8283befa611825b42b",
}
for key, value in expected.items():
    if lock.get(key) != value or str(value) not in workflow:
        raise SystemExit(f"Clang {key} drift between toolchain lock and CI")
PY

"$python" - "$toolchain_lock" "$workflow" "$repo/deploy/dev/RustService.Dockerfile" <<'PY'
import pathlib
import sys
import tomllib

lock_path, workflow_path, dockerfile_path = map(pathlib.Path, sys.argv[1:])
image = tomllib.loads(lock_path.read_text(encoding="utf-8"))["rust"]["image"]
workflow = workflow_path.read_text(encoding="utf-8")
dockerfile = dockerfile_path.read_text(encoding="utf-8")
if workflow.count(image) != 6:
    raise SystemExit(
        "Rust CI image must match the toolchain lock in all six references across four jobs"
    )
if f"ARG RUST_IMAGE={image}" not in dockerfile:
    raise SystemExit("Rust service build image must match the toolchain lock")
if "COPY interface ./interface" not in dockerfile:
    raise SystemExit("Rust service build context must include embedded interface contracts")
PY

printf '%s\n' Cargo.toml web-dm/package.json pnpm-lock.yaml >"$tmp/wrong-lock"
expect_fail "wrong web lock path" "$gate" --check-path-list "$tmp/wrong-lock"

printf 'repo-policy-tests: PASS\n'
