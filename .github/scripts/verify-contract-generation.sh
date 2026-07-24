#!/usr/bin/env bash

set -euo pipefail

CONTRACT_BASE_SHA=737807302351fe8feee425a89d666caf3d611f96
DESCRIPTOR_SHA256=81cede8c016bea5278d13fda68dd96ed8ba84dede1d83caf25e30367d854f6bf

die() {
  printf 'contract-generation: %s\n' "$1" >&2
  exit 2
}

gate_run_native() {
  local class=$1
  local label=$2
  shift 2
  local rc
  set +e
  "$@"
  rc=$?
  set -e
  if [[ $rc -eq 0 ]]; then
    return 0
  fi
  if [[ $class == finding && $rc -eq 1 ]]; then
    printf 'contract-generation: finding: %s (native exit %s)\n' "$label" "$rc" >&2
    return 1
  fi
  printf 'contract-generation: tool/evidence error: %s (native exit %s)\n' "$label" "$rc" >&2
  return 2
}

tree_digest() {
  python3 - "$1" <<'PY'
import hashlib
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
if not root.is_dir():
    raise SystemExit(2)
h = hashlib.sha256()
files = sorted(path for path in root.rglob("*") if path.is_file())
if not files:
    raise SystemExit(2)
for path in files:
    rel = path.relative_to(root).as_posix().encode()
    data = path.read_bytes()
    h.update(len(rel).to_bytes(8, "big"))
    h.update(rel)
    h.update(len(data).to_bytes(8, "big"))
    h.update(data)
print(h.hexdigest())
PY
}

verify_trees() {
  [[ $# -eq 3 ]] || die '--verify-trees requires generated-a generated-b tracked'
  local a b tracked
  a=$(tree_digest "$1") || die "cannot hash $1"
  b=$(tree_digest "$2") || die "cannot hash $2"
  tracked=$(tree_digest "$3") || die "cannot hash $3"
  if [[ $a != "$b" || $a != "$tracked" ]]; then
    printf 'contract-generation: drift a=%s b=%s tracked=%s\n' "$a" "$b" "$tracked" >&2
    return 1
  fi
}

build_descriptor() {
  [[ $# -eq 2 ]] || die '--build-descriptor requires input output'
  buf build "$1" --as-file-descriptor-set -o "$2"
}

if [[ ${1:-} == '--verify-trees' ]]; then
  shift
  verify_trees "$@"
  exit $?
fi
if [[ ${1:-} == '--build-descriptor' ]]; then
  shift
  build_descriptor "$@"
  exit $?
fi
if [[ ${1:-} == '--map-native' ]]; then
  shift
  [[ $# -ge 2 && ($1 == finding || $1 == tool) ]] || die '--map-native requires finding|tool command [args...]'
  class=$1
  shift
  gate_run_native "$class" 'diagnostic command' "$@"
  exit $?
fi
[[ $# -eq 0 ]] || die 'unexpected arguments'

repo=$(git rev-parse --show-toplevel 2>/dev/null) || die 'not in a Git worktree'
cd "$repo" || die 'cannot enter repository root'
status=$(git status --porcelain) || die 'cannot inspect worktree status'
[[ -z $status ]] || die 'worktree must be clean'
for command in git tar python3 sha256sum awk mktemp mkdir rm cp buf cargo uv corepack; do
  command -v "$command" >/dev/null || die "missing tool: $command"
done
[[ $(buf --version) == '1.56.0' ]] || die 'Buf must be 1.56.0'
[[ $(cargo --version) == cargo\ 1.96.1* ]] || die 'Cargo must be 1.96.1'
[[ $(uv --version) == 'uv 0.7.13' ]] || die 'uv must be 0.7.13'
[[ $(corepack pnpm@10.12.4 --version) == '10.12.4' ]] || die 'pnpm must be 10.12.4'
git cat-file -e "${CONTRACT_BASE_SHA}^{commit}" || die 'missing exact contract baseline commit'

gate_run_native finding 'Buf format drift' buf format --diff --exit-code interface || exit $?
gate_run_native finding 'Buf lint violation' buf lint interface || exit $?
gate_run_native finding 'Buf breaking violation' buf breaking interface --against ".git#ref=${CONTRACT_BASE_SHA},subdir=interface" || exit $?

tmp=$(mktemp -d) || die 'cannot create temporary directory'
trap 'rm -rf "$tmp"' EXIT
gate_run_native tool 'Buf descriptor build' build_descriptor interface "$tmp/descriptor.bin" || exit $?
set +e
actual_descriptor=$(sha256sum "$tmp/descriptor.bin" | awk '{print $1}')
descriptor_rc=$?
set -e
[[ $descriptor_rc -eq 0 && -n $actual_descriptor ]] || die 'cannot hash descriptor evidence'
if [[ $actual_descriptor != "$DESCRIPTOR_SHA256" ]]; then
  printf 'contract-generation: finding: descriptor hash mismatch: %s\n' "$actual_descriptor" >&2
  exit 1
fi

materialize_copy() {
  git archive HEAD | tar -x -C "$1"
}
create_directory() {
  mkdir -p "$1"
}
generate_copy() {
  (cd "$1" && buf generate interface --template interface/buf.gen.yaml)
}
for copy in a b; do
  gate_run_native tool "create temporary tree $copy" create_directory "$tmp/$copy" || exit $?
  gate_run_native tool "materialize tracked tree $copy" materialize_copy "$tmp/$copy" || exit $?
  gate_run_native tool "Buf generation $copy" generate_copy "$tmp/$copy" || exit $?
done

roots=(
  crates/ficant-contracts/src/generated
  python/node-contracts/src/ficant_contracts/generated
  web-dm/packages/contracts-generated/src
)
for root in "${roots[@]}"; do
  verify_trees "$tmp/a/$root" "$tmp/b/$root" "$repo/$root" || exit 1
done

export CARGO_TARGET_DIR="$tmp/cargo-target"
rust_consumer() {
  (cd "$tmp/a" && cargo test --locked -p ficant-contract-tests)
}
python_consumer() {
  (cd "$tmp/a/python" && uv sync --frozen --dev && uv run --frozen pytest -q tests/test_contract_import.py)
}
typescript_consumer() {
  (cd "$tmp/a/web-dm" && \
    corepack pnpm@10.12.4 install --frozen-lockfile --store-dir "$tmp/pnpm-store" && \
    corepack pnpm@10.12.4 typecheck && \
    CI=1 corepack pnpm@10.12.4 --filter @ficant/platform-shell exec vitest run tests/contracts-consumer.test.ts)
}
gate_run_native tool 'Rust contract consumer' rust_consumer || exit $?
gate_run_native tool 'Python contract consumer' python_consumer || exit $?
gate_run_native tool 'TypeScript contract consumer' typescript_consumer || exit $?

printf 'contract-generation: PASS baseline=%s descriptor=%s\n' "$CONTRACT_BASE_SHA" "$DESCRIPTOR_SHA256"
