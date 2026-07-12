#!/usr/bin/env bash

set -euo pipefail

scripts_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo=$(CDPATH= cd -- "$scripts_dir/../.." && pwd)
lock_file="$repo/deploy/dev/toolchain.lock.toml"
index_url='https://apt.llvm.org/noble/dists/llvm-toolchain-noble-18/main/binary-amd64/Packages.gz'
index_size=12613
index_sha256='8cf692ec3dd86f484d2db39877b35a7f8124bb60b7f66c03f78e030fe33d3919'

die() {
  printf 'llvm-toolchain: %s\n' "$1" >&2
  exit 2
}

verify_host() {
  [[ $# -eq 1 ]] || die 'verify_host requires os-release path'
  python3 - "$lock_file" "$1" <<'PY'
import pathlib
import re
import shlex
import sys
import tomllib

lock_name, os_release_name = sys.argv[1:]
clang = tomllib.loads(pathlib.Path(lock_name).read_text(encoding="utf-8"))["clang"]
if clang.get("preinstalled_conflict", {}).get("runner") != "ubuntu-24.04":
    raise SystemExit("LLVM runner lock identity drift")

path = pathlib.Path(os_release_name)
try:
    text = path.read_text(encoding="utf-8", errors="strict")
except (OSError, UnicodeError) as error:
    raise SystemExit(f"cannot read os-release: {error}") from error

fields = {}
for line_number, raw in enumerate(text.splitlines(), start=1):
    line = raw.strip()
    if not line or line.startswith("#"):
        continue
    if "=" not in line:
        raise SystemExit(f"malformed os-release line {line_number}")
    key, encoded = line.split("=", 1)
    if not re.fullmatch(r"[A-Z][A-Z0-9_]*", key) or key in fields:
        raise SystemExit(f"invalid/duplicate os-release key at line {line_number}")
    try:
        values = shlex.split(encoded, comments=False, posix=True)
    except ValueError as error:
        raise SystemExit(f"malformed os-release value at line {line_number}") from error
    if encoded == "":
        value = ""
    elif len(values) == 1:
        value = values[0]
    else:
        raise SystemExit(f"malformed os-release value at line {line_number}")
    fields[key] = value

expected = {"ID": "ubuntu", "VERSION_ID": "24.04", "VERSION_CODENAME": "noble"}
if any(fields.get(key) != value for key, value in expected.items()):
    raise SystemExit("host must be Ubuntu 24.04 noble")
PY
}

verify_index() {
  [[ $# -eq 4 ]] || die 'verify_index requires index, size, hash, and manifest'
  python3 - "$lock_file" "$1" "$2" "$3" "$4" <<'PY'
import gzip
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import tomllib

lock_name, index_name, expected_size_text, expected_hash, output_name = sys.argv[1:]
clang = tomllib.loads(pathlib.Path(lock_name).read_text(encoding="utf-8"))["clang"]
fixed = {
    "index_url": "https://apt.llvm.org/noble/dists/llvm-toolchain-noble-18/main/binary-amd64/Packages.gz",
    "index_size": 12613,
    "index_sha256": "8cf692ec3dd86f484d2db39877b35a7f8124bb60b7f66c03f78e030fe33d3919",
    "source": "llvm-toolchain-18",
    "architecture": "amd64",
    "package_version": "1:18.1.8~++20240731025043+3b5b5c1ec4a3-1~exp1~20240731145144.92",
    "pool_prefix": "pool/main/l/llvm-toolchain-18/",
}
if any(clang.get(key) != value for key, value in fixed.items()):
    raise SystemExit("LLVM lock identity drift")
expected_names = [
    "clang-18", "libclang-cpp18", "libllvm18", "libclang-common-18-dev",
    "llvm-18-linker-tools", "libclang1-18",
]
locked_packages = clang.get("packages", [])
if [item.get("name") for item in locked_packages] != expected_names:
    raise SystemExit("LLVM package set/order drift")

index = pathlib.Path(index_name)
data = index.read_bytes()
if len(data) != int(expected_size_text) or hashlib.sha256(data).hexdigest() != expected_hash:
    raise SystemExit("Packages.gz size/hash mismatch")
try:
    text = gzip.decompress(data).decode("utf-8", errors="strict")
except (OSError, UnicodeError) as error:
    raise SystemExit(f"invalid Packages.gz: {error}") from error

paragraphs = []
for raw in text.split("\n\n"):
    if not raw.strip():
        continue
    fields = {}
    current = None
    for line in raw.splitlines():
        if line.startswith((" ", "\t")):
            if current is None:
                raise SystemExit("invalid continuation in Packages.gz")
            fields[current] += "\n" + line
            continue
        if ": " not in line:
            raise SystemExit("invalid field in Packages.gz")
        key, value = line.split(": ", 1)
        if key in fields:
            raise SystemExit(f"duplicate field {key}")
        fields[key] = value
        current = key
    paragraphs.append(fields)

by_name = {}
for fields in paragraphs:
    name = fields.get("Package")
    if name in expected_names:
        by_name.setdefault(name, []).append(fields)
if set(by_name) != set(expected_names) or any(len(items) != 1 for items in by_name.values()):
    raise SystemExit("LLVM index package set is missing or duplicated")

locked_by_name = {item["name"]: item for item in locked_packages}
allowlist = set(clang.get("system_dependency_allowlist", []))
relation = re.compile(r"^([a-z0-9][a-z0-9+.-]*)(?: \((<<|<=|=|>=|>>) ([^)]+)\))?$")

def parse_relations(value, field, owner):
    if not value:
        return []
    if "|" in value:
        raise SystemExit(f"alternatives forbidden in {owner} {field}")
    result = []
    for raw_item in value.replace("\n ", " ").split(","):
        item = raw_item.strip()
        match = relation.fullmatch(item)
        if match is None:
            raise SystemExit(f"invalid dependency syntax in {owner} {field}: {item}")
        result.append({"name": match.group(1), "op": match.group(2), "version": match.group(3)})
    return result

def version_satisfies(actual, op, required):
    if op is None:
        return True
    return subprocess.run(["dpkg", "--compare-versions", actual, op, required], check=False).returncode == 0

packages = []
requirements = []
breaks = []
conflicts = []
edges = {name: set() for name in expected_names}
for name in expected_names:
    fields = by_name[name][0]
    locked = locked_by_name[name]
    for key, expected in (("Version", fixed["package_version"]), ("Architecture", fixed["architecture"]), ("Source", fixed["source"])):
        if fields.get(key) != expected:
            raise SystemExit(f"{name} {key} mismatch")
    filename = fields.get("Filename", "")
    path = pathlib.PurePosixPath(filename)
    if path.is_absolute() or ".." in path.parts or not filename.startswith(fixed["pool_prefix"]):
        raise SystemExit(f"{name} filename escapes frozen pool")
    if filename != locked.get("filename") or int(fields.get("Size", "-1")) != locked.get("size") or fields.get("SHA256") != locked.get("sha256"):
        raise SystemExit(f"{name} artifact metadata mismatch")
    dependencies = parse_relations(fields.get("Pre-Depends", ""), "Pre-Depends", name) + parse_relations(fields.get("Depends", ""), "Depends", name)
    for dependency in dependencies:
        dependency_name = dependency["name"]
        if dependency_name in edges:
            edges[name].add(dependency_name)
            if not version_satisfies(fixed["package_version"], dependency["op"], dependency["version"]):
                raise SystemExit(f"{name} closure version constraint is not satisfied")
        elif dependency_name in allowlist:
            requirements.append(dependency)
        else:
            raise SystemExit(f"{name} dependency outside allowlist: {dependency_name}")
    breaks.extend(parse_relations(fields.get("Breaks", ""), "Breaks", name))
    conflicts.extend(parse_relations(fields.get("Conflicts", ""), "Conflicts", name))
    packages.append({
        "name": name, "version": fields["Version"], "architecture": fields["Architecture"],
        "source": fields["Source"], "filename": filename, "size": int(fields["Size"]),
        "sha256": fields["SHA256"],
    })

reachable = {"clang-18"}
pending = ["clang-18"]
while pending:
    for dependency in edges[pending.pop()]:
        if dependency not in reachable:
            reachable.add(dependency)
            pending.append(dependency)
if reachable != set(expected_names):
    raise SystemExit("LLVM six-package closure is not fully reachable from clang-18")

manifest = {
    "schema_version": 1, "repository": clang["repository"], "packages": packages,
    "system_requirements": requirements, "breaks": breaks, "conflicts": conflicts,
}
pathlib.Path(output_name).write_text(json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
PY
}

verify_system() {
  [[ $# -eq 2 ]] || die 'verify_system requires manifest and status TSV|--live'
  python3 - "$1" "$2" <<'PY'
import json
import pathlib
import subprocess
import sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
status_source = sys.argv[2]
names = {item["name"] for key in ("system_requirements", "breaks", "conflicts") for item in manifest[key]}
installed = {}
if status_source == "--live":
    for name in sorted(names):
        result = subprocess.run(["dpkg-query", "-W", "-f=${Status}\t${Version}", name], text=True, capture_output=True, check=False)
        if result.returncode == 0 and result.stdout.startswith("install ok installed\t"):
            installed[name] = result.stdout.split("\t", 1)[1]
else:
    for line in pathlib.Path(status_source).read_text(encoding="utf-8").splitlines():
        name, version = line.split("\t", 1)
        if name in installed:
            raise SystemExit(f"duplicate installed package: {name}")
        installed[name] = version

def matches(version, item):
    if item["op"] is None:
        return True
    return subprocess.run(["dpkg", "--compare-versions", version, item["op"], item["version"]], check=False).returncode == 0

for requirement in manifest["system_requirements"]:
    version = installed.get(requirement["name"])
    if version is None or not matches(version, requirement):
        raise SystemExit(f"missing/incompatible system dependency: {requirement['name']}")
for field in ("breaks", "conflicts"):
    for item in manifest[field]:
        version = installed.get(item["name"])
        if version is not None and matches(version, item):
            raise SystemExit(f"installed package violates {field}: {item['name']}")
PY
}

plan_preinstalled_conflict() {
  [[ $# -eq 4 ]] || die 'plan_preinstalled_conflict requires manifest, dpkg status, audit, and output'
  python3 - "$lock_file" "$1" "$2" "$3" "$4" <<'PY'
import json
import pathlib
import re
import subprocess
import sys
import tomllib

lock_name, manifest_name, status_name, audit_name, output_name = sys.argv[1:]
clang = tomllib.loads(pathlib.Path(lock_name).read_text(encoding="utf-8"))["clang"]
allowed = clang.get("preinstalled_conflict")
expected = {
    "name": "llvm-18-dev",
    "version": "1:18.1.3-1ubuntu1",
    "architecture": "amd64",
    "runner": "ubuntu-24.04",
    "broken_by": "libllvm18",
    "operator": "<<",
    "threshold": "1:18.1.8~++20240730104741",
}
if allowed != expected:
    raise SystemExit("preinstalled conflict lock identity drift")
if pathlib.Path(audit_name).read_text(encoding="utf-8").strip():
    raise SystemExit("dpkg audit is not clean before conflict handling")

manifest = json.loads(pathlib.Path(manifest_name).read_text(encoding="utf-8"))
expected_break = {"name": allowed["name"], "op": allowed["operator"], "version": allowed["threshold"]}
if manifest.get("breaks") != [expected_break] or manifest.get("conflicts") != []:
    raise SystemExit("unknown Breaks/Conflicts cannot be removed")
if allowed["broken_by"] not in {item["name"] for item in manifest.get("packages", [])}:
    raise SystemExit("locked Breaks owner is absent from closure")

paragraphs = []
for raw in pathlib.Path(status_name).read_text(encoding="utf-8").split("\n\n"):
    if not raw.strip():
        continue
    fields = {}
    current = None
    for line in raw.splitlines():
        if line.startswith((" ", "\t")):
            if current is None:
                raise SystemExit("invalid dpkg status continuation")
            fields[current] = fields.get(current, "") + "\n" + line
            continue
        if ":" not in line:
            raise SystemExit("invalid dpkg status field")
        key, value = line.split(":", 1)
        if key in fields:
            raise SystemExit(f"duplicate dpkg status field: {key}")
        fields[key] = value.lstrip()
        current = key
    if fields.get("Status") == "install ok installed":
        paragraphs.append(fields)

by_name = {}
for fields in paragraphs:
    name = fields.get("Package")
    if not name or name in by_name:
        raise SystemExit("missing/duplicate installed package identity")
    by_name[name] = fields

target = by_name.get(allowed["name"])
if target is None:
    pathlib.Path(output_name).write_text("", encoding="utf-8")
    raise SystemExit(0)
if target.get("Version") != allowed["version"] or target.get("Architecture") != allowed["architecture"]:
    raise SystemExit("preinstalled conflict version/architecture is not allowlisted")
if target.get("Essential", "no").lower() == "yes":
    raise SystemExit("preinstalled conflict unexpectedly Essential")
if subprocess.run(["dpkg", "--compare-versions", target["Version"], allowed["operator"], allowed["threshold"]], check=False).returncode != 0:
    raise SystemExit("allowlisted preinstalled package does not violate frozen Breaks")

dependency_name = re.compile(
    r"(?:^|[|,]\s*)"
    + re.escape(allowed["name"])
    + r"(?::[A-Za-z0-9-]+)?(?:\s|\(|\[|<|$)"
)
for name, fields in by_name.items():
    if name == allowed["name"]:
        continue
    for key in ("Pre-Depends", "Depends"):
        if dependency_name.search(fields.get(key, "")):
            raise SystemExit(f"installed reverse dependency blocks removal: {name}")

output = pathlib.Path(output_name)
temporary = output.with_suffix(output.suffix + ".tmp")
temporary.write_text(allowed["name"] + "\n", encoding="utf-8")
temporary.replace(output)
PY
}

verify_debs() {
  [[ $# -eq 2 ]] || die 'verify_debs requires manifest and directory'
  python3 - "$1" "$2" <<'PY'
import hashlib
import json
import pathlib
import subprocess
import sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
root = pathlib.Path(sys.argv[2])
for item in manifest["packages"]:
    path = root / pathlib.PurePosixPath(item["filename"]).name
    if not path.is_file() or path.stat().st_size != item["size"] or hashlib.sha256(path.read_bytes()).hexdigest() != item["sha256"]:
        raise SystemExit(f"deb size/hash mismatch: {item['name']}")
    actual = {}
    for field in ("Package", "Version", "Architecture", "Source"):
        result = subprocess.run(["dpkg-deb", "-f", path, field], text=True, capture_output=True, check=True)
        actual[field] = result.stdout.strip()
    expected = {"Package": item["name"], "Version": item["version"], "Architecture": item["architecture"], "Source": item["source"]}
    if actual != expected:
        raise SystemExit(f"deb control metadata mismatch: {item['name']}")
PY
}

case ${1:-} in
  --verify-host-fixture)
    shift; verify_host "$@" || exit 2; exit 0
    ;;
  --verify-index-fixture)
    shift; verify_index "$@" || exit 2; exit 0
    ;;
  --verify-system-fixture)
    shift; verify_system "$@" || exit 2; exit 0
    ;;
  --verify-debs-fixture)
    shift; verify_debs "$@" || exit 2; exit 0
    ;;
  --verify-preinstalled-conflict-fixture)
    shift; plan_preinstalled_conflict "$@" || exit 2; exit 0
    ;;
  --install)
    [[ $# -eq 1 ]] || die 'unexpected arguments'
    ;;
  *)
    die 'usage: setup-llvm-toolchain.sh --install'
    ;;
esac

for command in curl python3 sha256sum dpkg dpkg-query dpkg-deb sudo; do
  command -v "$command" >/dev/null || die "missing tool: $command"
done
verify_host /etc/os-release || die 'unsupported LLVM installation host'
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl --retry 3 --retry-all-errors --fail --location --output "$tmp/Packages.gz" "$index_url"
verify_index "$tmp/Packages.gz" "$index_size" "$index_sha256" "$tmp/manifest.json" || die 'Packages.gz verification failed'
dpkg --audit >"$tmp/dpkg-audit-before"
dpkg-query -W -f='Package: ${Package}\nVersion: ${Version}\nArchitecture: ${Architecture}\nStatus: ${Status}\nEssential: ${Essential}\nDepends: ${Depends}\nPre-Depends: ${Pre-Depends}\n\n' >"$tmp/dpkg-status"
plan_preinstalled_conflict "$tmp/manifest.json" "$tmp/dpkg-status" "$tmp/dpkg-audit-before" "$tmp/remove-plan" || die 'preinstalled conflict is not safe to remove'
mapfile -t remove_plan <"$tmp/remove-plan"
if ((${#remove_plan[@]} > 0)); then
  [[ ${#remove_plan[@]} -eq 1 && ${remove_plan[0]} == llvm-18-dev ]] || die 'unexpected preinstalled removal plan'
  dpkg --no-act --remove llvm-18-dev >/dev/null || die 'preinstalled conflict removal dry-run failed'
  sudo dpkg --remove llvm-18-dev
  if [[ $(dpkg-query -W -f='${Status}' llvm-18-dev 2>/dev/null || true) == 'install ok installed' ]]; then
    die 'preinstalled conflict remains installed'
  fi
  [[ -z $(dpkg --audit) ]] || die 'dpkg audit failed after conflict removal'
fi
verify_system "$tmp/manifest.json" --live || die 'system dependency/conflict verification failed'

python3 - "$tmp/manifest.json" <<'PY' >"$tmp/downloads.tsv"
import json, pathlib, sys
manifest = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
for item in manifest["packages"]:
    print(manifest["repository"] + item["filename"], pathlib.PurePosixPath(item["filename"]).name, item["size"], item["sha256"], sep="\t")
PY
while IFS=$'\t' read -r url filename size sha256; do
  curl --retry 3 --retry-all-errors --fail --location --output "$tmp/$filename" "$url"
  [[ $(wc -c <"$tmp/$filename") -eq $size ]] || die "size mismatch: $filename"
  printf '%s  %s\n' "$sha256" "$tmp/$filename" | sha256sum -c - >/dev/null || die "hash mismatch: $filename"
done <"$tmp/downloads.tsv"
verify_debs "$tmp/manifest.json" "$tmp" || die 'deb metadata verification failed'
mapfile -t debs < <(python3 - "$tmp/manifest.json" "$tmp" <<'PY'
import json, pathlib, sys
manifest = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
root = pathlib.Path(sys.argv[2])
for item in manifest["packages"]:
    print(root / pathlib.PurePosixPath(item["filename"]).name)
PY
)
dpkg --no-act --install "${debs[@]}" >/dev/null || die 'dpkg dry-run failed'
sudo dpkg --install "${debs[@]}"
if [[ $(dpkg-query -W -f='${Status}' llvm-18-dev 2>/dev/null || true) == 'install ok installed' ]]; then
  die 'preinstalled conflict returned after LLVM closure installation'
fi
[[ -z $(dpkg --audit) ]] || die 'dpkg audit failed after LLVM closure installation'
clang++-18 --version | head -n 1
