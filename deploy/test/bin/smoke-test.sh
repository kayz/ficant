#!/usr/bin/env bash
set -euo pipefail

root=${FICANT_ROOT:-/srv/ficant-test}
state_file="$root/state/current.env"

if [[ -z ${FICANT_DEPLOY_SHA:-} ]]; then
  [[ -f "$state_file" ]] || { echo "No current deployment state." >&2; exit 1; }
  # shellcheck disable=SC1090
  source "$state_file"
fi

[[ ${FICANT_DEPLOY_SHA:-} =~ ^[0-9a-f]{40}$ ]] || { echo "Invalid deployment SHA." >&2; exit 1; }

# shellcheck disable=SC1091
source "$root/.env"
server_port=${FICANT_SERVER_PORT:-28080}
timeout 3 bash -c "exec 3<>/dev/tcp/127.0.0.1/$server_port"

echo "Smoke tests passed for $FICANT_DEPLOY_SHA."
