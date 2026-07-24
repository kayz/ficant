#!/usr/bin/env bash
set -euo pipefail

root=${FICANT_ROOT:-/srv/ficant-test}
env_file="$root/.env"
[[ -f "$env_file" ]] || { echo "Missing test environment file: $env_file" >&2; exit 1; }

IFS= read -r access_key
IFS= read -r secret_key
IFS= read -r bucket
IFS= read -r cursor_key
IFS= read -r bearer_token

[[ "$access_key" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{7,63}$ ]] \
  || { echo 'Invalid S3 access key.' >&2; exit 2; }
[[ "$secret_key" =~ ^[A-Za-z0-9._~-]{32,128}$ ]] \
  || { echo 'Invalid S3 secret key.' >&2; exit 2; }
[[ "$bucket" =~ ^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$ && "$bucket" != *..* ]] \
  || { echo 'Invalid S3 bucket.' >&2; exit 2; }
[[ "$cursor_key" =~ ^[0-9a-f]{64}$ ]] \
  || { echo 'Invalid experiment cursor key.' >&2; exit 2; }
[[ "$bearer_token" =~ ^[A-Za-z0-9._~-]{32,128}$ ]] \
  || { echo 'Invalid test bootstrap bearer token.' >&2; exit 2; }

exec 8>"$root/state/config.lock"
flock -n 8 || { echo 'Another test-environment configuration update is in progress.' >&2; exit 1; }

temporary=$(mktemp "$root/.env.XXXXXX")
trap 'rm -f "$temporary"' EXIT
awk '!/^FICANT_S3_(ACCESS_KEY|SECRET_KEY|BUCKET)=/ &&
     !/^FICANT_EXPERIMENT_CURSOR_KEY_HEX=/ &&
     !/^FICANT_BOOTSTRAP_BEARER_TOKEN=/' "$env_file" >"$temporary"
printf 'FICANT_S3_ACCESS_KEY=%s\nFICANT_S3_SECRET_KEY=%s\nFICANT_S3_BUCKET=%s\nFICANT_EXPERIMENT_CURSOR_KEY_HEX=%s\nFICANT_BOOTSTRAP_BEARER_TOKEN=%s\n' \
  "$access_key" "$secret_key" "$bucket" "$cursor_key" "$bearer_token" >>"$temporary"
chmod 0600 "$temporary"
mv -f "$temporary" "$env_file"
trap - EXIT
echo 'Test object-store and experiment credentials configured.'
