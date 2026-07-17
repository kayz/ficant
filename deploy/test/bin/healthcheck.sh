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
export FICANT_DEPLOY_SHA

compose=(docker compose --env-file "$root/.env" --file "$root/compose.test.yml")
for service in postgres ficant-server ficant-worker ficant-web ficant-ui; do
  container_id=$("${compose[@]}" ps --quiet "$service")
  [[ -n "$container_id" ]] || { echo "$service has no container." >&2; exit 1; }
  status=$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$container_id")
  [[ "$status" == healthy ]] || { echo "$service is $status." >&2; exit 1; }
done

echo "Health checks passed for $FICANT_DEPLOY_SHA."
