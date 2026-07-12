#!/usr/bin/env bash
set -euo pipefail
tool=${1:?reachability tool required}
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/project/reachable"
cp "$root/Cargo.lock.fixture" "$tmp/project/Cargo.lock"
cp "$root/Cargo.toml.fixture" "$tmp/project/Cargo.toml"
cp "$root/reachable/Cargo.toml.fixture" "$tmp/project/reachable/Cargo.toml"
cargo_lock="$tmp/project/Cargo.lock"
manifest_root="$tmp/project"
python3 "$tool" generate --resolved-graph "$root/resolved-tree.txt" --cargo-lock "$cargo_lock" \
  --manifest-root "$manifest_root" --cargo-version 'cargo 1.96.1 (fixture)' --output "$tmp/pass.json"
python3 "$tool" verify --evidence "$tmp/pass.json" --resolved-graph "$root/resolved-tree.txt" \
  --cargo-lock "$cargo_lock" --manifest-root "$manifest_root" --cargo-version 'cargo 1.96.1 (fixture)'
expect_fail() { local name=$1; shift; if "$@" >"$tmp/$name.out" 2>"$tmp/$name.err"; then echo "reachability fixture unexpectedly passed: $name" >&2; exit 1; fi; }
mutate() { cp "$tmp/pass.json" "$tmp/$1.json"; python3 - "$tmp/$1.json" "$2" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); x=json.loads(p.read_text()); exec(sys.argv[2],{"data":x}); p.write_text(json.dumps(x,sort_keys=True,separators=(",",":"))+"\n")
PY
}
mutate forged 'data["unreachable_lock_only"]=[]'
expect_fail forged python3 "$tool" verify --evidence "$tmp/forged.json" --resolved-graph "$root/resolved-tree.txt" --cargo-lock "$cargo_lock" --manifest-root "$manifest_root" --cargo-version 'cargo 1.96.1 (fixture)'
mutate reachable-mislabel 'item=data["reachable"].pop(); data["unreachable_lock_only"].append(item)'
expect_fail reachable-mislabel python3 "$tool" verify --evidence "$tmp/reachable-mislabel.json" --resolved-graph "$root/resolved-tree.txt" --cargo-lock "$cargo_lock" --manifest-root "$manifest_root" --cargo-version 'cargo 1.96.1 (fixture)'
mutate metadata-hash 'data["resolved_graph_sha256"]="0"*64'
expect_fail metadata-hash python3 "$tool" verify --evidence "$tmp/metadata-hash.json" --resolved-graph "$root/resolved-tree.txt" --cargo-lock "$cargo_lock" --manifest-root "$manifest_root" --cargo-version 'cargo 1.96.1 (fixture)'
mutate manifest-hash 'data["manifests_digest"]="0"*64'
expect_fail manifest-hash python3 "$tool" verify --evidence "$tmp/manifest-hash.json" --resolved-graph "$root/resolved-tree.txt" --cargo-lock "$cargo_lock" --manifest-root "$manifest_root" --cargo-version 'cargo 1.96.1 (fixture)'
mutate target 'data["configuration"]["all_features"]=False'
expect_fail target python3 "$tool" verify --evidence "$tmp/target.json" --resolved-graph "$root/resolved-tree.txt" --cargo-lock "$cargo_lock" --manifest-root "$manifest_root" --cargo-version 'cargo 1.96.1 (fixture)'
mutate tool 'data["cargo_version"]="cargo 1.95.0"'
expect_fail tool python3 "$tool" verify --evidence "$tmp/tool.json" --resolved-graph "$root/resolved-tree.txt" --cargo-lock "$cargo_lock" --manifest-root "$manifest_root" --cargo-version 'cargo 1.96.1 (fixture)'
cp "$cargo_lock" "$tmp/Cargo.lock"; printf '\n# drift\n' >>"$tmp/Cargo.lock"
expect_fail lock-hash python3 "$tool" verify --evidence "$tmp/pass.json" --resolved-graph "$root/resolved-tree.txt" --cargo-lock "$tmp/Cargo.lock" --manifest-root "$manifest_root" --cargo-version 'cargo 1.96.1 (fixture)'
printf 'cargo reachability fixtures: PASS\n'
