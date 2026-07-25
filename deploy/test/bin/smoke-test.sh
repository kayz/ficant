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
[[ ${FICANT_STORAGE_RUNTIME_IMAGE:-} =~ @sha256:[0-9a-f]{64}$ ]] \
  || { echo "Invalid storage runtime image." >&2; exit 1; }
export FICANT_STORAGE_RUNTIME_IMAGE

# shellcheck disable=SC1091
source "$root/.env"
server_port=${FICANT_SERVER_PORT:-28080}
worker_port=${FICANT_WORKER_PORT:-28081}
web_port=${FICANT_WEB_PORT:-28082}
ui_port=${FICANT_UI_PORT:-28083}

timeout 3 bash -c "exec 3<>/dev/tcp/127.0.0.1/$server_port"
[[ $(curl --fail --silent --show-error "http://127.0.0.1:$worker_port/worker-ready") == ok ]]
[[ $(curl --fail --silent --show-error "http://127.0.0.1:$web_port/web-ready") == ok ]]
ui_html=$(curl --fail --silent --show-error "http://127.0.0.1:$ui_port/ficant/")
[[ "$ui_html" == *'<div id="root">'* ]] || { echo "FICANT UI root marker is missing." >&2; exit 1; }

compose=(docker compose --env-file "$root/.env" --file "$root/compose.test.yml")
expected_file=$(mktemp)
applied_file=$(mktemp)
trap 'rm -f "$expected_file" "$applied_file"' EXIT
find "$root/releases/$FICANT_DEPLOY_SHA/migrations" -maxdepth 1 -type f -name '*.sql' -printf '%f\n' | LC_ALL=C sort >"$expected_file"
"${compose[@]}" exec -T postgres psql -U ficant -d ficant -At \
  -c 'SELECT version FROM public.ficant_schema_migrations ORDER BY version' \
  | LC_ALL=C sort >"$applied_file"
missing=$(comm -23 "$expected_file" "$applied_file")
[[ -z "$missing" ]] || {
  printf 'Required migrations are missing for %s:\n%s\n' "$FICANT_DEPLOY_SHA" "$missing" >&2
  exit 1
}
required=$(wc -l <"$expected_file")
applied=$(wc -l <"$applied_file")

echo "Smoke tests passed for $FICANT_DEPLOY_SHA (required_migrations=$required applied_migrations=$applied)."
