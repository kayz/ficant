#!/usr/bin/env bash
set -euo pipefail

root=${FICANT_ROOT:-/srv/ficant-test}
sha=${1:-}
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo 'Usage: deploy.sh <40-character-commit-sha>' >&2; exit 2; }
[[ -n ${GHCR_USER:-} ]] || { echo 'GHCR_USER is required.' >&2; exit 2; }
IFS= read -r ghcr_token
[[ -n "$ghcr_token" ]] || { echo 'A GHCR token must be provided on standard input.' >&2; exit 2; }

exec 9>"$root/state/deploy.lock"
flock -n 9 || { echo 'Another deployment is in progress.' >&2; exit 1; }

release_root="$root/releases/$sha"
[[ -d "$release_root/migrations" ]] || { echo "Missing migrations for $sha." >&2; exit 1; }
[[ -f "$root/.env" && -f "$root/compose.test.yml" ]] || { echo 'Server deployment configuration is incomplete.' >&2; exit 1; }

current=''
current_storage_image=''
current_storage_config=''
current_runtime=''
current_source=''
if [[ -f "$root/state/current.env" ]]; then
  # shellcheck disable=SC1090
  source "$root/state/current.env"
  current=${FICANT_DEPLOY_SHA:-}
  current_storage_image=${FICANT_STORAGE_RUNTIME_IMAGE:-}
  current_storage_config=${FICANT_STORAGE_RUNTIME_CONFIG_DIGEST:-}
  current_runtime=${FICANT_WORKER_RUNTIME_IMAGE_DIGEST:-}
  current_source=${FICANT_WORKER_NATIVE_SOURCE_DIGEST:-}
fi

export FICANT_DEPLOY_SHA=$sha
storage_image=${FICANT_STORAGE_RUNTIME_IMAGE:-}
storage_config=${FICANT_STORAGE_RUNTIME_CONFIG_DIGEST:-}
[[ "$storage_image" =~ ^ghcr\.io/[a-z0-9_.-]+/[a-z0-9_.-]+@sha256:[0-9a-f]{64}$ ]] \
  || { echo 'FICANT_STORAGE_RUNTIME_IMAGE must be a full immutable GHCR digest reference.' >&2; exit 2; }
[[ "$storage_config" =~ ^sha256:[0-9a-f]{64}$ ]] \
  || { echo 'FICANT_STORAGE_RUNTIME_CONFIG_DIGEST must be canonical.' >&2; exit 2; }
export FICANT_STORAGE_RUNTIME_IMAGE=$storage_image
export FICANT_STORAGE_RUNTIME_CONFIG_DIGEST=$storage_config
export FICANT_WORKER_RUNTIME_IMAGE_DIGEST="sha256:$(printf '00%.0s' {1..32})"
export FICANT_WORKER_NATIVE_SOURCE_DIGEST="sha256:$(printf '00%.0s' {1..32})"
compose=(docker compose --env-file "$root/.env" --file "$root/compose.test.yml")
printf '%s' "$ghcr_token" | docker login ghcr.io --username "$GHCR_USER" --password-stdin >/dev/null
unset ghcr_token
trap 'docker logout ghcr.io >/dev/null 2>&1 || true' EXIT

verify_storage_runtime() {
  local actual
  local index=${storage_image##*@}
  actual=$(docker image inspect --format '{{.Id}}' "$storage_image") \
    || { echo "Storage runtime is not prepared: $storage_image" >&2; return 1; }
  [[ "$actual" == "$storage_config" || "$actual" == "$index" ]] \
    || { echo "Storage runtime identity mismatch: expected config $storage_config or index $index, got $actual" >&2; return 1; }
  docker image inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "$storage_image" \
    | grep -Fqx "$storage_image" \
    || { echo "Storage runtime RepoDigest is not exact: $storage_image" >&2; return 1; }
}

configure_execution_identity() {
  local deploy_sha=$1
  local allow_legacy=${2:-false}
  local image="${FICANT_IMAGE_PREFIX:-ghcr.io/kayz/ficant}-worker:sha-$deploy_sha"
  local runtime
  local source
  runtime=$(docker image inspect --format '{{.Id}}' "$image")
  if [[ ! "$runtime" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "Worker image has no canonical local digest: $image" >&2
    return 1
  fi
  if ! source=$(docker run --rm --read-only --cap-drop ALL \
    --security-opt no-new-privileges:true --pids-limit 64 --memory 128m \
    "$image" --print-native-source-digest); then
    if [[ "$allow_legacy" == true ]]; then
      echo "Legacy Worker does not expose source identity; using compatibility placeholders." >&2
      return 0
    fi
    return 1
  fi
  source=${source%%$'\n'*}
  [[ "$source" =~ ^sha256:[0-9a-f]{64}$ ]] \
    || { echo 'Worker native source digest is not canonical.' >&2; return 1; }
  export FICANT_WORKER_RUNTIME_IMAGE_DIGEST=$runtime
  export FICANT_WORKER_NATIVE_SOURCE_DIGEST=$source
}

write_deployment_state() (
  local destination=$1
  local deploy_sha=$2
  local state_storage_image=$3
  local state_storage_config=$4
  local state_runtime=$5
  local state_source=$6
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

  if ! printf 'FICANT_DEPLOY_SHA=%s\nFICANT_STORAGE_RUNTIME_IMAGE=%s\nFICANT_STORAGE_RUNTIME_CONFIG_DIGEST=%s\nFICANT_WORKER_RUNTIME_IMAGE_DIGEST=%s\nFICANT_WORKER_NATIVE_SOURCE_DIGEST=%s\n' \
    "$deploy_sha" "$state_storage_image" "$state_storage_config" "$state_runtime" "$state_source" \
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
  timestamp=$(date --utc +%Y-%m-%dT%H:%M:%SZ)
  printf '{"commit_sha":"%s","storage_runtime_image":"%s","storage_runtime_config_digest":"%s","image_prefix":"%s","deployed_at":"%s","status":"%s","automatic_rollback":%s}\n' \
    "$sha" "$storage_image" "$storage_config" "${FICANT_IMAGE_PREFIX:-ghcr.io/kayz/ficant}" "$timestamp" "$status" "$rollback" \
    >"$root/state/deployments/$sha.json"
}

rollback_current() {
  if [[ "$current" =~ ^[0-9a-f]{40}$ ]]; then
    echo "Deployment failed; restoring $current." >&2
    export FICANT_DEPLOY_SHA=$current
    verify_storage_runtime
    "${compose[@]}" pull ficant-server ficant-worker ficant-ui
    configure_execution_identity "$current" true
    "${compose[@]}" up -d --remove-orphans --wait --wait-timeout 180 postgres ceph-rgw ficant-server ficant-worker ficant-ui
    FICANT_DEPLOY_SHA=$current "$root/bin/healthcheck.sh"
    FICANT_DEPLOY_SHA=$current "$root/bin/smoke-test.sh"
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
configure_execution_identity "$sha"
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
    "$previous_storage_image" \
    "$previous_storage_config" \
    "$current_runtime" \
    "$current_source"
fi
write_deployment_state \
  "$root/state/current.env" \
  "$sha" \
  "$storage_image" \
  "$storage_config" \
  "$FICANT_WORKER_RUNTIME_IMAGE_DIGEST" \
  "$FICANT_WORKER_NATIVE_SOURCE_DIGEST"
record success false
trap - ERR
echo "Deployment succeeded: $sha"
