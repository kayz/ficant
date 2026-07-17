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
expected=$(find "$root/releases/$FICANT_DEPLOY_SHA/migrations" -maxdepth 1 -type f -name '*.sql' | wc -l)
applied=$("${compose[@]}" exec -T postgres psql -U ficant -d ficant -At -c 'SELECT count(*) FROM public.ficant_schema_migrations')
[[ "$applied" -eq "$expected" ]] || { echo "Migration count mismatch: expected=$expected applied=$applied" >&2; exit 1; }

echo "Smoke tests passed for $FICANT_DEPLOY_SHA (migrations=$applied)."
