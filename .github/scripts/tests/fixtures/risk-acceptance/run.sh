#!/usr/bin/env bash
set -euo pipefail
tool=${1:?risk acceptance tool required}
lock=${2:?supply lock required}
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
cp "$lock" "$tmp/lock.json"
cat >"$tmp/reachability.json" <<'JSON'
{"schema_version":1,"cargo_version":"cargo 1.96.1 (fixture)","configuration":{"locked":true,"all_features":true,"target":"all","command":"cargo tree","format":"{p}"},"cargo_lock_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","manifests_digest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","resolved_graph_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","reachable":[{"name":"async-std","version":"1.13.2"}],"unreachable_lock_only":[]}
JSON
cat >"$tmp/chain.txt" <<'EOF'
async-std v1.13.2
minio v0.4.0
ficant-storage v0.1.0
EOF
write_vulnerability() {
  local version=$1
  cat >"$tmp/vulnerabilities.json" <<JSON
{"candidate":{"commit":"1111111111111111111111111111111111111111","tree":"2222222222222222222222222222222222222222"},"results":[{"packages":[{"package":{"name":"async-std","version":"$version","ecosystem":"crates.io"},"vulnerabilities":[{"id":"RUSTSEC-2025-0052","affected":[{"package":{"ecosystem":"crates.io","name":"async-std"},"ranges":[{"type":"SEMVER","events":[{"introduced":"0.0.0-0"}]}],"database_specific":{"categories":[],"cvss":null,"informational":"unmaintained","source":"https://github.com/rustsec/advisory-db/blob/osv/crates/RUSTSEC-2025-0052.json"}}],"database_specific":{"license":"CC0-1.0"}}]}]}]}
JSON
}
expect_fail() { local label=$1; shift; if "$@" >"$tmp/$label.out" 2>"$tmp/$label.err"; then echo "risk fixture unexpectedly passed: $label" >&2; exit 1; fi; }
write_vulnerability 1.13.2
python3 "$tool" generate --supply-lock "$tmp/lock.json" --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" --chain "$tmp/chain.txt" --output "$tmp/accepted.json"
python3 "$tool" verify --supply-lock "$tmp/lock.json" --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" --chain "$tmp/chain.txt" --output "$tmp/accepted.json"
python3 - "$tmp/accepted.json" <<'PY'
import json, pathlib, sys
d=json.loads(pathlib.Path(sys.argv[1]).read_text())
assert d["status"] == "accepted-unfixed" and d["acceptances"][0]["vulnerability_ids"] == ["RUSTSEC-2025-0052"]
PY
python3 - "$tmp/vulnerabilities.json" <<'PY'
import json, pathlib, sys
p=pathlib.Path(sys.argv[1]); d=json.loads(p.read_text()); d["results"][0]["packages"][0]["vulnerabilities"][0]["id"]="RUSTSEC-2099-9999"; p.write_text(json.dumps(d))
PY
expect_fail advisory-id-drift python3 "$tool" generate --supply-lock "$tmp/lock.json" --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" --chain "$tmp/chain.txt" --output "$tmp/invalid.json"
write_vulnerability 1.13.2
python3 - "$tmp/vulnerabilities.json" <<'PY'
import copy, json, pathlib, sys
p=pathlib.Path(sys.argv[1]); d=json.loads(p.read_text()); extra=copy.deepcopy(d["results"][0]["packages"][0]["vulnerabilities"][0]); extra["id"]="RUSTSEC-2099-9999"; d["results"][0]["packages"][0]["vulnerabilities"].append(extra); p.write_text(json.dumps(d))
PY
expect_fail additional-advisory python3 "$tool" generate --supply-lock "$tmp/lock.json" --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" --chain "$tmp/chain.txt" --output "$tmp/invalid.json"
write_vulnerability 1.13.2
python3 - "$tmp/vulnerabilities.json" <<'PY'
import json, pathlib, sys
p=pathlib.Path(sys.argv[1]); d=json.loads(p.read_text()); v=d["results"][0]["packages"][0]["vulnerabilities"][0]; v["affected"][0]["database_specific"]={"categories":["security"],"cvss":"CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H","informational":"unmaintained","source":"https://github.com/rustsec/advisory-db/blob/osv/crates/RUSTSEC-2025-0052.json"}; p.write_text(json.dumps(d))
PY
expect_fail security-drift python3 "$tool" generate --supply-lock "$tmp/lock.json" --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" --chain "$tmp/chain.txt" --output "$tmp/invalid.json"
write_vulnerability 1.13.2
python3 - "$tmp/vulnerabilities.json" <<'PY'
import json, pathlib, sys
p=pathlib.Path(sys.argv[1]); d=json.loads(p.read_text()); d["results"][0]["packages"][0]["vulnerabilities"][0]["affected"][0]["database_specific"]["informational"]="unsound"; p.write_text(json.dumps(d))
PY
expect_fail unsound-drift python3 "$tool" generate --supply-lock "$tmp/lock.json" --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" --chain "$tmp/chain.txt" --output "$tmp/invalid.json"
cp "$tmp/lock.json" "$tmp/expired-lock.json"
python3 - "$tmp/expired-lock.json" <<'PY'
import json, pathlib, sys
p=pathlib.Path(sys.argv[1]); d=json.loads(p.read_text()); d["risk_acceptances"][0]["expires_on"]="2000-01-01"; p.write_text(json.dumps(d))
PY
write_vulnerability 1.13.2
expect_fail expired-policy python3 "$tool" generate --supply-lock "$tmp/expired-lock.json" --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" --chain "$tmp/chain.txt" --output "$tmp/invalid.json"
printf 'async-std v1.13.2\n' >"$tmp/chain.txt"
expect_fail chain-drift python3 "$tool" generate --supply-lock "$tmp/lock.json" --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" --chain "$tmp/chain.txt" --output "$tmp/accepted.json"
write_vulnerability 1.13.3
python3 "$tool" generate --supply-lock "$tmp/lock.json" --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" --chain "$tmp/missing.txt" --output "$tmp/not-inherited.json"
python3 - "$tmp/not-inherited.json" <<'PY'
import json, pathlib, sys
d=json.loads(pathlib.Path(sys.argv[1]).read_text()); assert d["status"] == "none" and d["acceptances"] == []
PY
python3 - "$tmp/lock.json" <<'PY'
import json, pathlib, sys
p=pathlib.Path(sys.argv[1]); d=json.loads(p.read_text()); d["risk_acceptances"][0]["status"]="ignored"; p.write_text(json.dumps(d))
PY
expect_fail status-drift python3 "$tool" generate --supply-lock "$tmp/lock.json" --vulnerabilities "$tmp/vulnerabilities.json" --reachability "$tmp/reachability.json" --chain "$tmp/missing.txt" --output "$tmp/invalid.json"

# The supply evidence gate consumes the verified acceptance once, preserves
# the raw OSV finding, and binds the acceptance to release provenance.
scripts_dir=$(CDPATH= cd -- "$(dirname -- "$tool")" && pwd)
cp -R "$scripts_dir/tests/fixtures/pass" "$tmp/evidence"
printf 'async-std v1.13.2\nminio v0.4.0\nficant-storage v0.1.0\n' >"$tmp/chain.txt"
cp "$tmp/chain.txt" "$tmp/evidence/cargo-async-std-chain.txt"
cp "$tmp/reachability.json" "$tmp/evidence/cargo-reachability.json"
write_vulnerability 1.13.2
python3 - "$tmp/vulnerabilities.json" "$tmp/evidence/vulnerabilities.json" <<'PY'
import json,pathlib,sys
d=json.loads(pathlib.Path(sys.argv[1]).read_text()); d["schema_version"]=1
d["candidate"]["tree"]="dddddddddddddddddddddddddddddddddddddddd"
d["scans"]=[{"lock_path":"Cargo.lock","lock_sha256":"a"*64,"result_count":1},{"lock_path":"python/uv.lock","lock_sha256":"b"*64,"result_count":0},{"lock_path":"web-dm/pnpm-lock.yaml","lock_sha256":"c"*64,"result_count":0}]
d["results"][0]["source"]={"path":"Cargo.lock","type":"lockfile"}
d["results"][0]["packages"][0]["vulnerabilities"][0]["databaseSpecific"]={"severity":"HIGH"}
pathlib.Path(sys.argv[2]).write_text(json.dumps(d,sort_keys=True,separators=(",",":"))+"\n")
PY
python3 "$tool" generate --supply-lock "$lock" --vulnerabilities "$tmp/evidence/vulnerabilities.json" --reachability "$tmp/evidence/cargo-reachability.json" --chain "$tmp/evidence/cargo-async-std-chain.txt" --output "$tmp/evidence/accepted-unfixed.json"
python3 - "$tmp/evidence" "$scripts_dir/license-inventory.lock.json" "$lock" <<'PY'
import hashlib,json,pathlib,sys
r=pathlib.Path(sys.argv[1]); inventory_path=pathlib.Path(sys.argv[2]); inventory=json.loads(inventory_path.read_text()); lock=json.loads(pathlib.Path(sys.argv[3]).read_text()); p=r/"release-provenance.json"; d=json.loads(p.read_text()); reach=json.loads((r/"cargo-reachability.json").read_text()); accepted=json.loads((r/"accepted-unfixed.json").read_text())
(r/"packages.syft.json").write_text(json.dumps({"artifacts":[{"name":x["name"],"version":x["version"],"purl":x["purl"],"licenses":[{"value":"NOASSERTION"}]} for x in inventory["packages"]]},sort_keys=True,separators=(",",":"))+"\n")
base=lock["release_topology"]["trusted_base"]; d["topology"]["trusted_base"]=base; d["topology"]["parent"]=base
for scan in d["secret_scans"]:
    if scan["kind"]=="published_base_history": scan["scope"]=base
    elif scan["kind"]=="candidate_range": scan["scope"]=f'{base}..{d["topology"]["candidate"]}'
d["license_inventory"]={"digest":inventory["inventory_digest"],"file_sha256":hashlib.sha256(inventory_path.read_bytes()).hexdigest(),"generator":inventory["generator"]}
d["cargo_reachability"]={"evidence_sha256":hashlib.sha256((r/"cargo-reachability.json").read_bytes()).hexdigest(),"cargo_lock_sha256":reach.get("cargo_lock_sha256"),"manifests_digest":reach.get("manifests_digest"),"resolved_graph_sha256":reach.get("resolved_graph_sha256")}
d["accepted_unfixed"]={"evidence_sha256":hashlib.sha256((r/"accepted-unfixed.json").read_bytes()).hexdigest(),"status":accepted["status"],"acceptance_ids":[x["id"] for x in accepted["acceptances"]]}
p.write_text(json.dumps(d,sort_keys=True,separators=(",",":"))+"\n")
PY
"$scripts_dir/verify-supply-chain.sh" --verify-evidence "$tmp/evidence"
printf 'risk acceptance fixtures: PASS\n'
