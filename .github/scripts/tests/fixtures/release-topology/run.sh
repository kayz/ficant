#!/usr/bin/env bash

set -euo pipefail

gate=${1:?gate path required}
gitleaks=${2:?gitleaks path required}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

expect_pass() {
  "$gate" --verify-release-fixture "$1" "$2" "$3" "$gitleaks"
}

expect_pass_count() {
  "$gate" --verify-release-fixture "$1" "$2" "$3" "$gitleaks" "$4"
}

expect_fail() {
  local label=$1
  shift
  if "$gate" --verify-release-fixture "$1" "$2" "$3" "$gitleaks" >"$tmp/stdout" 2>"$tmp/stderr"; then
    printf 'release topology fixture unexpectedly passed: %s\n' "$label" >&2
    exit 1
  fi
}

write_test_secret() {
  local name_a='to' name_b='ken'
  local value_a='01234567' value_b='89abcdef' value_c='0123456789ab'
  printf '%s%s=%s%s%s\n' "$name_a" "$name_b" "$value_a" "$value_b" "$value_c"
}

assert_single_generic_hit() {
  local report=$1
  python3 - "$report" <<'PY'
import json,pathlib,sys
findings=json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
if len(findings) != 1 or findings[0].get("RuleID") != "generic-api-key": raise SystemExit(1)
PY
}

expect_gitleaks_git_hit() {
  local label=$1 repo=$2 log_opts=$3 rc
  local report="$tmp/$label-gitleaks.json"
  set +e
  "$gitleaks" git --no-banner --redact --report-format json --report-path "$report" --log-opts="$log_opts" "$repo" >/dev/null 2>&1
  rc=$?
  set -e
  [[ $rc -eq 1 ]] || { printf 'release topology fixture expected Gitleaks git hit: %s\n' "$label" >&2; exit 1; }
  assert_single_generic_hit "$report"
}

expect_gitleaks_dir_hit() {
  local label=$1 repo=$2 rc
  local report="$tmp/$label-gitleaks.json"
  set +e
  "$gitleaks" dir --no-banner --redact --report-format json --report-path "$report" "$repo" >/dev/null 2>&1
  rc=$?
  set -e
  [[ $rc -eq 1 ]] || { printf 'release topology fixture expected Gitleaks dir hit: %s\n' "$label" >&2; exit 1; }
  assert_single_generic_hit "$report"
}

commit_all() {
  local repo=$1 message=$2
  git -C "$repo" add -A
  GIT_AUTHOR_DATE=2026-01-01T00:00:00Z GIT_COMMITTER_DATE=2026-01-01T00:00:00Z \
    git -C "$repo" commit -q -m "$message"
}

init_clean_repo() {
  local repo=$1
  git init -q "$repo"
  git -C "$repo" config user.name 'Supply Fixture'
  git -C "$repo" config user.email 'supply-fixture@example.invalid'
  printf 'root\n' >"$repo/release.txt"
  commit_all "$repo" root
  printf 'published base\n' >>"$repo/release.txt"
  commit_all "$repo" base
}

make_candidate() {
  local repo=$1 value=$2
  printf '%s\n' "$value" >>"$repo/release.txt"
  commit_all "$repo" candidate
}

repo="$tmp/pass"
init_clean_repo "$repo"
base=$(git -C "$repo" rev-parse HEAD)
make_candidate "$repo" clean-candidate
candidate=$(git -C "$repo" rev-parse HEAD)
expect_pass "$repo" "$base" "$candidate"

repo="$tmp/base-drift"
init_clean_repo "$repo"
base=$(git -C "$repo" rev-parse HEAD)
wrong_base=$(git -C "$repo" rev-parse HEAD^)
make_candidate "$repo" drifted-candidate
candidate=$(git -C "$repo" rev-parse HEAD)
expect_fail base-drift "$repo" "$wrong_base" "$candidate"

repo="$tmp/multi-commit"
init_clean_repo "$repo"
base=$(git -C "$repo" rev-parse HEAD)
make_candidate "$repo" first-candidate
make_candidate "$repo" second-candidate
candidate=$(git -C "$repo" rev-parse HEAD)
expect_fail multi-commit "$repo" "$base" "$candidate"
expect_pass_count "$repo" "$base" "$candidate" 2

repo="$tmp/merge-parent"
init_clean_repo "$repo"
base=$(git -C "$repo" rev-parse HEAD)
git -C "$repo" checkout -q -b side-a
printf 'side-a\n' >"$repo/side-a.txt"
commit_all "$repo" side-a
git -C "$repo" checkout -q -b side-b "$base"
printf 'side-b\n' >"$repo/side-b.txt"
commit_all "$repo" side-b
git -C "$repo" merge -q --no-ff side-a -m merge-candidate
candidate=$(git -C "$repo" rev-parse HEAD)
expect_fail merge-parent "$repo" "$base" "$candidate"

repo="$tmp/ancestor-secret"
git init -q "$repo"
git -C "$repo" config user.name 'Supply Fixture'
git -C "$repo" config user.email 'supply-fixture@example.invalid'
write_test_secret >"$repo/release.txt"
commit_all "$repo" ancestor-secret
printf 'published base\n' >"$repo/release.txt"
commit_all "$repo" base-removes-secret
base=$(git -C "$repo" rev-parse HEAD)
make_candidate "$repo" clean-candidate
candidate=$(git -C "$repo" rev-parse HEAD)
expect_gitleaks_git_hit ancestor-secret "$repo" "$base"
expect_fail ancestor-secret "$repo" "$base" "$candidate"

repo="$tmp/range-secret"
init_clean_repo "$repo"
base=$(git -C "$repo" rev-parse HEAD)
make_candidate "$repo" "$(write_test_secret)"
candidate=$(git -C "$repo" rev-parse HEAD)
expect_gitleaks_git_hit range-secret "$repo" "$base..$candidate"
expect_fail range-secret "$repo" "$base" "$candidate"

repo="$tmp/tree-secret"
init_clean_repo "$repo"
base=$(git -C "$repo" rev-parse HEAD)
make_candidate "$repo" clean-candidate
candidate=$(git -C "$repo" rev-parse HEAD)
write_test_secret >>"$repo/release.txt"
expect_gitleaks_dir_hit tree-secret "$repo"
expect_fail tree-secret "$repo" "$base" "$candidate"

repo="$tmp/missing-base"
init_clean_repo "$repo"
make_candidate "$repo" clean-candidate
candidate=$(git -C "$repo" rev-parse HEAD)
expect_fail missing-base "$repo" 0000000000000000000000000000000000000000 "$candidate"

printf 'release topology fixtures: PASS\n'
