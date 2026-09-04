#!/usr/bin/env bash
set -euo pipefail

root=${FICANT_ROOT:-/srv/ficant-test}
sha=${1:-}
tree=${2:-}
zero_sha=$(printf '0%.0s' {1..40})
zero_digest="sha256:$(printf '00%.0s' {1..32})"
[[ $# -eq 2 && "$sha" =~ ^[0-9a-f]{40}$ && "$tree" =~ ^[0-9a-f]{40}$ ]] \
  || { echo 'Usage: deploy.sh <40-character-commit-sha> <40-character-tree-sha>' >&2; exit 2; }
legacy_rollback=${FICANT_ALLOW_LEGACY_ROLLBACK:-false}
unset FICANT_ALLOW_LEGACY_ROLLBACK
[[ "$legacy_rollback" == false || "$legacy_rollback" == true ]] \
  || { echo 'FICANT_ALLOW_LEGACY_ROLLBACK must be true or false.' >&2; exit 2; }
[[ "$tree" != "$zero_sha" || "$legacy_rollback" == true ]] \
  || { echo 'A zero tree identity is reserved for legacy rollback.' >&2; exit 2; }
[[ "$legacy_rollback" != true || "$tree" == "$zero_sha" ]] \
  || { echo 'Legacy rollback mode requires the zero tree identity.' >&2; exit 2; }
[[ -n ${GHCR_USER:-} ]] || { echo 'GHCR_USER is required.' >&2; exit 2; }
IFS= read -r ghcr_token
[[ -n "$ghcr_token" ]] || { echo 'A GHCR token must be provided on standard input.' >&2; exit 2; }

exec 9>"$root/state/deploy.lock"
flock -n 9 || { echo 'Another deployment is in progress.' >&2; exit 1; }

release_root="$root/releases/$sha"
[[ -d "$release_root/migrations" ]] || { echo "Missing migrations for $sha." >&2; exit 1; }
[[ -f "$root/.env" && -f "$root/compose.test.yml" ]] || { echo 'Server deployment configuration is incomplete.' >&2; exit 1; }

current=''
current_tree=''
current_storage_image=''
current_storage_config=''
current_server_runtime=''
current_runtime=''
current_source=''
if [[ -f "$root/state/current.env" ]]; then
  unset FICANT_CODE_TREE_SHA FICANT_SERVER_RUNTIME_IMAGE_DIGEST
  # shellcheck disable=SC1090
  source "$root/state/current.env"
  current=${FICANT_DEPLOY_SHA:-}
  current_tree=${FICANT_CODE_TREE_SHA:-}
  current_storage_image=${FICANT_STORAGE_RUNTIME_IMAGE:-}
  current_storage_config=${FICANT_STORAGE_RUNTIME_CONFIG_DIGEST:-}
  current_server_runtime=${FICANT_SERVER_RUNTIME_IMAGE_DIGEST:-}
  current_runtime=${FICANT_WORKER_RUNTIME_IMAGE_DIGEST:-}
  current_source=${FICANT_WORKER_NATIVE_SOURCE_DIGEST:-}
  if [[ -z "$current_tree" ]]; then
    current_tree=$zero_sha
  elif [[ ! "$current_tree" =~ ^[0-9a-f]{40}$ ]]; then
    echo 'Current deployment tree state is invalid.' >&2
    exit 1
  fi
  if [[ -z "$current_server_runtime" ]]; then
    current_server_runtime=$zero_digest
  elif [[ ! "$current_server_runtime" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo 'Current Server runtime state is invalid.' >&2
    exit 1
  fi
fi

export FICANT_DEPLOY_SHA=$sha
export FICANT_CODE_COMMIT_SHA=$sha
export FICANT_CODE_TREE_SHA=$tree
storage_image=${FICANT_STORAGE_RUNTIME_IMAGE:-}
storage_config=${FICANT_STORAGE_RUNTIME_CONFIG_DIGEST:-}
[[ "$storage_image" =~ ^ghcr\.io/[a-z0-9_.-]+/[a-z0-9_.-]+@sha256:[0-9a-f]{64}$ ]] \
  || { echo 'FICANT_STORAGE_RUNTIME_IMAGE must be a full immutable GHCR digest reference.' >&2; exit 2; }
[[ "$storage_config" =~ ^sha256:[0-9a-f]{64}$ ]] \
  || { echo 'FICANT_STORAGE_RUNTIME_CONFIG_DIGEST must be canonical.' >&2; exit 2; }
export FICANT_STORAGE_RUNTIME_IMAGE=$storage_image
export FICANT_STORAGE_RUNTIME_CONFIG_DIGEST=$storage_config
export FICANT_SERVER_RUNTIME_IMAGE_DIGEST=$zero_digest
export FICANT_WORKER_RUNTIME_IMAGE_DIGEST=$zero_digest
export FICANT_WORKER_NATIVE_SOURCE_DIGEST=$zero_digest
compose=(docker compose --env-file "$root/.env" --file "$root/compose.test.yml")
printf '%s' "$ghcr_token" | docker login ghcr.io --username "$GHCR_USER" --password-stdin >/dev/null
unset ghcr_token
trap 'docker logout ghcr.io >/dev/null 2>&1 || true' EXIT

verify_storage_runtime() {
  local expected_image=${1:-$storage_image}
  local expected_config=${2:-$storage_config}
  local actual
  local index=${expected_image##*@}
  actual=$(docker image inspect --format '{{.Id}}' "$expected_image") \
    || { echo "Storage runtime is not prepared: $expected_image" >&2; return 1; }
  [[ "$actual" == "$expected_config" || "$actual" == "$index" ]] \
    || { echo "Storage runtime identity mismatch: expected config $expected_config or index $index, got $actual" >&2; return 1; }
  docker image inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "$expected_image" \
    | grep -Fqx "$expected_image" \
    || { echo "Storage runtime RepoDigest is not exact: $expected_image" >&2; return 1; }
}

configure_execution_identity() {
  local deploy_sha=$1
  local deploy_tree=$2
  local allow_legacy=${3:-false}
  local image_prefix=${FICANT_IMAGE_PREFIX:-ghcr.io/kayz/ficant}
  local server_image="$image_prefix-server:sha-$deploy_sha"
  local worker_image="$image_prefix-worker:sha-$deploy_sha"
  local server_runtime
  local worker_runtime
  local source

  [[ "$deploy_sha" =~ ^[0-9a-f]{40}$ && "$deploy_tree" =~ ^[0-9a-f]{40}$ ]] \
    || { echo 'Execution Code identity is not canonical.' >&2; return 1; }
  if ! server_runtime=$(docker image inspect --format '{{.Id}}' "$server_image"); then
    echo "Unable to inspect the Server image: $server_image" >&2
    return 1
  fi
  if [[ ! "$server_runtime" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "Server image has no canonical local digest: $server_image" >&2
    return 1
  fi
  if ! worker_runtime=$(docker image inspect --format '{{.Id}}' "$worker_image"); then
    echo "Unable to inspect the Worker image: $worker_image" >&2
    return 1
  fi
  if [[ ! "$worker_runtime" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "Worker image has no canonical local digest: $worker_image" >&2
    return 1
  fi

  export FICANT_CODE_COMMIT_SHA=$deploy_sha
  export FICANT_CODE_TREE_SHA=$deploy_tree
  export FICANT_SERVER_RUNTIME_IMAGE_DIGEST=$server_runtime
  export FICANT_WORKER_RUNTIME_IMAGE_DIGEST=$worker_runtime
  if ! source=$(docker run --rm --read-only --cap-drop ALL \
    --security-opt no-new-privileges:true --pids-limit 64 --memory 128m \
    "$worker_image" --print-native-source-digest); then
    if [[ "$allow_legacy" == true ]]; then
      export FICANT_WORKER_NATIVE_SOURCE_DIGEST=$zero_digest
      echo "Legacy Worker does not expose source identity; using compatibility placeholders." >&2
      return 0
    fi
    return 1
  fi
  source=${source%%$'\n'*}
  [[ "$source" =~ ^sha256:[0-9a-f]{64}$ ]] \
    || { echo 'Worker native source digest is not canonical.' >&2; return 1; }
  export FICANT_WORKER_NATIVE_SOURCE_DIGEST=$source
}

write_deployment_state() (
  local destination=$1
  local deploy_sha=$2
  local state_tree=$3
  local state_storage_image=$4
  local state_storage_config=$5
  local state_server_runtime=$6
  local state_runtime=$7
  local state_source=$8
  local directory=${destination%/*}
  local filename=${destination##*/}
  local temporary=''

  [[ "$directory" != "$destination" && -d "$directory" ]] || return 1
  temporary=$(mktemp "$directory/.${filename}.tmp.XXXXXX") || return 1
  cleanup_state_temporary() {
    if [[ -n "$temporary" ]]; then
      rm -f -- "$temporary" || true
    fi
  }
  trap cleanup_state_temporary EXIT
  trap 'exit 1' HUP INT TERM

  if ! printf 'FICANT_DEPLOY_SHA=%s\nFICANT_CODE_TREE_SHA=%s\nFICANT_STORAGE_RUNTIME_IMAGE=%s\nFICANT_STORAGE_RUNTIME_CONFIG_DIGEST=%s\nFICANT_SERVER_RUNTIME_IMAGE_DIGEST=%s\nFICANT_WORKER_RUNTIME_IMAGE_DIGEST=%s\nFICANT_WORKER_NATIVE_SOURCE_DIGEST=%s\n' \
    "$deploy_sha" "$state_tree" "$state_storage_image" "$state_storage_config" \
    "$state_server_runtime" "$state_runtime" "$state_source" \
    >"$temporary"; then
    return 1
  fi
  chmod 0600 "$temporary" || return 1
  if ! mv -f -- "$temporary" "$destination"; then
    return 1
  fi
  temporary=''
)

record() {
  local status=$1
  local rollback=$2
  local timestamp
  timestamp=$(date --utc +%Y-%m-%dT%H:%M:%SZ) || return $?
  printf '{"commit_sha":"%s","storage_runtime_image":"%s","storage_runtime_config_digest":"%s","image_prefix":"%s","deployed_at":"%s","status":"%s","automatic_rollback":%s}\n' \
    "$sha" "$storage_image" "$storage_config" "${FICANT_IMAGE_PREFIX:-ghcr.io/kayz/ficant}" "$timestamp" "$status" "$rollback" \
    >"$root/state/deployments/$sha.json" \
    || return $?
}

rollback_current() {
  if [[ "$current" =~ ^[0-9a-f]{40}$ ]]; then
    local rollback_storage_image=$current_storage_image
    local rollback_storage_config=$current_storage_config
    [[ "$rollback_storage_image" =~ ^ghcr\.io/[a-z0-9_.-]+/[a-z0-9_.-]+@sha256:[0-9a-f]{64}$ ]] \
      || { echo 'Current storage runtime image state is invalid.' >&2; return 1; }
    [[ "$rollback_storage_config" =~ ^sha256:[0-9a-f]{64}$ ]] \
      || { echo 'Current storage runtime config state is invalid.' >&2; return 1; }
    echo "Deployment failed; restoring $current." >&2
    export FICANT_DEPLOY_SHA=$current
    export FICANT_CODE_COMMIT_SHA=$current
    export FICANT_CODE_TREE_SHA=$current_tree
    export FICANT_STORAGE_RUNTIME_IMAGE=$rollback_storage_image
    export FICANT_STORAGE_RUNTIME_CONFIG_DIGEST=$rollback_storage_config
    verify_storage_runtime "$rollback_storage_image" "$rollback_storage_config" || return 1
    "${compose[@]}" pull ficant-server ficant-worker ficant-ui || return 1
    allow_legacy=false
    if [[ "$current_tree" == "$zero_sha" ]]; then
      allow_legacy=true
    fi
    configure_execution_identity "$current" "$current_tree" "$allow_legacy" || return 1
    "${compose[@]}" up -d --remove-orphans --wait --wait-timeout 180 postgres ceph-rgw ficant-server ficant-worker ficant-ui || return 1
    FICANT_DEPLOY_SHA=$current "$root/bin/healthcheck.sh" || return 1
    FICANT_DEPLOY_SHA=$current "$root/bin/smoke-test.sh" || return 1
    write_deployment_state \
      "$root/state/current.env" \
      "$current" \
      "$current_tree" \
      "$rollback_storage_image" \
      "$rollback_storage_config" \
      "$FICANT_SERVER_RUNTIME_IMAGE_DIGEST" \
      "$FICANT_WORKER_RUNTIME_IMAGE_DIGEST" \
      "$FICANT_WORKER_NATIVE_SOURCE_DIGEST" \
      || return 1
    record failed true
  else
    echo 'First deployment failed; stopping application containers.' >&2
    "${compose[@]}" stop ficant-server ficant-worker ficant-ui || true
    record failed false
  fi
}

trap 'status=$?; if [[ $status -ne 0 ]]; then rollback_current || true; fi; exit $status' ERR

verify_storage_runtime
"${compose[@]}" pull postgres ficant-server ficant-worker ficant-ui
allow_legacy=false
if [[ "$legacy_rollback" == true ]]; then
  allow_legacy=true
fi
configure_execution_identity "$sha" "$tree" "$allow_legacy"
"${compose[@]}" up -d --wait --wait-timeout 180 postgres ceph-rgw
"${compose[@]}" run --rm migration
"${compose[@]}" up -d --remove-orphans --wait --wait-timeout 180 ficant-server ficant-worker ficant-ui
FICANT_DEPLOY_SHA=$sha "$root/bin/healthcheck.sh"
FICANT_DEPLOY_SHA=$sha "$root/bin/smoke-test.sh"

if [[ "$current" =~ ^[0-9a-f]{40}$ && "$current" != "$sha" ]]; then
  previous_storage_image=$current_storage_image
  [[ "$previous_storage_image" =~ @sha256:[0-9a-f]{64}$ ]] || previous_storage_image=$storage_image
  previous_storage_config=$current_storage_config
  [[ "$previous_storage_config" =~ ^sha256:[0-9a-f]{64}$ ]] || previous_storage_config=$storage_config
  write_deployment_state \
    "$root/state/previous.env" \
    "$current" \
    "$current_tree" \
    "$previous_storage_image" \
    "$previous_storage_config" \
    "$current_server_runtime" \
    "$current_runtime" \
    "$current_source"
fi
write_deployment_state \
  "$root/state/current.env" \
  "$sha" \
  "$tree" \
  "$storage_image" \
  "$storage_config" \
  "$FICANT_SERVER_RUNTIME_IMAGE_DIGEST" \
  "$FICANT_WORKER_RUNTIME_IMAGE_DIGEST" \
  "$FICANT_WORKER_NATIVE_SOURCE_DIGEST"
record success false || {
  status=$?
  rollback_current || true
  exit "$status"
}
trap - ERR
echo "Deployment succeeded: $sha"
