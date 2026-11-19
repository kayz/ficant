#!/usr/bin/env bash
set -euo pipefail

root=${FICANT_ROOT:-/srv/ficant-test}
previous_file="$root/state/previous.env"
current_file="$root/state/current.env"
[[ -f "$previous_file" && -f "$current_file" ]] || { echo 'Both current and previous deployment states are required.' >&2; exit 1; }

# shellcheck disable=SC1090
source "$previous_file"
previous=${FICANT_DEPLOY_SHA:-}
previous_tree=${FICANT_CODE_TREE_SHA:-}
[[ "$previous" =~ ^[0-9a-f]{40}$ && "$previous_tree" =~ ^[0-9a-f]{40}$ ]] || { echo 'Previous deployment state is invalid.' >&2; exit 1; }

IFS= read -r token
[[ -n "$token" ]] || { echo 'A GHCR token must be provided on standard input.' >&2; exit 2; }
printf '%s\n' "$token" | GHCR_USER=${GHCR_USER:?GHCR_USER is required} "$root/bin/deploy.sh" "$previous" "$previous_tree"
