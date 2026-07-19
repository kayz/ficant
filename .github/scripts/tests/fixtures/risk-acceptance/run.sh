#!/usr/bin/env bash
set -euo pipefail

tool=${1:?risk acceptance tool required}
lock=${2:?supply lock required}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cp "$lock" "$tmp/lock.json"
cat >"$tmp/reachability.json" <<'JSON'
{"schema_version":1,"cargo_version":"cargo 1.96.1 (fixture)","configuration":{"locked":true,"all_features":true,"target":"all","command":"cargo tree","format":"{p}"},"cargo_lock_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","manifests_digest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","resolved_graph_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","reachable":[{"name":"ficant-storage","version":"0.1.0"}],"unreachable_lock_only":[]}
JSON
cat >"$tmp/vulnerabilities.json" <<'JSON'
{"schema_version":1,"candidate":{"commit":"1111111111111111111111111111111111111111","tree":"2222222222222222222222222222222222222222"},"scans":[],"results":[]}
JSON
: >"$tmp/retired-chain.txt"

python3 "$tool" generate --supply-lock "$tmp/lock.json" \
  --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" \
  --chain "$tmp/retired-chain.txt" --output "$tmp/disposition.json"
python3 "$tool" verify --supply-lock "$tmp/lock.json" \
  --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" \
  --chain "$tmp/retired-chain.txt" --output "$tmp/disposition.json"

python3 - "$tmp/disposition.json" <<'PY'
import json, pathlib, sys
document = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
assert document["status"] == "none"
assert document["acceptances"] == []
assert document["inputs"]["chain_sha256"] is None
PY

python3 - "$tmp/lock.json" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
document = json.loads(path.read_text(encoding="utf-8"))
document["risk_acceptances"] = [{"id": "forbidden-active-acceptance"}]
path.write_text(json.dumps(document), encoding="utf-8")
PY
if python3 "$tool" generate --supply-lock "$tmp/lock.json" \
  --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" \
  --chain "$tmp/retired-chain.txt" --output "$tmp/invalid.json" >/dev/null 2>&1; then
  echo "active risk acceptance unexpectedly passed" >&2
  exit 1
fi

printf 'empty risk acceptance fixtures: PASS\n'
