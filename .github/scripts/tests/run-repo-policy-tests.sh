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
  grep -Fq 'ref: ${{ github.event.pull_request.head.sha || github.sha }}' <<<"$supply" || return 1
  grep -Fq 'FICANT_TRUSTED_BASE: ${{ github.event.pull_request.base.sha || github.event.before }}' <<<"$supply" || return 1
  grep -Fq 'FICANT_DEFAULT_BRANCH: ${{ github.event.repository.default_branch }}' <<<"$supply" || return 1
  grep -Fq 'FICANT_EVENT_NAME: ${{ github.event_name }}' <<<"$supply" || return 1
  grep -Fq 'FICANT_REF_NAME: ${{ github.ref_name }}' <<<"$supply" || return 1
  grep -Fq 'FICANT_GATE_OUTPUT_DIR: ${{ runner.temp }}/ficant-supply-evidence' <<<"$supply" || return 1
  grep -Fq 'actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02' <<<"$supply" || return 1
  grep -Fq 'if-no-files-found: error' <<<"$supply" || return 1
  [[ $(grep -Fc '${{ runner.temp }}/ficant-supply-evidence' <<<"$supply") -ge 2 ]]
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

printf '中文证据\n' >"$tmp/chinese.md"
printf 'English only\n' >"$tmp/english.md"
: >"$tmp/empty.md"
printf '\377\376\375' >"$tmp/invalid-utf8.md"
LC_ALL=C.UTF-8 "$gate" --check-chinese "$tmp/chinese.md"
expect_fail "missing Chinese text" env LC_ALL=C.UTF-8 "$gate" --check-chinese "$tmp/english.md"
expect_fail "empty Chinese document" env LC_ALL=C.UTF-8 "$gate" --check-chinese "$tmp/empty.md"
expect_fail "invalid UTF-8" env LC_ALL=C.UTF-8 "$gate" --check-chinese "$tmp/invalid-utf8.md"

"$gate" --check-ci "$workflow"
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
  .github/scripts/verify-cargo-reachability.py \
  .github/scripts/verify-license-inventory.py \
  .github/scripts/verify-risk-acceptance.py \
  docs/history/hoqa/deploy-execution/execution-validator.py \
  tests/oracle/china-rates/validator.py \
  tests/phase2b/verify_acceptance_matrix.py \
  tests/phase3/verify_acceptance_matrix.py \
  tests/oracle/china-rates/quantlib_oracle.cpp >"$tmp/safe-python-gate-tools"
"$gate" --check-path-list "$tmp/safe-python-gate-tools"
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
if workflow.count(image) != 4:
    raise SystemExit("Rust CI image must match the toolchain lock in all four jobs")
if f"ARG RUST_IMAGE={image}" not in dockerfile:
    raise SystemExit("Rust service build image must match the toolchain lock")
PY

printf '%s\n' Cargo.toml web-dm/package.json pnpm-lock.yaml >"$tmp/wrong-lock"
expect_fail "wrong web lock path" "$gate" --check-path-list "$tmp/wrong-lock"

printf 'repo-policy-tests: PASS\n'
