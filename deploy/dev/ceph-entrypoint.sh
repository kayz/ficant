#!/usr/bin/env bash
set -Eeuo pipefail

umask 077

readonly cluster="ceph"
readonly mon_name="ficant"
readonly rgw_name="ficant"
readonly state_dir="/var/lib/ceph"
readonly conf="/etc/ceph/${cluster}.conf"
readonly admin_keyring="/etc/ceph/${cluster}.client.admin.keyring"
readonly mon_keyring="/etc/ceph/${cluster}.mon.keyring"
readonly mon_dir="${state_dir}/mon/${cluster}-${mon_name}"
readonly osd_dir="${state_dir}/osd/${cluster}-0"
readonly rgw_dir="${state_dir}/radosgw/${cluster}-rgw.${rgw_name}"
readonly rgw_keyring="${rgw_dir}/keyring"
readonly fsid_file="${state_dir}/fsid"
readonly monmap="/tmp/${cluster}.monmap"
readonly osd_new="/tmp/${cluster}.osd-new.json"
readonly rgw_port="${FICANT_CEPH_RGW_PORT:-9000}"
readonly s3_region="us-east-1"
readonly empty_sha256="e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

require_env() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    printf 'required environment variable is empty: %s\n' "$name" >&2
    exit 2
  fi
}

require_env FICANT_S3_ACCESS_KEY
require_env FICANT_S3_SECRET_KEY
require_env FICANT_S3_BUCKET
if [[ ! "$FICANT_S3_BUCKET" =~ ^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$ || "$FICANT_S3_BUCKET" == *..* ]]; then
  printf 'fixture bucket name is not DNS-compatible\n' >&2
  exit 2
fi

mon_ip="$(hostname -i | tr ' ' '\n' | grep -m1 -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$' || true)"
if [[ -z "$mon_ip" ]]; then
  printf 'unable to resolve container IPv4 address\n' >&2
  exit 2
fi

mkdir -p /etc/ceph "$mon_dir" "$osd_dir" "$rgw_dir"

if [[ ! -s "$fsid_file" ]]; then
  uuidgen >"$fsid_file"
fi
fsid="$(<"$fsid_file")"

cat >"$conf" <<EOF
[global]
fsid = ${fsid}
mon initial members = ${mon_name}
mon host = [v2:${mon_ip}:3300/0,v1:${mon_ip}:6789/0]
auth cluster required = cephx
auth service required = cephx
auth client required = cephx
osd pool default size = 1
osd pool default min size = 1
osd crush chooseleaf type = 0
mon allow pool size one = true
mon data avail warn = 2
mon data avail crit = 1
osd memory target = 268435456
osd objectstore = bluestore
osd bluestore block create = true
osd bluestore block size = 2147483648
ms async op threads = 1
osd op num shards = 1
osd op num threads per shard = 1
log to file = false
log to stderr = true
err to stderr = true

[mon.${mon_name}]
mon data = ${mon_dir}

[osd.0]
osd data = ${osd_dir}

[client.rgw.${rgw_name}]
rgw frontends = beast endpoint=0.0.0.0:${rgw_port}
rgw thread pool size = 64
rgw enable usage log = false
EOF

if [[ ! -s "$admin_keyring" ]]; then
  ceph-authtool "$admin_keyring" --create-keyring --gen-key -n client.admin \
    --cap mon 'allow *' --cap osd 'allow *' --cap mgr 'allow *' --cap mds 'allow *'
fi

if [[ ! -s "$mon_keyring" ]]; then
  ceph-authtool "$mon_keyring" --create-keyring --gen-key -n mon. --cap mon 'allow *'
  ceph-authtool "$mon_keyring" --import-keyring "$admin_keyring"
fi

if [[ ! -s "${mon_dir}/store.db/CURRENT" ]]; then
  monmaptool --create --addv "$mon_name" "[v2:${mon_ip}:3300/0,v1:${mon_ip}:6789/0]" \
    --fsid "$fsid" "$monmap"
  ceph-mon --cluster "$cluster" --mkfs -i "$mon_name" --inject-monmap "$monmap" \
    --keyring "$mon_keyring" --mon-data "$mon_dir"
else
  ceph-mon --cluster "$cluster" -i "$mon_name" --mon-data "$mon_dir" --extract-monmap "$monmap"
  monmaptool --rm "$mon_name" "$monmap"
  monmaptool --addv "$mon_name" "[v2:${mon_ip}:3300/0,v1:${mon_ip}:6789/0]" "$monmap"
  ceph-mon --cluster "$cluster" -i "$mon_name" --mon-data "$mon_dir" --inject-monmap "$monmap"
fi

pids=()
terminate() {
  local pid
  for pid in "${pids[@]:-}"; do
    kill -TERM "$pid" 2>/dev/null || true
  done
  wait || true
}
trap terminate TERM INT EXIT

ceph-mon --cluster "$cluster" -i "$mon_name" --mon-data "$mon_dir" --foreground \
  --public-addr "$mon_ip" --setuser ceph --setgroup ceph &
pids+=("$!")

for _ in $(seq 1 60); do
  if ceph --cluster "$cluster" --name client.admin --keyring "$admin_keyring" status >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
ceph --cluster "$cluster" --name client.admin --keyring "$admin_keyring" status >/dev/null

if [[ ! -s "${osd_dir}/keyring" ]]; then
  osd_uuid="$(uuidgen)"
  osd_secret="$(ceph-authtool --gen-print-key)"
  printf '{"cephx_secret":"%s"}\n' "$osd_secret" >"$osd_new"
  osd_id="$(ceph --cluster "$cluster" --name client.admin --keyring "$admin_keyring" \
    osd new "$osd_uuid" -i "$osd_new")"
  if [[ "$osd_id" != "0" ]]; then
    printf 'single-node fixture expected OSD 0, got %s\n' "$osd_id" >&2
    exit 3
  fi
  printf '[osd.0]\n\tkey = %s\n' "$osd_secret" >"${osd_dir}/keyring"
  printf '%s\n' "$osd_uuid" >"${osd_dir}/fsid"
  ceph-osd --cluster "$cluster" -i 0 --osd-data "$osd_dir" --osd-uuid "$osd_uuid" \
    --keyring "${osd_dir}/keyring" --mkfs
fi

osd_uuid="$(<"${osd_dir}/fsid")"
ceph-osd --cluster "$cluster" -i 0 --osd-data "$osd_dir" --osd-uuid "$osd_uuid" \
  --keyring "${osd_dir}/keyring" --foreground --setuser ceph --setgroup ceph &
pids+=("$!")

for _ in $(seq 1 90); do
  if ceph --cluster "$cluster" --name client.admin --keyring "$admin_keyring" osd stat 2>/dev/null | grep -Eq '1 up'; then
    break
  fi
  sleep 1
done
ceph --cluster "$cluster" --name client.admin --keyring "$admin_keyring" osd stat | grep -Eq '1 up'

if [[ ! -s "$rgw_keyring" ]]; then
  ceph --cluster "$cluster" --name client.admin --keyring "$admin_keyring" auth get-or-create \
    "client.rgw.${rgw_name}" mon 'allow rw' osd 'allow rwx' mgr 'allow rw' -o "$rgw_keyring"
fi

if ! radosgw-admin --cluster "$cluster" --name "client.rgw.${rgw_name}" --keyring "$rgw_keyring" \
  user info --uid ficant >/dev/null 2>&1; then
  radosgw-admin --cluster "$cluster" --name "client.rgw.${rgw_name}" --keyring "$rgw_keyring" \
    user create --uid ficant --display-name FICANT \
    --access-key "$FICANT_S3_ACCESS_KEY" --secret-key "$FICANT_S3_SECRET_KEY" >/dev/null
fi

zonegroup_json="/tmp/${cluster}.zonegroup.json"
radosgw-admin --cluster "$cluster" --name client.admin --keyring "$admin_keyring" \
  zonegroup modify --rgw-zonegroup default --api-name "$s3_region" >"$zonegroup_json"
if [[ "$(jq -r '.api_name // empty' "$zonegroup_json")" != "$s3_region" ]]; then
  printf 'default zonegroup S3 region does not match %s\n' "$s3_region" >&2
  exit 4
fi

radosgw --cluster "$cluster" --name "client.rgw.${rgw_name}" --keyring "$rgw_keyring" \
  --foreground --setuser ceph --setgroup ceph &
pids+=("$!")

for _ in $(seq 1 90); do
  if [[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:${rgw_port}/" || true)" == "200" ]]; then
    break
  fi
  sleep 1
done
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:${rgw_port}/" || true)" == "200" ]]

bucket_json="/tmp/${cluster}.bucket.json"
bucket_response="/tmp/${cluster}.bucket-response.xml"
if ! radosgw-admin --cluster "$cluster" --name client.admin --keyring "$admin_keyring" \
  bucket stats --bucket "$FICANT_S3_BUCKET" >"$bucket_json" 2>/dev/null; then
  bucket_code="$(curl --silent --show-error --output "$bucket_response" --write-out '%{http_code}' \
    --aws-sigv4 "aws:amz:${s3_region}:s3" \
    --user "${FICANT_S3_ACCESS_KEY}:${FICANT_S3_SECRET_KEY}" \
    --header "x-amz-content-sha256:${empty_sha256}" \
    --request PUT "http://127.0.0.1:${rgw_port}/${FICANT_S3_BUCKET}")"
  if [[ "$bucket_code" != "200" && "$bucket_code" != "204" ]]; then
    printf 'fixture bucket creation returned HTTP %s\n' "$bucket_code" >&2
    sed -n '1,20p' "$bucket_response" >&2
    exit 4
  fi
  radosgw-admin --cluster "$cluster" --name client.admin --keyring "$admin_keyring" \
    bucket stats --bucket "$FICANT_S3_BUCKET" >"$bucket_json"
fi
if [[ "$(jq -r '.owner // empty' "$bucket_json")" != "ficant" ]]; then
  printf 'fixture bucket is not owned by the ficant user\n' >&2
  exit 4
fi

touch /run/ceph/ficant-ready
printf 'Ceph RGW fixture is ready on port %s\n' "$rgw_port"
wait -n "${pids[@]}"
