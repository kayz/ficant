#!/usr/bin/env bash

set -euo pipefail

scripts_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
lock_file="$scripts_dir/supply-chain.lock.json"
SUPPLY_LOCK_SHA256=3743b82b32cc63dbc81bb02c9ab0698bda0ff02490106c1d9bf9dd976134f8fe

die() {
  printf 'supply-chain: %s\n' "$1" >&2
  exit 2
}

sha256_file() {
  python3 - "$1" <<'PY'
import hashlib, pathlib, sys
path = pathlib.Path(sys.argv[1])
if not path.is_file():
    raise SystemExit(2)
h = hashlib.sha256()
with path.open("rb") as stream:
    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
        h.update(chunk)
print(h.hexdigest())
PY
}

cache_file_size() {
  [[ $# -eq 1 ]] || die 'cache_file_size requires a path'
  if [[ -f $1 ]]; then
    wc -c <"$1"
  else
    printf '0\n'
  fi
}

verify_lock() {
  local lock_sha
  lock_sha=$(sha256_file "$lock_file") || die 'cannot hash supply-chain lock'
  [[ $lock_sha == "$SUPPLY_LOCK_SHA256" ]] || die "supply-chain lock hash mismatch: $lock_sha"
  python3 - "$lock_file" <<'PY'
import datetime, hashlib, json, pathlib, sys, urllib.parse
try:
    data = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
except Exception as exc:
    print(f"supply-chain: invalid lock: {exc}", file=sys.stderr); raise SystemExit(2)
if data.get("schema_version") != 1 or len(data.get("tools", [])) != 3:
    print("supply-chain: invalid tool lock", file=sys.stderr); raise SystemExit(2)
if data.get("release_topology") != {
    "trusted_base_source": "ci-event-or-default-branch",
    "candidate_commit_count": "derived-from-positive-linear-range",
    "main_update": "squash-merge-only-after-final-consistency-audit",
}:
    print("supply-chain: frozen release topology mismatch", file=sys.stderr); raise SystemExit(2)
if data.get("cargo_reachability") != {
    "cargo_version": "1.96.1",
    "command": ["tree", "--locked", "--all-features", "--target", "all", "--prefix", "none", "--format", "{p}"],
}:
    print("supply-chain: frozen Cargo reachability contract mismatch", file=sys.stderr); raise SystemExit(2)
if len(data.get("first_party_packages", [])) != 16 or len({item.get("purl") for item in data["first_party_packages"]}) != 16:
    print("supply-chain: exact first-party policy mismatch", file=sys.stderr); raise SystemExit(2)
if {item.get("purl") for item in data.get("license_scoped_exceptions", [])} != {
    "pkg:cargo/webpki-roots@0.26.11", "pkg:cargo/webpki-root-certs@1.0.9",
    "pkg:cargo/webpki-roots@1.0.9", "pkg:npm/caniuse-lite@1.0.30001805",
}:
    print("supply-chain: scoped license exception set mismatch", file=sys.stderr); raise SystemExit(2)
acceptances = data.get("risk_acceptances", [])
if acceptances != []:
    print("supply-chain: active risk acceptance set must be empty", file=sys.stderr); raise SystemExit(2)
expected_tools = {
    "osv-scanner": ("2.4.0", "Apache-2.0", "15314940c10d26af9c6649f150b8a47c1262e8fc7e17b1d1029b0e479e8ed8a0"),
    "syft": ("1.46.0", "Apache-2.0", "d654f678b709eb53c393d38519d5ed7d2e57205529404018614cfefa0fb2b5ca"),
    "gitleaks": ("8.28.0", "MIT", "a65b5253807a68ac0cafa4414031fd740aeb55f54fb7e55f386acb52e6a840eb"),
}
if {tool.get("name"): (tool.get("version"), tool.get("license"), tool.get("sha256")) for tool in data["tools"]} != expected_tools:
    print("supply-chain: frozen tool matrix mismatch", file=sys.stderr); raise SystemExit(2)
dbs = data.get("osv_snapshot", {}).get("databases", [])
if [db.get("ecosystem") for db in dbs] != ["crates.io", "PyPI", "npm"]:
    print("supply-chain: invalid OSV ecosystem set/order", file=sys.stderr); raise SystemExit(2)
captured = datetime.datetime.fromisoformat(data["captured_utc"].replace("Z", "+00:00"))
lines = "".join(f"{db['ecosystem']}\t{db['generation']}\t{db['sha256']}\n" for db in dbs)
actual = hashlib.sha256(lines.encode()).hexdigest()
if actual != data["osv_snapshot"].get("aggregate_sha256"):
    print("supply-chain: OSV aggregate mismatch", file=sys.stderr); raise SystemExit(2)
for item in data["tools"] + dbs:
    value = item.get("sha256", "")
    if len(value) != 64 or any(c not in "0123456789abcdef" for c in value):
        print("supply-chain: invalid SHA-256 in lock", file=sys.stderr); raise SystemExit(2)
for db in dbs:
    updated = datetime.datetime.fromisoformat(db["updated_utc"].replace("Z", "+00:00"))
    if updated > captured or captured - updated > datetime.timedelta(hours=24):
        print("supply-chain: OSV snapshot was not fresh when captured", file=sys.stderr); raise SystemExit(2)
    parsed = urllib.parse.urlparse(db["official_url"])
    generation = urllib.parse.parse_qs(parsed.query).get("generation", [])
    if parsed.scheme != "https" or parsed.netloc != "storage.googleapis.com" or generation != [db["generation"]] or urllib.parse.parse_qs(parsed.query).get("alt") != ["media"]:
        print("supply-chain: OSV URL is not generation-pinned", file=sys.stderr); raise SystemExit(2)
    mirror = urllib.parse.urlparse(db.get("mirror_url", ""))
    expected_path = f"/kayz/ficant/releases/download/supply-chain-osv-2026-07-19/{db['asset']}"
    if mirror.scheme != "https" or mirror.netloc != "github.com" or mirror.path != expected_path or mirror.query or mirror.fragment:
        print("supply-chain: OSV mirror URL is not release-pinned", file=sys.stderr); raise SystemExit(2)
    if not isinstance(db.get("size"), int) or db["size"] <= 0:
        print("supply-chain: invalid OSV object size", file=sys.stderr); raise SystemExit(2)
PY
}

verify_tool_cache() {
  [[ $# -eq 1 ]] || die '--verify-tool-cache requires a directory'
  python3 - "$lock_file" "$1" <<'PY'
import hashlib, json, pathlib, sys
lock = json.loads(pathlib.Path(sys.argv[1]).read_text())
root = pathlib.Path(sys.argv[2])
for tool in lock["tools"]:
    path = root / tool["asset"]
    if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != tool["sha256"]:
        print(f"supply-chain: missing or invalid tool asset: {tool['asset']}", file=sys.stderr)
        raise SystemExit(2)
PY
}

verify_db_cache() {
  [[ $# -eq 1 ]] || die '--verify-db-cache requires a directory'
  python3 - "$lock_file" "$1" <<'PY'
import hashlib, json, pathlib, sys
lock = json.loads(pathlib.Path(sys.argv[1]).read_text())
root = pathlib.Path(sys.argv[2])
for db in lock["osv_snapshot"]["databases"]:
    path = root / db["asset"]
    if not path.is_file() or path.stat().st_size != db["size"] or hashlib.sha256(path.read_bytes()).hexdigest() != db["sha256"]:
        print(f"supply-chain: missing or invalid OSV snapshot: {db['asset']}", file=sys.stderr)
        raise SystemExit(2)
PY
}

verify_release_topology() {
  [[ $# -ge 3 && $# -le 4 ]] || die 'verify_release_topology requires repo, trusted base, candidate, and optional exact commit count'
  local repo=$1 trusted_base=$2 candidate=$3 expected_count=${4:-} count
  [[ $trusted_base =~ ^[0-9a-f]{40}$ && $candidate =~ ^[0-9a-f]{40}$ ]] || die 'release topology SHA invalid'
  [[ -z $expected_count || $expected_count =~ ^[1-9][0-9]*$ ]] || die 'release candidate commit count invalid'
  git -C "$repo" cat-file -e "$trusted_base^{commit}" 2>/dev/null || die 'trusted release base object missing'
  git -C "$repo" cat-file -e "$candidate^{commit}" 2>/dev/null || die 'release candidate object missing'
  git -C "$repo" merge-base --is-ancestor "$trusted_base" "$candidate" \
    || die 'release candidate does not descend from the trusted base'
  [[ -z $(git -C "$repo" rev-list --min-parents=2 "$trusted_base..$candidate") ]] \
    || die 'release candidate range must remain linear without merge commits'
  count=$(git -C "$repo" rev-list --count "$trusted_base..$candidate") || die 'cannot count release candidate commits'
  [[ $count -gt 0 ]] || die 'release candidate range must contain at least one forward-only commit'
  [[ -z $expected_count || $count -eq $expected_count ]] \
    || die "release candidate must contain exactly $expected_count forward-only commit(s)"
  git -C "$repo" rev-parse "$candidate^{tree}"
}

resolve_trusted_base() {
  [[ $# -eq 2 ]] || die 'resolve_trusted_base requires repo and candidate'
  local repo=$1 candidate=$2 trusted_base=${FICANT_TRUSTED_BASE:-} default_branch=${FICANT_DEFAULT_BRANCH:-main}
  local event_name=${FICANT_EVENT_NAME:-local} ref_name=${FICANT_REF_NAME:-} default_tip
  git -C "$repo" check-ref-format "refs/heads/$default_branch" >/dev/null 2>&1 \
    || die 'default branch name invalid'
  if [[ $event_name == push && -n $ref_name && $ref_name != "$default_branch" ]]; then
    default_tip=$(git -C "$repo" rev-parse "origin/$default_branch^{commit}" 2>/dev/null) \
      || die 'cannot resolve default branch for feature push'
    trusted_base=$(git -C "$repo" merge-base "$candidate" "$default_tip" 2>/dev/null) \
      || die 'cannot resolve feature push base'
  elif [[ ! $trusted_base =~ ^[0-9a-f]{40}$ || $trusted_base == 0000000000000000000000000000000000000000 ]]; then
    trusted_base=$(git -C "$repo" rev-parse "origin/$default_branch^{commit}" 2>/dev/null) \
      || die 'cannot resolve trusted base from CI event or default branch'
  fi
  if [[ $trusted_base == "$candidate" ]]; then
    trusted_base=$(git -C "$repo" rev-parse "$candidate^" 2>/dev/null) \
      || die 'candidate has no trusted predecessor'
  fi
  printf '%s\n' "$trusted_base"
}

scan_release_secrets() {
  [[ $# -eq 6 ]] || die 'scan_release_secrets requires repo, base, candidate, tree, gitleaks, and output'
  local repo=$1 trusted_base=$2 candidate=$3 release_tree=$4 gitleaks=$5 output=$6
  local base_rc range_rc tree_rc
  mkdir -p "$output"
  set +e
  "$gitleaks" git --no-banner --redact --report-format json --report-path "$output/secrets-base.json" --log-opts="$trusted_base" "$repo"
  base_rc=$?
  "$gitleaks" git --no-banner --redact --report-format json --report-path "$output/secrets-range.json" --log-opts="$trusted_base..$candidate" "$repo"
  range_rc=$?
  "$gitleaks" dir --no-banner --redact --report-format json --report-path "$output/secrets-dir.json" "$release_tree"
  tree_rc=$?
  set -e
  [[ $base_rc -eq 0 || $base_rc -eq 1 ]] || die "Gitleaks trusted base failed with exit $base_rc"
  [[ $range_rc -eq 0 || $range_rc -eq 1 ]] || die "Gitleaks candidate range failed with exit $range_rc"
  [[ $tree_rc -eq 0 || $tree_rc -eq 1 ]] || die "Gitleaks release tree failed with exit $tree_rc"
  [[ -f $output/secrets-base.json ]] || printf '[]\n' >"$output/secrets-base.json"
  [[ -f $output/secrets-range.json ]] || printf '[]\n' >"$output/secrets-range.json"
  [[ -f $output/secrets-dir.json ]] || printf '[]\n' >"$output/secrets-dir.json"
  if [[ $base_rc -eq 1 || $range_rc -eq 1 || $tree_rc -eq 1 ]]; then
    return 1
  fi
}

verify_release_fixture() {
  [[ $# -ge 4 && $# -le 5 ]] || die '--verify-release-fixture requires repo, trusted base, candidate, Gitleaks, and optional exact commit count'
  local repo=$1 trusted_base=$2 candidate=$3 gitleaks=$4 expected_count=${5:-} output
  verify_release_topology "$repo" "$trusted_base" "$candidate" "$expected_count" >/dev/null
  output=$(mktemp -d)
  scan_release_secrets "$repo" "$trusted_base" "$candidate" "$repo" "$gitleaks" "$output"
  local rc=$?
  rm -rf "$output"
  return "$rc"
}

verify_syft_scope_fixture() {
  [[ $# -eq 1 ]] || die '--verify-syft-scope-fixture requires Syft'
  local syft=$1 root
  root=$(mktemp -d)
  mkdir -p "$root/production" "$root/test-fixtures"
  cat >"$root/production/Cargo.lock" <<'EOF'
version = 4

[[package]]
name = "ordinary-production"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
EOF
  cat >"$root/test-fixtures/Cargo.lock.fixture" <<'EOF'
version = 4

[[package]]
name = "template-only"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
EOF
  "$syft" scan "dir:$root" -o "syft-json=$root/packages.json" >/dev/null || { rm -rf "$root"; die 'Syft scope fixture scan failed'; }
  if ! python3 - "$root/packages.json" <<'PY'
import json, pathlib, sys
artifacts=json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")).get("artifacts", [])
purls=[item.get("purl") for item in artifacts]
if purls.count("pkg:cargo/ordinary-production@1.0.0") != 1 or "pkg:cargo/template-only@1.0.0" in purls:
    print("supply-chain: Syft scope fixture isolation failed", file=sys.stderr); raise SystemExit(2)
PY
  then
    rm -rf "$root"
    return 2
  fi
  rm -rf "$root"
}

verify_evidence() {
  [[ $# -ge 1 && $# -le 2 ]] || die '--verify-evidence requires an evidence directory and optional release root'
  local evidence_root=$1 release_root native_lf=()
  release_root=${2:-$(CDPATH= cd -- "$scripts_dir/../.." && pwd)}
  [[ $# -eq 2 ]] && native_lf=(--require-native-lf)
  python3 "$scripts_dir/verify-license-inventory.py" verify --inventory "$scripts_dir/license-inventory.lock.json" \
    --syft "$evidence_root/packages.syft.json" --cargo-lock "$release_root/Cargo.lock" --uv-lock "$release_root/python/uv.lock" \
    --pnpm-lock "$release_root/web-dm/pnpm-lock.yaml" --supply-lock "$lock_file" --release-root "$release_root" \
    --require-first-party "${native_lf[@]}" >/dev/null || die 'authoritative license inventory verification failed'
  if [[ -f $1/accepted-unfixed.json ]]; then
    python3 "$scripts_dir/verify-risk-acceptance.py" verify --supply-lock "$lock_file" \
      --vulnerabilities "$1/vulnerabilities.json" --reachability "$1/cargo-reachability.json" \
      --chain "$1/cargo-async-std-chain.txt" --output "$1/accepted-unfixed.json" \
      || die 'accepted-unfixed evidence verification failed'
  fi
  python3 - "$lock_file" "$1" "$scripts_dir/license-inventory.lock.json" <<'PY'
import hashlib, json, math, pathlib, re, sys

lock = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
root = pathlib.Path(sys.argv[2])
inventory_path = pathlib.Path(sys.argv[3])
inventory = json.loads(inventory_path.read_text(encoding="utf-8"))

def load(name, expected):
    try:
        value = json.loads((root / name).read_text(encoding="utf-8"))
    except Exception as exc:
        print(f"supply-chain: invalid {name}: {exc}", file=sys.stderr); raise SystemExit(2)
    if not isinstance(value, expected):
        print(f"supply-chain: wrong JSON shape: {name}", file=sys.stderr); raise SystemExit(2)
    return value

packages = load("packages.syft.json", dict).get("artifacts")
vulnerability_document = load("vulnerabilities.json", dict)
base_secrets = load("secrets-base.json", list)
range_secrets = load("secrets-range.json", list)
dir_secrets = load("secrets-dir.json", list)
provenance = load("release-provenance.json", dict)
reachability_path = root / "cargo-reachability.json"
reachability = load("cargo-reachability.json", dict) if reachability_path.is_file() else None
accepted_path = root / "accepted-unfixed.json"
accepted_document = load("accepted-unfixed.json", dict) if accepted_path.is_file() else None
if not isinstance(packages, list) or not packages:
    print("supply-chain: empty package inventory", file=sys.stderr); raise SystemExit(2)
results = vulnerability_document.get("results")
if not isinstance(results, list):
    print("supply-chain: vulnerability results missing", file=sys.stderr); raise SystemExit(2)
if vulnerability_document.get("schema_version") != 1:
    print("supply-chain: vulnerability evidence schema invalid", file=sys.stderr); raise SystemExit(2)
candidate = vulnerability_document.get("candidate")
if (not isinstance(candidate, dict)
        or not re.fullmatch(r"[0-9a-f]{40}", str(candidate.get("commit", "")))
        or not re.fullmatch(r"[0-9a-f]{40}", str(candidate.get("tree", "")))):
    print("supply-chain: vulnerability candidate binding invalid", file=sys.stderr); raise SystemExit(2)
scans = vulnerability_document.get("scans")
required_locks = ("Cargo.lock", "python/uv.lock", "web-dm/pnpm-lock.yaml")
if not isinstance(scans, list) or len(scans) != len(required_locks):
    print("supply-chain: vulnerability scan ledger incomplete", file=sys.stderr); raise SystemExit(2)
scan_counts = {}
for scan in scans:
    if not isinstance(scan, dict):
        print("supply-chain: vulnerability scan ledger invalid", file=sys.stderr); raise SystemExit(2)
    lock_path = scan.get("lock_path")
    lock_sha256 = scan.get("lock_sha256")
    result_count = scan.get("result_count")
    if lock_path not in required_locks or lock_path in scan_counts:
        print("supply-chain: vulnerability scan ledger path invalid", file=sys.stderr); raise SystemExit(2)
    if not isinstance(lock_sha256, str) or not re.fullmatch(r"[0-9a-f]{64}", lock_sha256):
        print("supply-chain: vulnerability scan ledger hash invalid", file=sys.stderr); raise SystemExit(2)
    if isinstance(result_count, bool) or not isinstance(result_count, int) or result_count < 0:
        print("supply-chain: vulnerability scan ledger count invalid", file=sys.stderr); raise SystemExit(2)
    scan_counts[lock_path] = result_count
if set(scan_counts) != set(required_locks):
    print("supply-chain: vulnerability scan ledger incomplete", file=sys.stderr); raise SystemExit(2)

expected_tools = [{"name": item["name"], "version": item["version"], "sha256": item["sha256"]} for item in lock["tools"]]
topology = provenance.get("topology")
if not isinstance(topology, dict) or topology.get("candidate_tree") != candidate["tree"]:
    print("supply-chain: release tree binding invalid", file=sys.stderr); raise SystemExit(2)
trusted_base = topology.get("trusted_base")
expected_commit_count = topology.get("commit_count")
if (not isinstance(trusted_base, str)
        or not re.fullmatch(r"[0-9a-f]{40}", trusted_base)
        or trusted_base == candidate["commit"]
        or isinstance(expected_commit_count, bool)
        or not isinstance(expected_commit_count, int)
        or expected_commit_count < 1):
    print("supply-chain: release topology binding invalid", file=sys.stderr); raise SystemExit(2)
if provenance.get("schema_version") != 1 or topology != {
    "trusted_base": trusted_base,
    "candidate": candidate["commit"],
    "parent": trusted_base,
    "candidate_tree": candidate["tree"],
    "commit_count": expected_commit_count,
} or provenance.get("tools") != expected_tools:
    print("supply-chain: release provenance binding invalid", file=sys.stderr); raise SystemExit(2)
license_binding = provenance.get("license_inventory")
if not isinstance(license_binding, dict) or {
    "digest": license_binding.get("digest"),
    "file_sha256": license_binding.get("file_sha256"),
    "generator": license_binding.get("generator"),
} != {
    "digest": inventory.get("inventory_digest"),
    "file_sha256": hashlib.sha256(inventory_path.read_bytes()).hexdigest(),
    "generator": inventory.get("generator"),
}:
    print("supply-chain: authoritative license inventory provenance mismatch", file=sys.stderr); raise SystemExit(2)
if reachability is not None and provenance.get("cargo_reachability") != {
    "evidence_sha256": hashlib.sha256(reachability_path.read_bytes()).hexdigest(),
    "cargo_lock_sha256": reachability.get("cargo_lock_sha256"),
    "manifests_digest": reachability.get("manifests_digest"),
    "resolved_graph_sha256": reachability.get("resolved_graph_sha256"),
}:
    print("supply-chain: Cargo reachability provenance mismatch", file=sys.stderr); raise SystemExit(2)
if accepted_document is not None:
    acceptance_provenance = provenance.get("accepted_unfixed")
    if acceptance_provenance != {
        "evidence_sha256": hashlib.sha256(accepted_path.read_bytes()).hexdigest(),
        "status": accepted_document.get("status"),
        "acceptance_ids": [item.get("id") for item in accepted_document.get("acceptances", [])],
    }:
        print("supply-chain: accepted-unfixed provenance mismatch", file=sys.stderr); raise SystemExit(2)
secret_reports = {
    "published_base_history": (trusted_base, "secrets-base.json", base_secrets),
    "candidate_range": (f"{trusted_base}..{candidate['commit']}", "secrets-range.json", range_secrets),
    "release_tree": (candidate["tree"], "secrets-dir.json", dir_secrets),
}
secret_scans = provenance.get("secret_scans")
if not isinstance(secret_scans, list) or len(secret_scans) != len(secret_reports):
    print("supply-chain: secret scan provenance incomplete", file=sys.stderr); raise SystemExit(2)
seen_secret_scans = set()
for scan in secret_scans:
    if not isinstance(scan, dict) or scan.get("kind") not in secret_reports or scan["kind"] in seen_secret_scans:
        print("supply-chain: secret scan provenance invalid", file=sys.stderr); raise SystemExit(2)
    seen_secret_scans.add(scan["kind"])
    scope, report_name, findings = secret_reports[scan["kind"]]
    report_hash = hashlib.sha256((root / report_name).read_bytes()).hexdigest()
    if scan != {"kind": scan["kind"], "scope": scope, "report": report_name, "sha256": report_hash, "finding_count": len(findings)}:
        print("supply-chain: secret scan provenance mismatch", file=sys.stderr); raise SystemExit(2)
if seen_secret_scans != set(secret_reports):
    print("supply-chain: secret scan provenance incomplete", file=sys.stderr); raise SystemExit(2)

def round_up_1(value):
    return math.ceil(value * 10.0 - 1e-10) / 10.0

def cvss3(vector):
    metrics = dict(part.split(":", 1) for part in vector.split("/")[1:] if ":" in part)
    av = {"N": .85, "A": .62, "L": .55, "P": .2}[metrics["AV"]]
    ac = {"L": .77, "H": .44}[metrics["AC"]]
    scope = metrics["S"]
    pr = ({"N": .85, "L": .62, "H": .27} if scope == "U" else {"N": .85, "L": .68, "H": .5})[metrics["PR"]]
    ui = {"N": .85, "R": .62}[metrics["UI"]]
    impact_metric = {"N": 0.0, "L": .22, "H": .56}
    iss = 1 - ((1-impact_metric[metrics["C"]]) * (1-impact_metric[metrics["I"]]) * (1-impact_metric[metrics["A"]]))
    impact = 6.42 * iss if scope == "U" else 7.52 * (iss-.029) - 3.25 * ((iss-.02) ** 15)
    if impact <= 0: return 0.0
    exploit = 8.22 * av * ac * pr * ui
    return round_up_1(min(impact + exploit, 10) if scope == "U" else min(1.08 * (impact + exploit), 10))

def severity(vulnerability):
    label = str(vulnerability.get("databaseSpecific", {}).get("severity", "")).upper()
    if label in {"CRITICAL", "HIGH", "MODERATE", "MEDIUM", "LOW"}:
        return {"CRITICAL": 10.0, "HIGH": 7.0, "MODERATE": 4.0, "MEDIUM": 4.0, "LOW": .1}[label]
    scores = []
    for item in vulnerability.get("severity", []) or []:
        score = item.get("score")
        try:
            if isinstance(score, (int, float)) or re.fullmatch(r"\d+(\.\d+)?", str(score)):
                scores.append(float(score))
            elif isinstance(score, str) and score.startswith(("CVSS:3.0/", "CVSS:3.1/")):
                scores.append(cvss3(score))
        except (KeyError, ValueError, ZeroDivisionError):
            pass
    return max(scores) if scores else None

vulnerability_findings = []
accepted_keys = set()
if accepted_document is not None:
    if accepted_document.get("status") not in {"none", "accepted-unfixed"}: print("supply-chain: invalid accepted-unfixed status", file=sys.stderr); raise SystemExit(2)
    accepted_keys = {(item.get("purl"), vulnerability_id) for item in accepted_document.get("acceptances", []) for vulnerability_id in item.get("vulnerability_ids", [])}
reachable_cargo = set()
unreachable_cargo = set()
if reachability is not None:
    if reachability.get("schema_version") != 1 or reachability.get("configuration") != {"locked": True, "all_features": True, "target": "all", "command": "cargo tree", "format": "{p}"}:
        print("supply-chain: Cargo reachability evidence invalid", file=sys.stderr); raise SystemExit(2)
    reachable_cargo = {(item.get("name"), item.get("version")) for item in reachability.get("reachable", [])}
    unreachable_cargo = {(item.get("name"), item.get("version")) for item in reachability.get("unreachable_lock_only", [])}
    if not reachable_cargo or reachable_cargo & unreachable_cargo:
        print("supply-chain: Cargo reachability sets invalid", file=sys.stderr); raise SystemExit(2)
actual_result_counts = {path: 0 for path in required_locks}
for result in results:
    if not isinstance(result, dict):
        print("supply-chain: vulnerability result invalid", file=sys.stderr); raise SystemExit(2)
    source = result.get("source")
    if not isinstance(source, dict) or source.get("type") != "lockfile" or not isinstance(source.get("path"), str):
        print("supply-chain: vulnerability result source invalid", file=sys.stderr); raise SystemExit(2)
    normalized_source = pathlib.PurePath(source["path"]).as_posix()
    matches = [path for path in required_locks if normalized_source.endswith(path)]
    if len(matches) != 1:
        print("supply-chain: vulnerability result source unexpected", file=sys.stderr); raise SystemExit(2)
    actual_result_counts[matches[0]] += 1
    result_packages = result.get("packages")
    if not isinstance(result_packages, list):
        print("supply-chain: vulnerability result packages invalid", file=sys.stderr); raise SystemExit(2)
    for package in result_packages:
        if not isinstance(package, dict):
            print("supply-chain: vulnerability package invalid", file=sys.stderr); raise SystemExit(2)
        package_key = (package.get("package", {}).get("name"), package.get("package", {}).get("version"))
        package_ecosystem = package.get("package", {}).get("ecosystem")
        if reachability is not None and package_ecosystem == "crates.io" and package_key not in reachable_cargo | unreachable_cargo:
            print("supply-chain: OSV Cargo package absent from reachability evidence", file=sys.stderr); raise SystemExit(2)
        for vulnerability in package.get("vulnerabilities", []) or []:
            if reachability is not None and package_ecosystem == "crates.io" and package_key in unreachable_cargo:
                continue
            purl = f"pkg:cargo/{package_key[0]}@{package_key[1]}" if package_ecosystem == "crates.io" else None
            if (purl, str(vulnerability.get("id", "UNKNOWN"))) in accepted_keys:
                continue
            score = severity(vulnerability)
            if score is None or score >= 7.0:
                vulnerability_findings.append(f"{vulnerability.get('id', 'UNKNOWN')}:{score if score is not None else 'UNKNOWN'}")

if actual_result_counts != scan_counts:
    print("supply-chain: vulnerability scan ledger/result mismatch", file=sys.stderr); raise SystemExit(2)
purls = {str(package.get("purl", "")) for package in packages}
if not any(purl.startswith("pkg:cargo/") for purl in purls) or not any(purl.startswith("pkg:pypi/") for purl in purls) or not any(purl.startswith("pkg:npm/") for purl in purls):
    print("supply-chain: SBOM ecosystem coverage incomplete", file=sys.stderr); raise SystemExit(2)

findings = vulnerability_findings
if base_secrets or range_secrets or dir_secrets:
    findings.append(f"secrets:{len(base_secrets) + len(range_secrets) + len(dir_secrets)}")
if findings:
    print("supply-chain: blocking findings: " + ", ".join(findings), file=sys.stderr)
    raise SystemExit(1)
PY
}

download_verified() {
  [[ $# -eq 4 ]] || die 'download_verified requires URL, output, SHA-256, and size|-'
  python3 - "$1" "$2" "$3" "$4" <<'PY'
import hashlib
import pathlib
import sys
import time
import urllib.error
import urllib.request

url, output_name, expected_hash, expected_size_value = sys.argv[1:]
output = pathlib.Path(output_name)
temporary = output.with_suffix(output.suffix + ".tmp")
expected_size = None if expected_size_value == "-" else int(expected_size_value)
if len(expected_hash) != 64 or any(character not in "0123456789abcdef" for character in expected_hash):
    print("supply-chain: invalid expected download hash", file=sys.stderr)
    raise SystemExit(2)

for attempt in range(1, 4):
    temporary.unlink(missing_ok=True)
    try:
        urllib.request.urlretrieve(url, temporary)
    except (OSError, urllib.error.URLError) as error:
        temporary.unlink(missing_ok=True)
        print(f"supply-chain: download transport failure attempt {attempt}/3: {type(error).__name__}", file=sys.stderr)
        if attempt == 3:
            raise SystemExit(2)
        time.sleep(attempt)
        continue

    actual_size = temporary.stat().st_size
    actual_hash = hashlib.sha256(temporary.read_bytes()).hexdigest()
    if (expected_size is not None and actual_size != expected_size) or actual_hash != expected_hash:
        temporary.unlink(missing_ok=True)
        print("supply-chain: downloaded asset integrity mismatch", file=sys.stderr)
        raise SystemExit(2)
    temporary.replace(output)
    raise SystemExit(0)

raise SystemExit(2)
PY
}

case ${1:-} in
  --cache-file-size)
    shift; cache_file_size "$@"; exit $?
    ;;
  --download-verified)
    shift; download_verified "$@"; exit $?
    ;;
  --verify-evidence)
    shift; verify_lock; verify_evidence "$@"; exit $?
    ;;
  --verify-tool-cache)
    shift; verify_lock; verify_tool_cache "$@"; exit $?
    ;;
  --verify-db-cache)
    shift; verify_lock; verify_db_cache "$@"; exit $?
    ;;
  --verify-release-fixture)
    shift; verify_release_fixture "$@"; exit $?
    ;;
  --verify-syft-scope-fixture)
    shift; verify_syft_scope_fixture "$@"; exit $?
    ;;
esac
[[ $# -eq 0 ]] || die 'unexpected arguments'

verify_lock
repo=$(git rev-parse --show-toplevel 2>/dev/null) || die 'not in a Git worktree'
cd "$repo"
[[ -z $(git status --porcelain) ]] || die 'worktree must be clean'
candidate=$(git rev-parse HEAD) || die 'cannot identify candidate commit'
trusted_base=$(resolve_trusted_base "$repo" "$candidate")
candidate_tree=$(verify_release_topology "$repo" "$trusted_base" "$candidate")
candidate_commit_count=$(git -C "$repo" rev-list --count "$trusted_base..$candidate") \
  || die 'cannot count release candidate commits'
[[ $(git rev-parse --is-shallow-repository) == false ]] || die 'full Git history is required for secret scanning'
for command in cargo git python3 tar; do command -v "$command" >/dev/null || die "missing tool: $command"; done
[[ $(cargo --version) == cargo\ 1.96.1\ * ]] || die 'Cargo reachability tool version mismatch'

cache=${FICANT_GATE_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/ficant-supply-chain}
output=${FICANT_GATE_OUTPUT_DIR:-$(mktemp -d)}
mkdir -p "$cache/assets" "$cache/bin" "$cache/db" "$output"

while IFS=$'\t' read -r asset url sha256; do
  path="$cache/assets/$asset"
  if [[ ! -f $path || $(sha256_file "$path" 2>/dev/null || true) != "$sha256" ]]; then
    rm -f -- "$path" "$path.tmp"
    download_verified "$url" "$path" "$sha256" - || die "tool asset acquisition failed: $asset"
  fi
done < <(python3 - "$lock_file" <<'PY'
import json, pathlib, sys
lock = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
for item in lock["tools"]:
    print(item["asset"], item["url"], item["sha256"], sep="\t")
PY
)
verify_tool_cache "$cache/assets"

cp "$cache/assets/osv-scanner_linux_amd64" "$cache/bin/osv-scanner"
chmod 0755 "$cache/bin/osv-scanner"
tar -xzf "$cache/assets/syft_1.46.0_linux_amd64.tar.gz" -C "$cache/bin" syft || die 'cannot extract Syft'
tar -xzf "$cache/assets/gitleaks_8.28.0_linux_x64.tar.gz" -C "$cache/bin" gitleaks || die 'cannot extract Gitleaks'
chmod 0755 "$cache/bin/syft" "$cache/bin/gitleaks"
[[ $($cache/bin/osv-scanner --version) == *'2.4.0'* ]] || die 'OSV-Scanner version mismatch'
[[ $($cache/bin/syft version) == *'1.46.0'* ]] || die 'Syft version mismatch'
[[ $($cache/bin/gitleaks version) == *'8.28.0'* ]] || die 'Gitleaks version mismatch'
verify_syft_scope_fixture "$cache/bin/syft" || die 'Syft scope fixture failed'

while IFS=$'\t' read -r ecosystem asset url size sha256; do
  path="$cache/db/$asset"
  actual_size=$(cache_file_size "$path")
  if [[ ! -f $path || $actual_size -ne $size || $(sha256_file "$path" 2>/dev/null || true) != "$sha256" ]]; then
    rm -f -- "$path" "$path.tmp"
    download_verified "$url" "$path" "$sha256" "$size" || die "OSV snapshot acquisition failed: $ecosystem"
  fi
done < <(python3 - "$lock_file" <<'PY'
import json, pathlib, sys
lock = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
for item in lock["osv_snapshot"]["databases"]:
    print(item["ecosystem"], item["asset"], item["mirror_url"], item["size"], item["sha256"], sep="\t")
PY
)
verify_db_cache "$cache/db"

db_root=$(mktemp -d)
release_root=$(mktemp -d)
trap 'rm -rf "$db_root" "$release_root"' EXIT
git archive HEAD | tar -x -C "$release_root" || die 'cannot materialize tracked release tree'
if find "$release_root/.github/scripts/tests/fixtures" -type f \( -name Cargo.lock -o -name Cargo.toml -o -name uv.lock -o -name pnpm-lock.yaml \) -print -quit | grep -q .; then
  die 'recognized package manifest or lock leaked from test fixtures'
fi
for ecosystem in crates.io PyPI npm; do mkdir -p "$db_root/osv-scanner/$ecosystem"; done
cp "$cache/db/crates.io-all.zip" "$db_root/osv-scanner/crates.io/all.zip"
cp "$cache/db/PyPI-all.zip" "$db_root/osv-scanner/PyPI/all.zip"
cp "$cache/db/npm-all.zip" "$db_root/osv-scanner/npm/all.zip"

"$cache/bin/syft" scan "dir:$release_root" \
  -o "syft-json=$output/packages.syft.json" -o "cyclonedx-json=$output/sbom.cdx.json" || die 'Syft scan failed'

(cd "$release_root" && cargo tree --locked --all-features --target all --prefix none --format '{p}' | sort -u) >"$output/cargo-resolved-tree.txt" || die 'Cargo resolved graph failed'
if grep -q '^name = "async-std"$' "$release_root/Cargo.lock"; then
  (cd "$release_root" && cargo tree --locked --all-features --target all -i async-std --prefix none --format '{p}' | sort -u) >"$output/cargo-async-std-chain.txt" \
    || die 'async-std dependency chain failed'
else
  : >"$output/cargo-async-std-chain.txt"
fi
python3 "$scripts_dir/verify-cargo-reachability.py" generate --resolved-graph "$output/cargo-resolved-tree.txt" \
  --cargo-lock "$release_root/Cargo.lock" --manifest-root "$release_root" --cargo-version "$(cargo --version)" \
  --output "$output/cargo-reachability.json" || die 'Cargo reachability evidence failed'
python3 "$scripts_dir/verify-cargo-reachability.py" verify --evidence "$output/cargo-reachability.json" \
  --resolved-graph "$output/cargo-resolved-tree.txt" --cargo-lock "$release_root/Cargo.lock" --manifest-root "$release_root" \
  --cargo-version "$(cargo --version)" || die 'Cargo reachability verification failed'

license_inventory="$scripts_dir/license-inventory.lock.json"
python3 "$scripts_dir/verify-license-inventory.py" digest --inventory "$license_inventory" \
  >"$output/license-inventory.digest" || die 'license inventory digest failed'
python3 "$scripts_dir/verify-license-inventory.py" verify-notices --supply-lock "$lock_file" \
  --notice "$release_root/docs/delivery/third-party-notices.md" || die 'third-party notice verification failed'

osv_parts=()
for lock_path in Cargo.lock python/uv.lock web-dm/pnpm-lock.yaml; do
  part="$output/osv-$(printf '%s' "$lock_path" | tr '/.' '--').json"
  set +e
  OSV_SCANNER_LOCAL_DB_CACHE_DIRECTORY="$db_root" "$cache/bin/osv-scanner" scan source --offline --format json -L "$release_root/$lock_path" >"$part"
  rc=$?
  set -e
  [[ $rc -eq 0 || $rc -eq 1 ]] || die "OSV-Scanner failed for $lock_path with exit $rc"
  osv_parts+=("$part")
done
if ! python3 - "$output/vulnerabilities.json" "$candidate" "$candidate_tree" "$release_root" \
  Cargo.lock "${osv_parts[0]}" python/uv.lock "${osv_parts[1]}" \
  web-dm/pnpm-lock.yaml "${osv_parts[2]}" <<'PY'
import hashlib, json, pathlib, sys

output, candidate, candidate_tree, release_root, *scan_arguments = sys.argv[1:]
if len(scan_arguments) % 2:
    raise SystemExit(2)
merged = {"schema_version": 1, "candidate": {"commit": candidate, "tree": candidate_tree}, "scans": [], "results": []}
for index in range(0, len(scan_arguments), 2):
    lock_path, result_path = scan_arguments[index:index + 2]
    document = json.loads(pathlib.Path(result_path).read_text(encoding="utf-8"))
    if not isinstance(document, dict) or not isinstance(document.get("results"), list):
        raise SystemExit(2)
    results = document["results"]
    for result in results:
        if not isinstance(result, dict):
            raise SystemExit(2)
        source = result.get("source")
        source_path = source.get("path") if isinstance(source, dict) else None
        if not isinstance(source, dict) or source.get("type") != "lockfile" or not isinstance(source_path, str) or not pathlib.PurePath(source_path).as_posix().endswith(lock_path):
            raise SystemExit(2)
    lock_bytes = (pathlib.Path(release_root) / lock_path).read_bytes()
    merged["scans"].append({
        "lock_path": lock_path,
        "lock_sha256": hashlib.sha256(lock_bytes).hexdigest(),
        "result_count": len(results),
    })
    merged["results"].extend(results)
pathlib.Path(output).write_text(json.dumps(merged, sort_keys=True, separators=(",", ":")) + "\n")
PY
then
  die 'cannot merge OSV evidence'
fi

python3 "$scripts_dir/verify-risk-acceptance.py" generate --supply-lock "$lock_file" \
  --vulnerabilities "$output/vulnerabilities.json" --reachability "$output/cargo-reachability.json" \
  --chain "$output/cargo-async-std-chain.txt" --output "$output/accepted-unfixed.json" \
  || die 'accepted-unfixed evidence generation failed'

scan_release_secrets "$repo" "$trusted_base" "$candidate" "$release_root" "$cache/bin/gitleaks" "$output" || exit $?

if ! python3 - "$lock_file" "$output" "$trusted_base" "$candidate" "$candidate_tree" "$candidate_commit_count" "$license_inventory" "$release_root/docs/delivery/third-party-notices.md" <<'PY'
import hashlib, json, pathlib, sys

lock_path, output_path, trusted_base, candidate, candidate_tree, candidate_commit_count, inventory_path, notice_path = sys.argv[1:]
lock = json.loads(pathlib.Path(lock_path).read_text(encoding="utf-8"))
root = pathlib.Path(output_path)
inventory = json.loads(pathlib.Path(inventory_path).read_text(encoding="utf-8"))
reachability = json.loads((root / "cargo-reachability.json").read_text(encoding="utf-8"))
accepted = json.loads((root / "accepted-unfixed.json").read_text(encoding="utf-8"))
reports = [
    ("published_base_history", trusted_base, "secrets-base.json"),
    ("candidate_range", f"{trusted_base}..{candidate}", "secrets-range.json"),
    ("release_tree", candidate_tree, "secrets-dir.json"),
]
document = {
    "schema_version": 1,
    "topology": {
        "trusted_base": trusted_base,
        "candidate": candidate,
        "parent": trusted_base,
        "candidate_tree": candidate_tree,
        "commit_count": int(candidate_commit_count),
    },
    "tools": [{"name": item["name"], "version": item["version"], "sha256": item["sha256"]} for item in lock["tools"]],
    "license_inventory": {
        "digest": inventory["inventory_digest"],
        "file_sha256": hashlib.sha256(pathlib.Path(inventory_path).read_bytes()).hexdigest(),
        "generator": inventory["generator"],
        "notice_sha256": hashlib.sha256(pathlib.Path(notice_path).read_bytes()).hexdigest(),
    },
    "cargo_reachability": {
        "evidence_sha256": hashlib.sha256((root / "cargo-reachability.json").read_bytes()).hexdigest(),
        "cargo_lock_sha256": reachability["cargo_lock_sha256"],
        "manifests_digest": reachability["manifests_digest"],
        "resolved_graph_sha256": reachability["resolved_graph_sha256"],
    },
    "accepted_unfixed": {
        "evidence_sha256": hashlib.sha256((root / "accepted-unfixed.json").read_bytes()).hexdigest(),
        "status": accepted["status"],
        "acceptance_ids": [item["id"] for item in accepted["acceptances"]],
    },
    "secret_scans": [],
}
for kind, scope, report_name in reports:
    report_path = root / report_name
    findings = json.loads(report_path.read_text(encoding="utf-8"))
    document["secret_scans"].append({
        "kind": kind,
        "scope": scope,
        "report": report_name,
        "sha256": hashlib.sha256(report_path.read_bytes()).hexdigest(),
        "finding_count": len(findings),
    })
(root / "release-provenance.json").write_text(json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n")
PY
then
  die 'cannot write release provenance'
fi

verify_evidence "$output" "$release_root" || exit $?
if ! python3 - "$output" <<'PY'
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
files = ["packages.syft.json", "sbom.cdx.json", "vulnerabilities.json", "cargo-resolved-tree.txt", "cargo-reachability.json", "cargo-async-std-chain.txt", "accepted-unfixed.json", "release-provenance.json", "secrets-base.json", "secrets-range.json", "secrets-dir.json"]
manifest = {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in files}
(root / "evidence-sha256.json").write_text(json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n")
PY
then
  die 'cannot write evidence digest manifest'
fi
printf 'supply-chain: PASS output=%s\n' "$output"
