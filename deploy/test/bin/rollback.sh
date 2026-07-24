#!/usr/bin/env bash
set -euo pipefail

root=${FICANT_ROOT:-/srv/ficant-test}
previous_file="$root/state/previous.env"
current_file="$root/state/current.env"
[[ -f "$previous_file" && -f "$current_file" ]] || { echo 'Both current and previous deployment states are required.' >&2; exit 1; }

# shellcheck disable=SC1090
source "$previous_file"
previous=${FICANT_DEPLOY_SHA:-}
previous_storage=${FICANT_STORAGE_SHA:-}
# shellcheck disable=SC1090
source "$current_file"
current=${FICANT_DEPLOY_SHA:-}
current_storage=${FICANT_STORAGE_SHA:-}
[[ "$previous" =~ ^[0-9a-f]{40}$ && "$current" =~ ^[0-9a-f]{40}$ ]] || { echo 'Deployment state is invalid.' >&2; exit 1; }
storage_sha=$previous_storage
[[ "$storage_sha" =~ ^[0-9a-f]{40}$ ]] || storage_sha=$current_storage
[[ "$storage_sha" =~ ^[0-9a-f]{40}$ ]] || { echo 'Storage deployment state is invalid.' >&2; exit 1; }

IFS= read -r token
[[ -n "$token" ]] || { echo 'A GHCR token must be provided on standard input.' >&2; exit 2; }
printf '%s\n' "$token" | FICANT_STORAGE_SHA=$storage_sha GHCR_USER=${GHCR_USER:?GHCR_USER is required} "$root/bin/deploy.sh" "$previous"
