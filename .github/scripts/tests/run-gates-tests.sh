#!/usr/bin/env bash

set -euo pipefail

scripts_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
fixtures_dir="$scripts_dir/tests/fixtures"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

expect_exit() {
  local expected=$1
  shift
  set +e
  "$@" >"$tmp/stdout" 2>"$tmp/stderr"
  local actual=$?
  set -e
  if [[ $actual -ne $expected ]]; then
    printf 'expected exit %s, got %s: %s\n' "$expected" "$actual" "$*" >&2
    cat "$tmp/stdout" >&2 || true
    cat "$tmp/stderr" >&2 || true
    exit 1
  fi
}

# Source identities are resolved once in the real worktree and exported for
# deterministic archive builds, which intentionally contain no .git metadata.
mkdir -p "$tmp/source-identity"
git -C "$tmp/source-identity" init -q
git -C "$tmp/source-identity" config user.name fixture
git -C "$tmp/source-identity" config user.email fixture@example.invalid
printf 'identity fixture\n' >"$tmp/source-identity/input.txt"
git -C "$tmp/source-identity" add input.txt
git -C "$tmp/source-identity" commit -q -m fixture
fixture_commit=$(git -C "$tmp/source-identity" rev-parse HEAD)
fixture_tree=$(git -C "$tmp/source-identity" rev-parse 'HEAD^{tree}')
expect_exit 0 "$scripts_dir/verify-reproducibility.sh" --bind-source-identity \
  "$tmp/source-identity"
grep -Fx "FICANT_CODE_COMMIT_SHA=$fixture_commit" "$tmp/stdout" >/dev/null
grep -Fx "FICANT_CODE_TREE_SHA=$fixture_tree" "$tmp/stdout" >/dev/null
FICANT_CODE_COMMIT_SHA="$fixture_commit" FICANT_CODE_TREE_SHA="$fixture_tree" \
  expect_exit 0 "$scripts_dir/verify-reproducibility.sh" --bind-source-identity \
  "$tmp/source-identity"
FICANT_CODE_COMMIT_SHA=0000000000000000000000000000000000000000 \
  expect_exit 2 "$scripts_dir/verify-reproducibility.sh" --bind-source-identity \
  "$tmp/source-identity"
FICANT_CODE_TREE_SHA=0000000000000000000000000000000000000000 \
  expect_exit 2 "$scripts_dir/verify-reproducibility.sh" --bind-source-identity \
  "$tmp/source-identity"
mkdir -p "$tmp/source-archive"
git -C "$tmp/source-identity" archive HEAD | tar -x -C "$tmp/source-archive"
[[ ! -e $tmp/source-archive/.git ]]
FICANT_CODE_COMMIT_SHA="$fixture_commit" FICANT_CODE_TREE_SHA="$fixture_tree" \
  bash -c '[[ $FICANT_CODE_COMMIT_SHA =~ ^[0-9a-f]{40}$ && $FICANT_CODE_TREE_SHA =~ ^[0-9a-f]{40}$ ]]'

# The frozen descriptor authority is a plain FileDescriptorSet. Exercise the
# script's descriptor target with a deterministic Buf fixture so an image build
# cannot silently replace that evidence format.
mkdir -p "$tmp/fake-bin" "$tmp/interface"
cat >"$tmp/fake-bin/buf" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$@" >"${BUF_ARGS_LOG:?}"
output=
while [[ $# -gt 0 ]]; do
  if [[ $1 == -o || $1 == --output ]]; then
    output=$2
    shift 2
    continue
  fi
  shift
done
[[ -n $output ]] || exit 2
printf 'descriptor-fixture\n' >"$output"
SH
chmod 0755 "$tmp/fake-bin/buf"
BUF_ARGS_LOG="$tmp/buf-args" PATH="$tmp/fake-bin:$PATH" \
  expect_exit 0 "$scripts_dir/verify-contract-generation.sh" --build-descriptor \
  "$tmp/interface" "$tmp/descriptor.bin"
printf '%s\n' build "$tmp/interface" --as-file-descriptor-set -o "$tmp/descriptor.bin" \
  >"$tmp/expected-buf-args"
cmp "$tmp/expected-buf-args" "$tmp/buf-args"
[[ $(cat "$tmp/descriptor.bin") == descriptor-fixture ]]

cat >"$tmp/native-exit" <<'SH'
#!/usr/bin/env bash
case ${1:-} in
  signal) kill -TERM $$ ;;
  *) exit "${1:-0}" ;;
esac
SH
chmod 0755 "$tmp/native-exit"

# Complete execution paths normalize native exits without leaking Cargo/signal codes.
for gate in verify-contract-generation.sh verify-reproducibility.sh; do
  expect_exit 1 "$scripts_dir/$gate" --map-native finding "$tmp/native-exit" 1
  expect_exit 2 "$scripts_dir/$gate" --map-native finding "$tmp/native-exit" 42
  expect_exit 2 "$scripts_dir/$gate" --map-native finding "$tmp/native-exit" 101
  expect_exit 2 "$scripts_dir/$gate" --map-native finding "$tmp/command-does-not-exist"
  expect_exit 2 "$scripts_dir/$gate" --map-native finding "$tmp/native-exit" signal
  expect_exit 2 "$scripts_dir/$gate" --map-native tool "$tmp/native-exit" 101
  expect_exit 2 "$scripts_dir/$gate" --map-native tool "$tmp/command-does-not-exist"
  expect_exit 2 "$scripts_dir/$gate" --map-native tool "$tmp/native-exit" 42
  expect_exit 2 "$scripts_dir/$gate" --map-native tool "$tmp/native-exit" signal
done

# Contract generation evidence must reject drift and accept identical trees.
mkdir -p "$tmp/generated-a" "$tmp/generated-b" "$tmp/generated-tracked"
printf 'same\n' >"$tmp/generated-a/message.rs"
printf 'drift\n' >"$tmp/generated-b/message.rs"
printf 'same\n' >"$tmp/generated-tracked/message.rs"
expect_exit 1 "$scripts_dir/verify-contract-generation.sh" --verify-trees \
  "$tmp/generated-a" "$tmp/generated-b" "$tmp/generated-tracked"
cp "$tmp/generated-a/message.rs" "$tmp/generated-b/message.rs"
expect_exit 0 "$scripts_dir/verify-contract-generation.sh" --verify-trees \
  "$tmp/generated-a" "$tmp/generated-b" "$tmp/generated-tracked"

# The breaking baseline must be obtainable from normal repository history, not
# merely from a stale object retained by one developer clone.
contract_base=$(sed -n 's/^CONTRACT_BASE_SHA=//p' "$scripts_dir/verify-contract-generation.sh")
[[ $contract_base =~ ^[0-9a-f]{40}$ ]]
git -C "$scripts_dir" merge-base --is-ancestor "$contract_base" HEAD
cp "$scripts_dir/verify-contract-generation.sh" "$tmp/unreachable-contract-baseline.sh"
sed -i "s/^CONTRACT_BASE_SHA=.*/CONTRACT_BASE_SHA=0000000000000000000000000000000000000000/" \
  "$tmp/unreachable-contract-baseline.sh"
unreachable_base=$(sed -n 's/^CONTRACT_BASE_SHA=//p' "$tmp/unreachable-contract-baseline.sh")
expect_exit 128 git -C "$scripts_dir" merge-base --is-ancestor "$unreachable_base" HEAD

# Reproducibility evidence must reject a hash mismatch and accept equal manifests.
printf '{"artifacts":{"rust":"aaa","python":"bbb"}}\n' >"$tmp/build-a.json"
printf '{"artifacts":{"rust":"ccc","python":"bbb"}}\n' >"$tmp/build-b.json"
expect_exit 1 "$scripts_dir/verify-reproducibility.sh" --verify-manifests \
  "$tmp/build-a.json" "$tmp/build-b.json"
cp "$tmp/build-a.json" "$tmp/build-b.json"
expect_exit 0 "$scripts_dir/verify-reproducibility.sh" --verify-manifests \
  "$tmp/build-a.json" "$tmp/build-b.json"

# A local Python project has identical dependency content in different materialization roots.
mkdir -p "$tmp/freeze-a/python" "$tmp/freeze-b/python"
printf 'grpcio==1.73.1\nficant-sdk @ file://%s\n' "$tmp/freeze-a/python" >"$tmp/freeze-a.raw"
printf 'ficant-sdk @ file://%s\ngrpcio==1.73.1\n' "$tmp/freeze-b/python" >"$tmp/freeze-b.raw"
expect_exit 0 "$scripts_dir/verify-reproducibility.sh" --normalize-python-freeze \
  "$tmp/freeze-a/python" "$tmp/freeze-a.raw" "$tmp/freeze-a.txt"
expect_exit 0 "$scripts_dir/verify-reproducibility.sh" --normalize-python-freeze \
  "$tmp/freeze-b/python" "$tmp/freeze-b.raw" "$tmp/freeze-b.txt"
cmp "$tmp/freeze-a.txt" "$tmp/freeze-b.txt"
grep -Fx 'ficant-sdk @ file://<SOURCE_ROOT>' "$tmp/freeze-a.txt" >/dev/null

# Python wheel/sdist evidence includes source while ignoring filesystem timestamps.
mkdir -p "$tmp/python-a/node-contracts/src/ficant_contracts"
cat >"$tmp/python-a/pyproject.toml" <<'TOML'
[project]
name = "fixture-runtime"
version = "1.2.3"
requires-python = "==3.12.*"
dependencies = []

[tool.uv]
package = false
TOML
printf 'version = 1\nrevision = 1\nrequires-python = "==3.12.*"\n' >"$tmp/python-a/uv.lock"
printf 'VALUE = 1\n' >"$tmp/python-a/node-contracts/src/ficant_contracts/example.py"
cp -R "$tmp/python-a" "$tmp/python-b"
expect_exit 0 "$scripts_dir/verify-reproducibility.sh" --python-build-manifest \
  "$tmp/python-a" "$tmp/python-dist-a" "$tmp/python-a.json"
wheel_files=("$tmp/python-dist-a"/*.whl)
sdist_files=("$tmp/python-dist-a"/*.tar.gz)
[[ ${#wheel_files[@]} -eq 1 && -f ${wheel_files[0]} ]]
[[ ${#sdist_files[@]} -eq 1 && -f ${sdist_files[0]} ]]
python3 -m zipfile -t "${wheel_files[0]}" >/dev/null
tar -tzf "${sdist_files[0]}" >/dev/null
touch -d '2030-01-01 00:00:00Z' "$tmp/python-b/node-contracts/src/ficant_contracts/example.py"
expect_exit 0 "$scripts_dir/verify-reproducibility.sh" --python-build-manifest \
  "$tmp/python-b" "$tmp/python-dist-b" "$tmp/python-b.json"
expect_exit 0 "$scripts_dir/verify-reproducibility.sh" --verify-manifests \
  "$tmp/python-a.json" "$tmp/python-b.json"
printf 'VALUE = 2\n' >"$tmp/python-b/node-contracts/src/ficant_contracts/example.py"
expect_exit 0 "$scripts_dir/verify-reproducibility.sh" --python-build-manifest \
  "$tmp/python-b" "$tmp/python-dist-b" "$tmp/python-b.json"
expect_exit 1 "$scripts_dir/verify-reproducibility.sh" --verify-manifests \
  "$tmp/python-a.json" "$tmp/python-b.json"

# Supply-chain validation uses fixed fixtures and never contacts the network here.
bash "$fixtures_dir/license-inventory/run.sh" "$scripts_dir/verify-license-inventory.py" "$scripts_dir/supply-chain.lock.json"
bash "$fixtures_dir/risk-acceptance/run.sh" "$scripts_dir/verify-risk-acceptance.py" "$scripts_dir/supply-chain.lock.json"
prepare_supply_fixture() {
  local source=$1 name=$2 mode=$3
  local target="$tmp/supply-$name"
  cp -R "$source" "$target"
  python3 - "$scripts_dir/license-inventory.lock.json" "$target/packages.syft.json" "$target/release-provenance.json" "$mode" <<'PY'
import hashlib,json,pathlib,sys
inventory_path,syft_path,provenance_path,mode=map(pathlib.Path,sys.argv[1:])
inventory=json.loads(inventory_path.read_text(encoding="utf-8"))
artifacts=[]
for index,item in enumerate(inventory["packages"]):
    expression="GPL-3.0-only" if mode.name == "override" and index == 0 else "NOASSERTION"
    artifacts.append({"name":item["name"],"version":item["version"],"purl":item["purl"],"licenses":[{"value":expression}]})
syft_path.write_text(json.dumps({"artifacts":artifacts},sort_keys=True,separators=(",",":"))+"\n",encoding="utf-8")
provenance=json.loads(provenance_path.read_text(encoding="utf-8"))
provenance["license_inventory"]={"digest":inventory["inventory_digest"],"file_sha256":hashlib.sha256(inventory_path.read_bytes()).hexdigest(),"generator":inventory["generator"]}
provenance_path.write_text(json.dumps(provenance,sort_keys=True,separators=(",",":"))+"\n",encoding="utf-8")
PY
  printf '%s\n' "$target"
}
pass_supply=$(prepare_supply_fixture "$fixtures_dir/pass" pass noassertion)
high_supply=$(prepare_supply_fixture "$fixtures_dir/high-vulnerability" high noassertion)
secret_supply=$(prepare_supply_fixture "$fixtures_dir/secret" secret noassertion)
override_supply=$(prepare_supply_fixture "$fixtures_dir/license" override override)
expect_exit 0 "$scripts_dir/verify-supply-chain.sh" --verify-evidence "$pass_supply"
expect_exit 1 "$scripts_dir/verify-supply-chain.sh" --verify-evidence "$high_supply"
expect_exit 1 "$scripts_dir/verify-supply-chain.sh" --verify-evidence "$secret_supply"
expect_exit 0 "$scripts_dir/verify-supply-chain.sh" --verify-evidence "$override_supply"
expect_exit 2 "$scripts_dir/verify-supply-chain.sh" --verify-evidence "$fixtures_dir/malformed"
expect_exit 2 "$scripts_dir/verify-supply-chain.sh" --verify-tool-cache "$fixtures_dir/missing-tools"
expect_exit 2 "$scripts_dir/verify-supply-chain.sh" --verify-db-cache "$fixtures_dir/hash-mismatch"

# Missing cache files are a normal cache-miss probe and must not emit shell
# redirection noise before the verified downloader runs.
set +e
"$scripts_dir/verify-supply-chain.sh" --cache-file-size "$tmp/not-created.zip" >"$tmp/cache-size.out" 2>"$tmp/cache-size.err"
cache_size_rc=$?
set -e
[[ $cache_size_rc -eq 0 && $(cat "$tmp/cache-size.out") == 0 && ! -s $tmp/cache-size.err ]]

# Locked downloads retry transport failures, stop after a fixed bound, and
# never retry an integrity mismatch into acceptance.
start_download_stub() {
  local mode=$1 payload=$2 name=$3
  local port_file="$tmp/$name.port" count_file="$tmp/$name.count"
  python3 - "$mode" "$payload" "$port_file" "$count_file" <<'PY' &
import http.server
import pathlib
import socketserver
import sys

mode, payload, port_file, count_file = sys.argv[1:]
count = 0

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        global count
        count += 1
        pathlib.Path(count_file).write_text(str(count), encoding="ascii")
        if mode == "permanent" or (mode == "transient" and count == 1):
            self.connection.shutdown(2)
            self.connection.close()
            return
        data = payload.encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_):
        pass

with socketserver.TCPServer(("127.0.0.1", 0), Handler) as server:
    pathlib.Path(port_file).write_text(str(server.server_address[1]), encoding="ascii")
    server.serve_forever()
PY
  download_stub_pid=$!
  for _ in {1..100}; do [[ -s $port_file ]] && break; sleep 0.02; done
  [[ -s $port_file ]]
  download_stub_url="http://127.0.0.1:$(cat "$port_file")/asset"
  download_stub_count=$count_file
}

payload='frozen-download-fixture'
payload_sha=$(printf '%s' "$payload" | sha256sum | awk '{print $1}')
start_download_stub transient "$payload" transient
expect_exit 0 "$scripts_dir/verify-supply-chain.sh" --download-verified "$download_stub_url" "$tmp/transient.bin" "$payload_sha" "${#payload}"
kill "$download_stub_pid" 2>/dev/null || true
wait "$download_stub_pid" 2>/dev/null || true
[[ $(cat "$download_stub_count") -eq 2 ]]

start_download_stub permanent "$payload" permanent
expect_exit 2 "$scripts_dir/verify-supply-chain.sh" --download-verified "$download_stub_url" "$tmp/permanent.bin" "$payload_sha" "${#payload}"
kill "$download_stub_pid" 2>/dev/null || true
wait "$download_stub_pid" 2>/dev/null || true
[[ $(cat "$download_stub_count") -eq 3 && ! -e $tmp/permanent.bin && ! -e $tmp/permanent.bin.tmp ]]

start_download_stub success "$payload" mismatch
expect_exit 2 "$scripts_dir/verify-supply-chain.sh" --download-verified "$download_stub_url" "$tmp/mismatch.bin" "$(printf bad | sha256sum | awk '{print $1}')" "${#payload}"
kill "$download_stub_pid" 2>/dev/null || true
wait "$download_stub_pid" 2>/dev/null || true
[[ $(cat "$download_stub_count") -eq 1 && ! -e $tmp/mismatch.bin && ! -e $tmp/mismatch.bin.tmp ]]

printf 'gate fixture tests: PASS\n'
