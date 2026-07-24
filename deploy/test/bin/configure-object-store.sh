#!/usr/bin/env bash
set -euo pipefail

root=${FICANT_ROOT:-/srv/ficant-test}
env_file="$root/.env"
[[ -f "$env_file" ]] || { echo "Missing test environment file: $env_file" >&2; exit 1; }

IFS= read -r access_key
IFS= read -r secret_key
IFS= read -r bucket

[[ "$access_key" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{7,63}$ ]] \
  || { echo 'Invalid S3 access key.' >&2; exit 2; }
[[ "$secret_key" =~ ^[A-Za-z0-9._~-]{32,128}$ ]] \
  || { echo 'Invalid S3 secret key.' >&2; exit 2; }
[[ "$bucket" =~ ^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$ && "$bucket" != *..* ]] \
  || { echo 'Invalid S3 bucket.' >&2; exit 2; }

exec 8>"$root/state/config.lock"
flock -n 8 || { echo 'Another test-environment configuration update is in progress.' >&2; exit 1; }

temporary=$(mktemp "$root/.env.XXXXXX")
trap 'rm -f "$temporary"' EXIT
awk '!/^FICANT_S3_(ACCESS_KEY|SECRET_KEY|BUCKET)=/' "$env_file" >"$temporary"
printf 'FICANT_S3_ACCESS_KEY=%s\nFICANT_S3_SECRET_KEY=%s\nFICANT_S3_BUCKET=%s\n' \
  "$access_key" "$secret_key" "$bucket" >>"$temporary"
chmod 0600 "$temporary"
mv -f "$temporary" "$env_file"
trap - EXIT
echo 'Test object-store credentials configured.'
