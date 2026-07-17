#!/usr/bin/env bash

set -euo pipefail

tool=${1:?license inventory tool required}
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/locks"
python3 - "$root" "$tmp/locks" <<'PY'
import pathlib,sys
source,target=map(pathlib.Path,sys.argv[1:])
for name in ("Cargo.lock", "uv.lock", "pnpm-lock.yaml"):
    payload=(source/(name+".fixture")).read_bytes().replace(b"\r\n",b"\n")
    if b"\r" in payload: raise SystemExit(2)
    (target/name).write_bytes(payload)
PY
cargo_lock="$tmp/locks/Cargo.lock"
uv_lock="$tmp/locks/uv.lock"
pnpm_lock="$tmp/locks/pnpm-lock.yaml"

generate() {
  python3 "$tool" generate --syft "$root/packages.syft.json" --cargo-lock "$cargo_lock" \
    --uv-lock "$uv_lock" --pnpm-lock "$pnpm_lock" \
    --supply-lock "$root/../../../supply-chain.lock.json" --metadata "$root/registry-metadata.json" \
    --output "$tmp/pass.json"
}

verify() {
  python3 "$tool" verify --inventory "$1" --syft "$2" --cargo-lock "$3" \
    --uv-lock "$4" --pnpm-lock "$5" --supply-lock "$root/../../../supply-chain.lock.json"
}

expect_fail() {
  local label=$1
  shift
  if "$@" >"$tmp/$label.out" 2>"$tmp/$label.err"; then
    printf 'license inventory fixture unexpectedly passed: %s\n' "$label" >&2
    exit 1
  fi
}

mutate() {
  local name=$1 expression=$2
  cp "$tmp/pass.json" "$tmp/$name.json"
  python3 - "$tmp/$name.json" "$expression" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]); data = json.loads(path.read_text())
exec(sys.argv[2], {"data": data})
path.write_text(json.dumps(data, sort_keys=True, separators=(",", ":")) + "\n")
PY
}

generate
verify "$tmp/pass.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"

# Generation is bound to native-LF candidate blob content, not checkout EOL.
mkdir -p "$tmp/crlf-locks"
python3 - "$tmp/locks" "$tmp/crlf-locks" <<'PY'
import pathlib,sys
source,target=map(pathlib.Path,sys.argv[1:])
for name in ("Cargo.lock", "uv.lock", "pnpm-lock.yaml"):
    payload=(source/name).read_bytes(); (target/name).write_bytes(payload.replace(b"\n",b"\r\n"))
PY
python3 "$tool" generate --syft "$root/packages.syft.json" --cargo-lock "$tmp/crlf-locks/Cargo.lock" \
  --uv-lock "$tmp/crlf-locks/uv.lock" --pnpm-lock "$tmp/crlf-locks/pnpm-lock.yaml" \
  --supply-lock "$root/../../../supply-chain.lock.json" --metadata "$root/registry-metadata.json" \
  --output "$tmp/crlf.json"
cmp "$tmp/pass.json" "$tmp/crlf.json"
python3 "$tool" verify --inventory "$tmp/pass.json" --syft "$root/packages.syft.json" --cargo-lock "$cargo_lock" \
  --uv-lock "$uv_lock" --pnpm-lock "$pnpm_lock" --supply-lock "$root/../../../supply-chain.lock.json" --require-native-lf
python3 "$tool" verify --inventory "$tmp/pass.json" --syft "$root/packages.syft.json" --cargo-lock "$tmp/crlf-locks/Cargo.lock" \
  --uv-lock "$tmp/crlf-locks/uv.lock" --pnpm-lock "$tmp/crlf-locks/pnpm-lock.yaml" --supply-lock "$root/../../../supply-chain.lock.json"
expect_fail crlf-not-archive python3 "$tool" verify --inventory "$tmp/pass.json" --syft "$root/packages.syft.json" \
  --cargo-lock "$tmp/crlf-locks/Cargo.lock" --uv-lock "$tmp/crlf-locks/uv.lock" --pnpm-lock "$tmp/crlf-locks/pnpm-lock.yaml" \
  --supply-lock "$root/../../../supply-chain.lock.json" --require-native-lf

expression_case() {
  local name=$1 expression=$2
  python3 - "$root/registry-metadata.json" "$tmp/$name-metadata.json" "$expression" <<'PY'
import json,pathlib,sys
data=json.loads(pathlib.Path(sys.argv[1]).read_text()); data["pkg:cargo/rust-ok@1.0.0"]["license"]=sys.argv[3]
pathlib.Path(sys.argv[2]).write_text(json.dumps(data,sort_keys=True,separators=(",",":"))+"\n")
PY
  python3 "$tool" generate --syft "$root/packages.syft.json" --cargo-lock "$cargo_lock" \
    --uv-lock "$uv_lock" --pnpm-lock "$pnpm_lock" --supply-lock "$root/../../../supply-chain.lock.json" \
    --metadata "$tmp/$name-metadata.json" --output "$tmp/$name.json"
}
expression_case or-selectable 'MIT OR GPL-3.0-only'
verify "$tmp/or-selectable.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"
expression_case and-required 'MIT AND GPL-3.0-only'
expect_fail and-required verify "$tmp/and-required.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"
expression_case with-exact 'Apache-2.0 WITH LLVM-exception'
verify "$tmp/with-exact.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"
expression_case with-wrong 'Apache-2.0 WITH Classpath-exception-2.0'
expect_fail with-wrong verify "$tmp/with-wrong.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"
expression_case malformed 'MIT OR'
expect_fail malformed-expression verify "$tmp/malformed.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"

mutate missing 'data["packages"].pop()'
expect_fail missing verify "$tmp/missing.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"
mutate extra 'data["packages"].append(dict(data["packages"][0], purl="pkg:cargo/extra@1.0.0", name="extra"))'
expect_fail extra verify "$tmp/extra.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"
mutate duplicate 'data["packages"].append(dict(data["packages"][0]))'
expect_fail duplicate verify "$tmp/duplicate.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"
mutate key-drift 'data["packages"][0]["version"]="9.9.9"'
expect_fail key-drift verify "$tmp/key-drift.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"
mutate unknown 'data["packages"][0]["license_expression"]="NOASSERTION"'
expect_fail unknown verify "$tmp/unknown.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"
mutate disallowed 'data["packages"][0]["license_expression"]="GPL-3.0-only"'
expect_fail disallowed verify "$tmp/disallowed.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"
mutate integrity 'data["packages"][0]["source_integrity"]="sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"'
expect_fail integrity verify "$tmp/integrity.json" "$root/packages.syft.json" "$cargo_lock" "$uv_lock" "$pnpm_lock"

# A native-LF candidate blob content change must invalidate the frozen header.
cp "$cargo_lock" "$tmp/Cargo.lock"
printf '\n# drift\n' >>"$tmp/Cargo.lock"
expect_fail lock-drift verify "$tmp/pass.json" "$root/packages.syft.json" "$tmp/Cargo.lock" "$uv_lock" "$pnpm_lock"

# A first-party package is an exact package/version/purl/source member of the
# Syft universe and carries the project's exact MIT open-source grant.
mkdir -p "$tmp/release/internal-component"
printf 'internal source\n' >"$tmp/release/internal-component/source.txt"
python3 - "$root/../../../supply-chain.lock.json" "$tmp/first-supply.json" "$root/packages.syft.json" "$tmp/first-syft.json" "$tmp/unresolved.json" <<'PY'
import json,pathlib,sys
source,out,syft_in,syft_out,unresolved=sys.argv[1:]
lock=json.loads(pathlib.Path(source).read_text())
item={"name":"internal-component","version":"0.1.0","purl":"pkg:cargo/internal-component@0.1.0","ecosystem":"crates.io","source":"internal-component"}
lock["first_party_packages"]=[item]
pathlib.Path(out).write_text(json.dumps(lock,sort_keys=True,separators=(",",":"))+"\n")
syft=json.loads(pathlib.Path(syft_in).read_text()); syft["artifacts"].append({"name":item["name"],"version":item["version"],"purl":item["purl"]})
pathlib.Path(syft_out).write_text(json.dumps(syft,sort_keys=True,separators=(",",":"))+"\n")
pathlib.Path(unresolved).write_text(json.dumps([{key:item[key] for key in ("purl","ecosystem","name","version")}])+"\n")
PY
python3 - "$tmp/pass.json" "$tmp/blocked.json" "$tmp/unresolved.json" <<'PY'
import json,pathlib,sys
d=json.loads(pathlib.Path(sys.argv[1]).read_text()); d["status"]="blocked_first_party_license_decision"; d["unresolved_first_party_keys"]=json.loads(pathlib.Path(sys.argv[3]).read_text())
pathlib.Path(sys.argv[2]).write_text(json.dumps(d,sort_keys=True,separators=(",",":"))+"\n")
PY
python3 "$tool" finalize-first-party --inventory "$tmp/blocked.json" --syft "$tmp/first-syft.json" --release-root "$tmp/release" \
  --cargo-lock "$cargo_lock" --uv-lock "$uv_lock" --pnpm-lock "$pnpm_lock" --supply-lock "$tmp/first-supply.json" --output "$tmp/first-final.json"
python3 "$tool" verify --inventory "$tmp/first-final.json" --syft "$tmp/first-syft.json" --release-root "$tmp/release" --require-first-party \
  --cargo-lock "$cargo_lock" --uv-lock "$uv_lock" --pnpm-lock "$pnpm_lock" --supply-lock "$tmp/first-supply.json"
printf 'changed internal source\n' >"$tmp/release/internal-component/source.txt"
python3 "$tool" finalize-first-party --inventory "$tmp/blocked.json" --syft "$tmp/first-syft.json" --release-root "$tmp/release" \
  --cargo-lock "$cargo_lock" --uv-lock "$uv_lock" --pnpm-lock "$pnpm_lock" --supply-lock "$tmp/first-supply.json" --output "$tmp/first-source-changed.json"
python3 - "$tmp/first-final.json" "$tmp/first-source-changed.json" <<'PY'
import json,pathlib,sys
before,after=(json.loads(pathlib.Path(path).read_text()) for path in sys.argv[1:])
assert before["input_tree_digest"] != after["input_tree_digest"]
assert before["inventory_digest"] != after["inventory_digest"]
left={x["purl"]:x for x in before["packages"]}; right={x["purl"]:x for x in after["packages"]}
assert left.keys() == right.keys()
changed=[purl for purl in left if left[purl] != right[purl]]
assert changed == ["pkg:cargo/internal-component@0.1.0"]
assert set(left[changed[0]]) == set(right[changed[0]]) and left[changed[0]]["source_integrity"] != right[changed[0]]["source_integrity"]
PY
mutate_first() { cp "$tmp/first-final.json" "$tmp/$1.json"; python3 - "$tmp/$1.json" "$2" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); d=json.loads(p.read_text()); exec(sys.argv[2],{"data":d}); p.write_text(json.dumps(d,sort_keys=True,separators=(",",":"))+"\n")
PY
}
mutate_first first-source-drift 'next(x for x in data["packages"] if x["classification"]=="first-party-open-source")["source_integrity"]="sha256:bad"'
expect_fail first-source-drift python3 "$tool" verify --inventory "$tmp/first-source-drift.json" --syft "$tmp/first-syft.json" --release-root "$tmp/release" --require-first-party --cargo-lock "$cargo_lock" --uv-lock "$uv_lock" --pnpm-lock "$pnpm_lock" --supply-lock "$tmp/first-supply.json"
expect_fail first-missing python3 "$tool" finalize-first-party --inventory "$tmp/blocked.json" --syft "$root/packages.syft.json" --release-root "$tmp/release" --cargo-lock "$cargo_lock" --uv-lock "$uv_lock" --pnpm-lock "$pnpm_lock" --supply-lock "$tmp/first-supply.json" --output "$tmp/missing-final.json"

# Non-global licenses are allowed only by an exact purl/name/version/source match.
python3 - "$root/../../../supply-chain.lock.json" "$tmp/scoped-supply.json" "$root/registry-metadata.json" "$tmp/scoped-metadata.json" <<'PY'
import json,pathlib,sys
lock=json.loads(pathlib.Path(sys.argv[1]).read_text()); metadata=json.loads(pathlib.Path(sys.argv[3]).read_text())
entry=metadata["pkg:cargo/rust-ok@1.0.0"]; entry["license"]="GPL-3.0-only"
lock["license_scoped_exceptions"]=[{"purl":"pkg:cargo/rust-ok@1.0.0","name":"rust-ok","version":"1.0.0","license_expression":"GPL-3.0-only","source_locator":entry["source_locator"],"source_integrity":entry["integrity"],"license_text_sha256":"0"*64,"attribution":"fixture"}]
pathlib.Path(sys.argv[2]).write_text(json.dumps(lock,sort_keys=True,separators=(",",":"))+"\n"); pathlib.Path(sys.argv[4]).write_text(json.dumps(metadata,sort_keys=True,separators=(",",":"))+"\n")
PY
python3 "$tool" generate --syft "$root/packages.syft.json" --cargo-lock "$cargo_lock" --uv-lock "$uv_lock" --pnpm-lock "$pnpm_lock" --supply-lock "$tmp/scoped-supply.json" --metadata "$tmp/scoped-metadata.json" --output "$tmp/scoped.json"
python3 "$tool" verify --inventory "$tmp/scoped.json" --syft "$root/packages.syft.json" --cargo-lock "$cargo_lock" --uv-lock "$uv_lock" --pnpm-lock "$pnpm_lock" --supply-lock "$tmp/scoped-supply.json"
python3 - "$tmp/scoped-supply.json" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); d=json.loads(p.read_text()); d["license_scoped_exceptions"][0]["version"]="1.0.1"; p.write_text(json.dumps(d))
PY
expect_fail scoped-version-drift python3 "$tool" verify --inventory "$tmp/scoped.json" --syft "$root/packages.syft.json" --cargo-lock "$cargo_lock" --uv-lock "$uv_lock" --pnpm-lock "$pnpm_lock" --supply-lock "$tmp/scoped-supply.json"

digest=$(python3 "$tool" digest --inventory "$tmp/pass.json")
cat >"$tmp/provenance.json" <<JSON
{"topology":{"candidate":"1111111111111111111111111111111111111111","candidate_tree":"2222222222222222222222222222222222222222"},"license_inventory":{"digest":"$digest"}}
JSON
python3 "$tool" verify-provenance --inventory "$tmp/pass.json" --provenance "$tmp/provenance.json" \
  --candidate 1111111111111111111111111111111111111111 --tree 2222222222222222222222222222222222222222
expect_fail provenance-candidate python3 "$tool" verify-provenance --inventory "$tmp/pass.json" --provenance "$tmp/provenance.json" \
  --candidate 3333333333333333333333333333333333333333 --tree 2222222222222222222222222222222222222222
expect_fail provenance-tree python3 "$tool" verify-provenance --inventory "$tmp/pass.json" --provenance "$tmp/provenance.json" \
  --candidate 1111111111111111111111111111111111111111 --tree 3333333333333333333333333333333333333333
mutate provenance-inventory 'data["packages"][0]["license_expression"]="Apache-2.0"'
expect_fail provenance-inventory python3 "$tool" verify-provenance --inventory "$tmp/provenance-inventory.json" \
  --provenance "$tmp/provenance.json" --candidate 1111111111111111111111111111111111111111 \
  --tree 2222222222222222222222222222222222222222

printf 'license inventory fixtures: PASS\n'
