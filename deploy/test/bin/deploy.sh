#!/usr/bin/env bash
set -euo pipefail

root=${FICANT_ROOT:-/srv/ficant-test}
sha=${1:-}
tree=${2:-}
[[ $# -eq 2 && "$sha" =~ ^[0-9a-f]{40}$ && "$tree" =~ ^[0-9a-f]{40}$ ]] \
  || { echo 'Usage: deploy.sh <40-character-commit-sha> <40-character-tree-sha>' >&2; exit 2; }
[[ -n ${GHCR_USER:-} ]] || { echo 'GHCR_USER is required.' >&2; exit 2; }
IFS= read -r ghcr_token
[[ -n "$ghcr_token" ]] || { echo 'A GHCR token must be provided on standard input.' >&2; exit 2; }

mkdir -p "$root/state/deployments"
exec 9>"$root/state/deploy.lock"
flock -n 9 || { echo 'Another deployment is in progress.' >&2; exit 1; }
[[ -f "$root/.env" && -f "$root/compose.test.yml" ]] || { echo 'Server deployment configuration is incomplete.' >&2; exit 1; }

current=''
current_tree=''
if [[ -f "$root/state/current.env" ]]; then
  # shellcheck disable=SC1090
  source "$root/state/current.env"
  current=${FICANT_DEPLOY_SHA:-}
  current_tree=${FICANT_CODE_TREE_SHA:-}
fi

export FICANT_DEPLOY_SHA=$sha
export FICANT_CODE_TREE_SHA=$tree
compose=(docker compose --env-file "$root/.env" --file "$root/compose.test.yml")
printf '%s' "$ghcr_token" | docker login ghcr.io --username "$GHCR_USER" --password-stdin >/dev/null
unset ghcr_token
trap 'docker logout ghcr.io >/dev/null 2>&1 || true' EXIT

write_state() (
  local destination=$1
  local deploy_sha=$2
  local state_tree=$3
  local directory=${destination%/*}
  local filename=${destination##*/}
  local temporary
  temporary=$(mktemp "$directory/.${filename}.tmp.XXXXXX")
  trap 'rm -f -- "$temporary" || true' EXIT
  printf 'FICANT_DEPLOY_SHA=%s\nFICANT_CODE_TREE_SHA=%s\n' "$deploy_sha" "$state_tree" >"$temporary"
  chmod 0600 "$temporary"
  mv -f -- "$temporary" "$destination"
  trap - EXIT
)

rollback_current() {
  if [[ "$current" =~ ^[0-9a-f]{40}$ && "$current_tree" =~ ^[0-9a-f]{40}$ ]]; then
    echo "Deployment failed; restoring $current." >&2
    export FICANT_DEPLOY_SHA=$current
    export FICANT_CODE_TREE_SHA=$current_tree
    "${compose[@]}" pull ficant-server || return 1
    "${compose[@]}" up -d --remove-orphans --wait --wait-timeout 60 ficant-server || return 1
    FICANT_DEPLOY_SHA=$current "$root/bin/healthcheck.sh" || return 1
    FICANT_DEPLOY_SHA=$current "$root/bin/smoke-test.sh" || return 1
  else
    "${compose[@]}" stop ficant-server || true
  fi
}

trap 'status=$?; if [[ $status -ne 0 ]]; then rollback_current || true; fi; exit $status' ERR

"${compose[@]}" pull ficant-server
"${compose[@]}" up -d --remove-orphans --wait --wait-timeout 60 ficant-server
FICANT_DEPLOY_SHA=$sha "$root/bin/healthcheck.sh"
FICANT_DEPLOY_SHA=$sha "$root/bin/smoke-test.sh"

if [[ "$current" =~ ^[0-9a-f]{40}$ && "$current_tree" =~ ^[0-9a-f]{40}$ && "$current" != "$sha" ]]; then
  write_state "$root/state/previous.env" "$current" "$current_tree"
fi
write_state "$root/state/current.env" "$sha" "$tree"
trap - ERR
echo "Deployment succeeded: $sha"
