#!/usr/bin/env bash

set -euo pipefail

failures=()

record_failure() {
  failures+=("$1")
}

require_path() {
  local path=$1
  [[ -e $path ]] || {
    record_failure "missing required path: $path"
    return
  }
  git ls-files --error-unmatch -- "$path" >/dev/null 2>&1 || record_failure "untracked required path: $path"
}

repo_root=$(git rev-parse --show-toplevel 2>/dev/null) || {
  echo "repo-policy: not inside a Git worktree" >&2
  exit 2
}
cd "$repo_root"

required_paths=(
  .gitattributes Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml clippy.toml
  AGENTS.md README.md LICENSE
  scripts/check-fast.ps1 scripts/check.ps1 scripts/check-layering.ps1
  docs/development.md
  binaries/ficant-server/Cargo.toml binaries/ficant-server/src/main.rs
  cpp/fixed-income-kernel/CMakeLists.txt cpp/fixed-income-kernel/include/ficant_kernel.h
  interface/buf.yaml interface/buf.gen.yaml
  interface/proto/ficant/core/v1/common.proto
  interface/proto/ficant/rates/v1/analytics.proto
  python/pyproject.toml python/uv.lock
)

for path in "${required_paths[@]}"; do
  require_path "$path"
done

while IFS= read -r path; do
  [[ -n $path ]] || continue
  base=${path##*/}
  case "$base" in
    .env|.env.*|id_rsa|id_ed25519|*.pem|*.p12|*.pfx|*.key|*.keystore)
      record_failure "secret-like file must not be tracked: $path"
      ;;
  esac
  case "$path" in
    *.go|*.java|*.kt|*.kts|*.cs|*.fs|*.fsx|*.php|*.rb)
      record_failure "forbidden backend language source: $path"
      ;;
  esac
done < <(git -c core.quotepath=false ls-files)

if ((${#failures[@]} > 0)); then
  printf 'repo-policy: FAIL (%d violation(s))\n' "${#failures[@]}" >&2
  printf ' - %s\n' "${failures[@]}" >&2
  exit 1
fi
printf 'repo-policy: PASS\n'
