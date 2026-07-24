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
current_storage=''
if [[ -f "$root/state/current.env" ]]; then
  # shellcheck disable=SC1090
  source "$root/state/current.env"
  current=${FICANT_DEPLOY_SHA:-}
  current_storage=${FICANT_STORAGE_SHA:-}
fi

export FICANT_DEPLOY_SHA=$sha
storage_sha=${FICANT_STORAGE_SHA:-$sha}
[[ "$storage_sha" =~ ^[0-9a-f]{40}$ ]] || { echo 'FICANT_STORAGE_SHA must be a 40-character commit SHA.' >&2; exit 2; }
export FICANT_STORAGE_SHA=$storage_sha
compose=(docker compose --env-file "$root/.env" --file "$root/compose.test.yml")
printf '%s' "$ghcr_token" | docker login ghcr.io --username "$GHCR_USER" --password-stdin >/dev/null
unset ghcr_token
trap 'docker logout ghcr.io >/dev/null 2>&1 || true' EXIT

record() {
  local status=$1
  local rollback=$2
  local timestamp
  timestamp=$(date --utc +%Y-%m-%dT%H:%M:%SZ)
  printf '{"commit_sha":"%s","storage_sha":"%s","image_prefix":"%s","deployed_at":"%s","status":"%s","automatic_rollback":%s}\n' \
    "$sha" "$storage_sha" "${FICANT_IMAGE_PREFIX:-ghcr.io/kayz/ficant}" "$timestamp" "$status" "$rollback" \
    >"$root/state/deployments/$sha.json"
}

rollback_current() {
  if [[ "$current" =~ ^[0-9a-f]{40}$ ]]; then
    echo "Deployment failed; restoring $current." >&2
    export FICANT_DEPLOY_SHA=$current
    export FICANT_STORAGE_SHA=$storage_sha
    "${compose[@]}" pull ceph-rgw ficant-server ficant-worker ficant-web ficant-ui
    "${compose[@]}" up -d --remove-orphans --wait --wait-timeout 180 postgres ceph-rgw ficant-server ficant-worker ficant-web ficant-ui
    FICANT_DEPLOY_SHA=$current "$root/bin/healthcheck.sh"
    FICANT_DEPLOY_SHA=$current "$root/bin/smoke-test.sh"
    record failed true
  else
    echo 'First deployment failed; stopping application containers.' >&2
    "${compose[@]}" stop ficant-server ficant-worker ficant-web ficant-ui || true
    record failed false
  fi
}

trap 'status=$?; if [[ $status -ne 0 ]]; then rollback_current || true; fi; exit $status' ERR

"${compose[@]}" pull postgres ceph-rgw ficant-server ficant-worker ficant-web ficant-ui
"${compose[@]}" up -d --wait --wait-timeout 180 postgres ceph-rgw
"${compose[@]}" run --rm migration
"${compose[@]}" up -d --remove-orphans --wait --wait-timeout 180 ficant-server ficant-worker ficant-web ficant-ui
FICANT_DEPLOY_SHA=$sha "$root/bin/healthcheck.sh"
FICANT_DEPLOY_SHA=$sha "$root/bin/smoke-test.sh"

if [[ "$current" =~ ^[0-9a-f]{40}$ && "$current" != "$sha" ]]; then
  previous_storage=$current_storage
  [[ "$previous_storage" =~ ^[0-9a-f]{40}$ ]] || previous_storage=$storage_sha
  printf 'FICANT_DEPLOY_SHA=%s\nFICANT_STORAGE_SHA=%s\n' "$current" "$previous_storage" >"$root/state/previous.env"
fi
printf 'FICANT_DEPLOY_SHA=%s\nFICANT_STORAGE_SHA=%s\n' "$sha" "$storage_sha" >"$root/state/current.env"
record success false
trap - ERR
echo "Deployment succeeded: $sha"
