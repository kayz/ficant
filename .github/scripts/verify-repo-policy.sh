#!/usr/bin/env bash

set -uo pipefail

failures=()

record_failure() {
  failures+=("$1")
}

emit_result() {
  local label=$1
  if ((${#failures[@]} > 0)); then
    printf 'repo-policy (%s): FAIL (%d violation(s))\n' "$label" "${#failures[@]}" >&2
    printf ' - %s\n' "${failures[@]}" >&2
    return 1
  fi
  printf 'repo-policy (%s): PASS\n' "$label"
}

require_path() {
  local stage=$1 path=$2
  if [[ ! -e $path ]]; then
    record_failure "missing required ${stage} path: $path"
    return
  fi
  git ls-files --error-unmatch -- "$path" >/dev/null 2>&1 || record_failure "required ${stage} path is not tracked: $path"
}

python_command() {
  if python3 -c 'pass' >/dev/null 2>&1; then
    printf '%s\n' python3
  else
    printf '%s\n' python
  fi
}

has_chinese_text() {
  local document=$1
  local python
  python=$(python_command)
  "$python" - "$document" <<'PY'
import pathlib
import sys

ranges = (
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xF900, 0xFAFF),
    (0x20000, 0x2A6DF),
    (0x2A700, 0x2B73F),
    (0x2B740, 0x2B81F),
    (0x2B820, 0x2CEAF),
    (0x2F800, 0x2FA1F),
)
try:
    text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8", errors="strict")
except (OSError, UnicodeError):
    raise SystemExit(1)
if not any(start <= ord(character) <= end for character in text for start, end in ranges):
    raise SystemExit(1)
PY
}

validate_path_list() {
  local list=$1
  local saw_web_package=false saw_web_lock=false

  while IFS= read -r path; do
    [[ -n $path ]] || continue
    local lower=${path,,}
    local base=${lower##*/}
    local top=${path%%/*}

    if [[ $path == /* || $path == *\\* || $path == ../* || $path == */../* ]]; then
      record_failure "invalid tracked path: $path"
      continue
    fi

    local fixture_secret=false
    [[ $path == .github/scripts/tests/fixtures/secret/* ]] && fixture_secret=true
    IFS='/' read -r -a components <<<"$lower"
    for component in "${components[@]}"; do
      case "$component" in
        .hoqa|.proqaid|.superpowers|.planning|.codex|.claude|hidden|ui-dm|.worktrees|key|keys|secret|secrets|worker|workers|.worker|worktree|worktrees|temp|tmp|cache|.cache|target|build|node_modules|__pycache__|.venv)
          if [[ $component == secret && $fixture_secret == true ]]; then
            continue
          fi
          record_failure "forbidden tracked path component '$component': $path"
          ;;
      esac
    done

    if [[ $path == */* ]]; then
      case "$top" in
        .config|.github|binaries|crates|cpp|deploy|docs|domain-packs|interface|migrations|python|result|scripts|src|tests|web-dm) ;;
        *) record_failure "unknown release top-level directory: $top ($path)" ;;
      esac
    else
      case "$path" in
        .dockerignore|.gitignore|.gitattributes|AGENTS.md|README.md|LICENSE|Cargo.toml|Cargo.lock|rust-toolchain.toml|rustfmt.toml|clippy.toml|cicd.yml|iteration-[0-9]*-checklist.md) ;;
        *) record_failure "unknown release root file: $path" ;;
      esac
    fi

    case "$base" in
      .env|.env.*|id_rsa|id_ed25519|*.pem|*.p12|*.pfx|*.key|*.keystore)
        record_failure "secret-like file must not be tracked: $path"
        ;;
      worker-*|*.worker.*|*.tmp|*.temp|*~)
        record_failure "worker/temporary artifact must not be tracked: $path"
        ;;
    esac

    case "$path" in
      deploy/execution|deploy/execution/*) record_failure "active deploy/execution is forbidden; preserve legacy runner artifacts under docs/history/hoqa/: $path" ;;
      proto|proto/*) record_failure "root proto/ is forbidden; use interface/proto/: $path" ;;
      *.go|*.java|*.kt|*.kts|*.cs|*.fs|*.fsx|*.php|*.rb)
        record_failure "forbidden backend language source: $path"
        ;;
      *.py)
        case "$path" in
          python/*|tests/oracle/china-rates/*|tests/iteration-3/verify_acceptance_matrix.py|tests/phase[0-9]/verify_acceptance_matrix.py|tests/phase[0-9][a-z]/verify_acceptance_matrix.py|docs/history/hoqa/deploy-execution/execution-validator.py|deploy/test/validate_release.py|deploy/verify-storage-runtime.py|.github/scripts/compose_security_gate.py|.github/scripts/tests/test_compose_security_gate.py|.github/scripts/tests/test_license_inventory_bindings.py|.github/scripts/tests/test_storage_runtime_lock.py|.github/scripts/verify-cargo-reachability.py|.github/scripts/verify-license-inventory.py|.github/scripts/verify-risk-acceptance.py) ;;
          *) record_failure "Python is restricted to python/ or the exact CI gate tool allowlist: $path" ;;
        esac
        ;;
      *.c|*.cc|*.cpp|*.cxx|*.h|*.hh|*.hpp|*.hxx)
        [[ $path == cpp/* || $path == tests/oracle/china-rates/quantlib_oracle.cpp ]] || record_failure "C/C++ is restricted to cpp/ or the exact independent Oracle allowlist: $path"
        ;;
      *.js|*.jsx|*.ts|*.tsx)
        [[ $path == web-dm/* ]] || record_failure "JavaScript/TypeScript is restricted to web-dm/: $path"
        ;;
    esac

    [[ $path == web-dm/package.json ]] && saw_web_package=true
    [[ $path == web-dm/pnpm-lock.yaml ]] && saw_web_lock=true
  done <"$list"

  if [[ $saw_web_package == true && $saw_web_lock != true ]]; then
    record_failure "web-dm/package.json requires web-dm/pnpm-lock.yaml"
  fi
}

job_block() {
  local workflow=$1 job=$2
  awk -v job="$job" '
    $0 == "  " job ":" { inside=1; print; next }
    inside && $0 ~ /^  [a-z0-9-]+:$/ { exit }
    inside { print }
  ' "$workflow"
}

require_job_marker() {
  local workflow=$1 job=$2 marker=$3
  local block
  block=$(job_block "$workflow" "$job")
  grep -Fq -- "$marker" <<<"$block" || record_failure "CI job $job must contain: $marker"
}

validate_ci() {
  local workflow=$1
  [[ -f $workflow ]] || {
    record_failure "missing CI workflow: $workflow"
    return
  }

  local expected=(authorize-version business-loop contract cpp migration python repo-policy reproducibility rust supply-chain web)
  mapfile -t actual < <(awk '
    $0 == "jobs:" { inside=1; next }
    inside && $0 ~ /^  [a-z0-9-]+:$/ { value=$0; sub(/^  /, "", value); sub(/:$/, "", value); print value }
  ' "$workflow" | LC_ALL=C sort)
  if [[ ${actual[*]} != "${expected[*]}" ]]; then
    record_failure "CI jobs must be the version authorization job plus exactly ten gates: ${expected[*]}"
  fi

  while IFS= read -r line; do
    [[ $line =~ uses:[[:space:]]*[^@[:space:]]+@([0-9a-f]{40})([[:space:]#]|$) ]] || record_failure "GitHub Action must use a full commit SHA: ${line#*uses:}"
  done < <(grep -E '^[[:space:]]*-[[:space:]]+uses:' "$workflow" || true)

  while IFS= read -r line; do
    [[ $line =~ @sha256:[0-9a-f]{64}([[:space:]#]|$) ]] || record_failure "CI service/container image must use a full RepoDigest: ${line#*image:}"
  done < <(grep -E '^[[:space:]]+image:' "$workflow" || true)

  while IFS= read -r image; do
    [[ $image == *@sha256:* ]] || record_failure "external CI image is not digest-pinned: $image"
  done < <(grep -Eo '(rust|python|node|postgres|quay\.io/ceph/ceph|bufbuild/buf|mcr\.microsoft\.com/[A-Za-z0-9._/-]+)(:[A-Za-z0-9._-]+|@sha256:[0-9a-f]{64})' "$workflow" || true)

  local checkout_count depth_count
  checkout_count=$(grep -Ec 'uses:[[:space:]]*actions/checkout@' "$workflow" || true)
  depth_count=$(grep -Ec 'fetch-depth:[[:space:]]*0([[:space:]#]|$)' "$workflow" || true)
  [[ $checkout_count -eq 11 && $depth_count -eq 11 ]] || record_failure "version authorization and all ten CI gates require checkout with fetch-depth: 0"

  grep -Eq '^  compose-security:$' "$workflow" && record_failure "compose-security must not be an independent CI job"
  grep -Eq 'docker compose .*\b(up|down|ps|start|stop|restart)\b|docker inspect' "$workflow" && record_failure "live Compose/runtime inspection is forbidden in CI"

  require_job_marker "$workflow" repo-policy 'python3 .github/scripts/tests/test_compose_security_gate.py'
  require_job_marker "$workflow" repo-policy 'verify-repo-policy.sh --stage final'
  require_job_marker "$workflow" contract 'verify-contract-generation.sh'
  require_job_marker "$workflow" rust 'cargo test --workspace --locked'
  require_job_marker "$workflow" python 'python/node-runtime/Dockerfile'
  require_job_marker "$workflow" cpp 'ctest --test-dir'
  require_job_marker "$workflow" cpp 'https://apt.llvm.org/noble/pool/main/l/llvm-toolchain-18/clang-18_18.1.8~++20240731025043+3b5b5c1ec4a3-1~exp1~20240731145144.92_amd64.deb'
  require_job_marker "$workflow" cpp '119448'
  require_job_marker "$workflow" cpp '6b23c30bd68a86e9485cafd0806d3aa46a812089fba74d8283befa611825b42b'
  local cpp_block
  cpp_block=$(job_block "$workflow" cpp)
  grep -Eq 'llvm-snapshot\.gpg|apt-get install.*clang-18|apt\.llvm\.org/noble/[[:space:]]' <<<"$cpp_block" && record_failure "CI job cpp must not trust a dynamic LLVM apt repository"
  for marker in 'pnpm@10.12.4 install --frozen-lockfile' 'typecheck' 'build' 'test' 'playwright install' 'test:e2e' 'test:e2e:grpc' 'ficant-server' 'FICANT_GRPC_WEB_BEARER_TOKEN'; do
    require_job_marker "$workflow" web "$marker"
  done
  for job in migration business-loop; do
    require_job_marker "$workflow" "$job" 'postgres@sha256:38471f330eb885e04de130b768d6db4e10469e2311879c7e5c699f6d2d8a1c74'
    require_job_marker "$workflow" "$job" '--test-threads=1'
  done
  require_job_marker "$workflow" business-loop 'deploy/verify-storage-runtime.py verify-lock'
  require_job_marker "$workflow" business-loop 'deploy/storage-runtime.lock.json'
  require_job_marker "$workflow" business-loop 'docker pull "$ceph_image"'
  local business_loop_block
  business_loop_block=$(job_block "$workflow" business-loop)
  grep -Fq 'docker build' <<<"$business_loop_block" && record_failure "business-loop must reuse, not rebuild, the locked storage runtime"
  require_job_marker "$workflow" migration 'migration_acceptance'
  require_job_marker "$workflow" business-loop 'phase1_business_loop'
  require_job_marker "$workflow" business-loop 'negative_invariants'
  require_job_marker "$workflow" authorize-version '[[ "${{ github.ref_type }}" == tag ]]'
  require_job_marker "$workflow" authorize-version '[[ "$GITHUB_SHA" == $(git rev-parse origin/main) ]]'
  require_job_marker "$workflow" supply-chain 'ref: ${{ github.sha }}'
  require_job_marker "$workflow" supply-chain 'FICANT_TRUSTED_BASE: ${{ github.event.before }}'
  require_job_marker "$workflow" supply-chain 'FICANT_DEFAULT_BRANCH: ${{ github.event.repository.default_branch }}'
  require_job_marker "$workflow" supply-chain 'FICANT_EVENT_NAME: ${{ github.event_name }}'
  require_job_marker "$workflow" supply-chain 'FICANT_REF_NAME: ${{ github.ref_name }}'
  require_job_marker "$workflow" supply-chain 'verify-supply-chain.sh'
  require_job_marker "$workflow" reproducibility 'verify-reproducibility.sh'
}

case ${1:-} in
  --check-chinese)
    [[ $# -eq 2 && -f $2 ]] || exit 2
    has_chinese_text "$2" || record_failure "UTF-8 document must contain a CJK codepoint: $2"
    emit_result chinese
    exit $?
    ;;
  --check-ci)
    [[ $# -eq 2 ]] || exit 2
    validate_ci "$2"
    emit_result ci
    exit $?
    ;;
  --check-path-list)
    [[ $# -eq 2 && -f $2 ]] || exit 2
    validate_path_list "$2"
    emit_result paths
    exit $?
    ;;
  --stage)
    [[ $# -eq 2 && ($2 == baseline || $2 == final) ]] || exit 2
    ;;
  *)
    echo "usage: $0 --stage baseline|final | --check-ci FILE | --check-path-list FILE | --check-chinese FILE" >&2
    exit 2
    ;;
esac

stage=$2
repo_root=$(git rev-parse --show-toplevel 2>/dev/null) || {
  echo "repo-policy: not inside a Git worktree" >&2
  exit 2
}
cd "$repo_root" || exit 2

baseline_paths=(
  .gitattributes Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml clippy.toml
  AGENTS.md scripts/check-fast.ps1 scripts/check.ps1 docs/development.md
  docs/architecture/adr/0009-opaid-local-development-and-cicd-release-boundary.md
  docs/history/hoqa/governance/SKILL.md docs/history/hoqa/governance/references/contracts.md
  docs/history/hoqa/governance/state.toml docs/history/hoqa/governance/migration-map.md
  docs/history/hoqa/governance/history/proqaid-superseded/README.md
  docs/history/hoqa/deploy-execution/execution-validator.py
  docs/history/hoqa/iteration-3-checklist.md
  binaries/ficant-bootstrap/Cargo.toml binaries/ficant-bootstrap/src/lib.rs
  binaries/ficant-server/Cargo.toml binaries/ficant-server/src/main.rs
  binaries/ficant-worker/Cargo.toml binaries/ficant-worker/src/main.rs
  binaries/ficant-web/Cargo.toml binaries/ficant-web/src/main.rs
  deploy/dev/docker-compose.yml deploy/dev/Ceph.Dockerfile deploy/dev/ceph-entrypoint.sh
  deploy/dev/config/ficant.toml deploy/dev/toolchain.lock.toml
  .github/workflows/ci.yml .github/scripts/verify-repo-policy.sh
  cpp/fixed-income-kernel/CMakeLists.txt cpp/fixed-income-kernel/include/ficant_kernel.h
  cpp/fixed-income-kernel/src/abi_version.cpp cpp/fixed-income-kernel/tests/abi_smoke.cpp
  python/pyproject.toml python/uv.lock python/node-runtime/Dockerfile
)
final_paths=(
  interface/README.md interface/buf.yaml interface/buf.gen.yaml
  interface/proto/ficant/core/v1/common.proto interface/proto/ficant/core/v1/error.proto
  interface/proto/ficant/market/v1/instrument.proto interface/proto/ficant/market/v1/definition.proto
  interface/proto/ficant/market/v1/fact.proto interface/proto/ficant/market/v1/rule.proto
  interface/proto/ficant/research/v1/snapshot.proto interface/proto/ficant/research/v1/experiment.proto
  interface/proto/ficant/research/v1/artifact.proto interface/proto/ficant/research/v1/signal.proto
  interface/proto/ficant/research/v1/journal.proto interface/proto/ficant/research/v1/graph.proto
  interface/proto/ficant/research/v1/execution.proto interface/proto/ficant/app/v1/registry.proto
  interface/proto/ficant/app/v1/session.proto interface/proto/ficant/rates/v1/analytics.proto
  crates/ficant-contracts/Cargo.toml
  crates/ficant-contract-tests/Cargo.toml crates/ficant-domain/Cargo.toml
  crates/ficant-application/Cargo.toml crates/ficant-runtime/Cargo.toml
  crates/ficant-storage/Cargo.toml crates/ficant-data/Cargo.toml crates/ficant-acceptance/Cargo.toml
  migrations/postgresql/0001_primitives.sql migrations/postgresql/0002_market_definitions.sql
  migrations/postgresql/0003_market_facts.sql migrations/postgresql/0004_research_assets.sql
  migrations/postgresql/0005_run_journal.sql migrations/postgresql/0006_indexes.sql
  migrations/postgresql/0007_independent_signal_artifact.sql
  migrations/postgresql/0008_data_snapshot_manifest_blob.sql migrations/postgresql/0009_data_sources.sql
  migrations/postgresql/0010_graph_journal_events.sql migrations/postgresql/0011_execution_lease_queue.sql
  migrations/postgresql/0012_phase4_execution_closure.sql migrations/postgresql/0013_generic_artifact_blob_deduplication.sql
  tests/golden-cases/china-rates/phase1-business-loop.json
  web-dm/package.json web-dm/pnpm-lock.yaml web-dm/webapps/dmquant/design.md
)

for path in "${baseline_paths[@]}"; do require_path "$stage" "$path"; done
if [[ $stage == final ]]; then
  for path in "${final_paths[@]}"; do require_path "$stage" "$path"; done
  grep -Fq '"crates/*"' Cargo.toml || record_failure "final Cargo Workspace must include crates/*"
fi

tracked=$(mktemp)
trap 'rm -f "$tracked"' EXIT
git ls-files >"$tracked" || exit 2
validate_path_list "$tracked"
validate_ci .github/workflows/ci.yml

[[ ! -e python/pyproject.toml || -e python/uv.lock ]] || record_failure "python/pyproject.toml requires python/uv.lock"
[[ ! -e web-dm/package.json || -e web-dm/pnpm-lock.yaml ]] || record_failure "web-dm/package.json requires web-dm/pnpm-lock.yaml"

while IFS= read -r document; do
  if ! has_chinese_text "$document"; then
    record_failure "required Chinese natural-language document has no Chinese text: $document"
  fi
done < <({ [[ -f interface/README.md ]] && printf '%s\n' interface/README.md; find web-dm -type f -name design.md -print 2>/dev/null || true; } | sort -u)

emit_result "$stage"
