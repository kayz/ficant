#!/usr/bin/env bash

set -euo pipefail

die() {
  printf 'reproducibility: %s\n' "$1" >&2
  exit 2
}

gate_run_native() {
  local class=$1
  local label=$2
  shift 2
  local rc
  set +e
  "$@"
  rc=$?
  set -e
  if [[ $rc -eq 0 ]]; then
    return 0
  fi
  if [[ $class == finding && $rc -eq 1 ]]; then
    printf 'reproducibility: finding: %s (native exit %s)\n' "$label" "$rc" >&2
    return 1
  fi
  printf 'reproducibility: tool/evidence error: %s (native exit %s)\n' "$label" "$rc" >&2
  return 2
}

verify_manifests() {
  [[ $# -eq 2 ]] || die '--verify-manifests requires two JSON manifests'
  python3 - "$1" "$2" <<'PY'
import json
import pathlib
import sys

try:
    documents = [json.loads(pathlib.Path(path).read_text(encoding="utf-8")) for path in sys.argv[1:]]
except Exception as exc:
    print(f"reproducibility: invalid manifest: {exc}", file=sys.stderr)
    raise SystemExit(2)
for document in documents:
    if not isinstance(document, dict) or not isinstance(document.get("artifacts"), dict) or not document["artifacts"]:
        print("reproducibility: manifest has no artifacts", file=sys.stderr)
        raise SystemExit(2)
if documents[0]["artifacts"] != documents[1]["artifacts"]:
    print("reproducibility: artifact hash mismatch", file=sys.stderr)
    raise SystemExit(1)
PY
}

python_build_manifest() {
  [[ $# -eq 3 ]] || die '--python-build-manifest requires python-root output-dir manifest'
  local rc
  set +e
  python3 - "$1" "$2" "$3" <<'PY'
import base64
import csv
import gzip
import hashlib
import io
import json
import pathlib
import re
import shutil
import sys
import tarfile
import tomllib
import zipfile

root = pathlib.Path(sys.argv[1]).resolve()
output = pathlib.Path(sys.argv[2]).resolve()
manifest_path = pathlib.Path(sys.argv[3]).resolve()
pyproject_path = root / "pyproject.toml"
lock_path = root / "uv.lock"
if not pyproject_path.is_file() or not lock_path.is_file():
    print("reproducibility: Python pyproject.toml/uv.lock missing", file=sys.stderr)
    raise SystemExit(2)
try:
    project = tomllib.loads(pyproject_path.read_text(encoding="utf-8"))["project"]
    name = project["name"]
    version = project["version"]
except Exception as exc:
    print(f"reproducibility: invalid Python project metadata: {exc}", file=sys.stderr)
    raise SystemExit(2)
if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", name) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]*", version):
    print("reproducibility: invalid Python name/version", file=sys.stderr)
    raise SystemExit(2)

distribution = re.sub(r"[-_.]+", "_", name)
dist_info = f"{distribution}-{version}.dist-info"
sdist_root = f"{name.replace('_', '-')}-{version}"
if output.exists():
    shutil.rmtree(output)
output.mkdir(parents=True)

excluded_parts = {".git", ".pytest_cache", ".venv", "__pycache__", "dist", "build"}
source_files = sorted(
    path for path in root.rglob("*")
    if path.is_file() and not any(part in excluded_parts for part in path.relative_to(root).parts)
    and (path.suffix == ".py" or path.name in {"pyproject.toml", "uv.lock", "Dockerfile"})
)
if not any(path.suffix == ".py" for path in source_files):
    print("reproducibility: Python source is empty", file=sys.stderr)
    raise SystemExit(2)

metadata_lines = [
    "Metadata-Version: 2.3",
    f"Name: {name}",
    f"Version: {version}",
    f"Requires-Python: {project.get('requires-python', '')}",
]
for dependency in project.get("dependencies", []):
    metadata_lines.append(f"Requires-Dist: {dependency}")
metadata = ("\n".join(metadata_lines) + "\n").encode()

wheel_entries = {}
contracts_root = root / "node-contracts" / "src"
if contracts_root.is_dir():
    for path in sorted(contracts_root.rglob("*.py")):
        wheel_entries[path.relative_to(contracts_root).as_posix()] = path.read_bytes()
if not wheel_entries:
    for path in source_files:
        if path.suffix == ".py":
            wheel_entries[f"{distribution}/{path.relative_to(root).as_posix().replace('/', '_')}"] = path.read_bytes()
wheel_entries[f"{dist_info}/METADATA"] = metadata
wheel_entries[f"{dist_info}/WHEEL"] = b"Wheel-Version: 1.0\nGenerator: ficant-reproducibility-gate\nRoot-Is-Purelib: true\nTag: py3-none-any\n"

record_rows = []
for path, data in sorted(wheel_entries.items()):
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    record_rows.append((path, f"sha256={digest}", str(len(data))))
record_path = f"{dist_info}/RECORD"
record_buffer = io.StringIO(newline="")
writer = csv.writer(record_buffer, lineterminator="\n")
writer.writerows(record_rows + [(record_path, "", "")])
wheel_entries[record_path] = record_buffer.getvalue().encode()

wheel_path = output / f"{distribution}-{version}-py3-none-any.whl"
with zipfile.ZipFile(wheel_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
    for path, data in sorted(wheel_entries.items()):
        info = zipfile.ZipInfo(path, date_time=(1980, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = 0o100644 << 16
        archive.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)

sdist_path = output / f"{name.replace('_', '-')}-{version}.tar.gz"
with sdist_path.open("wb") as raw:
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0, compresslevel=9) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
            entries = [(path.relative_to(root).as_posix(), path.read_bytes()) for path in source_files]
            entries.append(("PKG-INFO", metadata))
            for relative, data in sorted(entries):
                info = tarfile.TarInfo(f"{sdist_root}/{relative}")
                info.size = len(data); info.mtime = 0; info.mode = 0o644
                info.uid = 0; info.gid = 0; info.uname = ""; info.gname = ""
                archive.addfile(info, io.BytesIO(data))

def normalized_zip(path):
    h = hashlib.sha256()
    with zipfile.ZipFile(path) as archive:
        names = sorted(name for name in archive.namelist() if not name.endswith("/"))
        for name in names:
            data = archive.read(name); encoded = name.encode()
            h.update(len(encoded).to_bytes(8, "big")); h.update(encoded)
            h.update(len(data).to_bytes(8, "big")); h.update(data)
    return h.hexdigest()

def normalized_tar(path):
    h = hashlib.sha256()
    with tarfile.open(path, "r:gz") as archive:
        members = sorted((member for member in archive.getmembers() if member.isfile()), key=lambda item: item.name)
        for member in members:
            data = archive.extractfile(member).read(); encoded = member.name.encode()
            h.update(len(encoded).to_bytes(8, "big")); h.update(encoded)
            h.update(len(data).to_bytes(8, "big")); h.update(data)
    return h.hexdigest()

artifacts = {
    "python-sdist": normalized_tar(sdist_path),
    "python-wheel": normalized_zip(wheel_path),
}
manifest_path.parent.mkdir(parents=True, exist_ok=True)
manifest_path.write_text(json.dumps({"artifacts": artifacts}, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
PY
  rc=$?
  set -e
  if [[ $rc -ne 0 ]]; then
    printf 'reproducibility: tool/evidence error: Python wheel/sdist builder (native exit %s)\n' "$rc" >&2
    return 2
  fi
}

if [[ ${1:-} == '--verify-manifests' ]]; then
  shift
  verify_manifests "$@"
  exit $?
fi
if [[ ${1:-} == '--python-build-manifest' ]]; then
  shift
  python_build_manifest "$@"
  exit $?
fi
if [[ ${1:-} == '--map-native' ]]; then
  shift
  [[ $# -ge 2 && ($1 == finding || $1 == tool) ]] || die '--map-native requires finding|tool command [args...]'
  class=$1
  shift
  gate_run_native "$class" 'diagnostic command' "$@"
  exit $?
fi
[[ $# -eq 0 ]] || die 'unexpected arguments'

repo=$(git rev-parse --show-toplevel 2>/dev/null) || die 'not in a Git worktree'
cd "$repo" || die 'cannot enter repository root'
status=$(git status --porcelain) || die 'cannot inspect worktree status'
[[ -z $status ]] || die 'worktree must be clean'
for command in git tar python3 sha256sum mktemp mkdir rm cat sort cargo uv clang++-18 cmake ninja corepack; do
  command -v "$command" >/dev/null || die "missing tool: $command"
done
[[ $(cargo --version) == cargo\ 1.96.1* ]] || die 'Cargo must be 1.96.1'
[[ $(uv --version) == 'uv 0.7.13' ]] || die 'uv must be 0.7.13'
[[ $(corepack pnpm@10.12.4 --version) == '10.12.4' ]] || die 'pnpm must be 10.12.4'

tmp=$(mktemp -d) || die 'cannot create temporary directory'
trap 'rm -rf "$tmp"' EXIT
create_directory() { mkdir -p "$1"; }
materialize_copy() { git archive HEAD | tar -x -C "$1"; }
for copy in a b; do
  gate_run_native tool "create temporary tree $copy" create_directory "$tmp/$copy" || exit $?
  gate_run_native tool "materialize tracked tree $copy" materialize_copy "$tmp/$copy" || exit $?
done

build_copy() {
  local root=$1
  local manifest=$2
  local tag=$3
  export CARGO_TARGET_DIR="$root/build/rust"
  rust_build() { (cd "$root" && cargo build --workspace --all-targets --locked --release); }
  python_environment() { (cd "$root/python" && uv sync --frozen --dev && uv pip freeze --python .venv/bin/python | LC_ALL=C sort >"$root/build/python-freeze.txt"); }
  cpp_configure() { cmake -S "$root/cpp/fixed-income-kernel" -B "$root/build/cpp" -G Ninja -DCMAKE_CXX_COMPILER=clang++-18 -DCMAKE_BUILD_TYPE=Release; }
  cpp_build() { cmake --build "$root/build/cpp" --parallel; }
  web_build() { (cd "$root/web-dm" && corepack pnpm@10.12.4 install --frozen-lockfile --store-dir "$tmp/pnpm-store-$tag" && corepack pnpm@10.12.4 build); }
  gate_run_native tool "Rust build $tag" rust_build || return $?
  gate_run_native tool "Python locked environment $tag" python_environment || return $?
  gate_run_native tool "Python wheel/sdist build $tag" python_build_manifest "$root/python" "$root/build/python-dist" "$root/build/python-artifacts.json" || return $?
  gate_run_native tool "C++ configure $tag" cpp_configure || return $?
  gate_run_native tool "C++ build $tag" cpp_build || return $?
  gate_run_native tool "Web build $tag" web_build || return $?
  if ! python3 - "$root" "$manifest" <<'PY'
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
output = pathlib.Path(sys.argv[2])

def digest_files(paths):
    h = hashlib.sha256()
    resolved = []
    for pattern in paths:
        resolved.extend(root.glob(pattern))
    files = sorted({path for path in resolved if path.is_file()}, key=lambda p: p.relative_to(root).as_posix())
    if not files:
        raise SystemExit(f"missing reproducibility artifacts for {paths}")
    for path in files:
        rel = path.relative_to(root).as_posix().encode()
        data = path.read_bytes()
        h.update(len(rel).to_bytes(8, "big")); h.update(rel)
        h.update(len(data).to_bytes(8, "big")); h.update(data)
    return h.hexdigest()

artifacts = {
    "rust": digest_files(["build/rust/release/ficant-server", "build/rust/release/ficant-worker", "build/rust/release/ficant-web"]),
    "python": digest_files(["python/uv.lock", "build/python-freeze.txt", "build/python-artifacts.json"]),
    "cpp": digest_files(["build/cpp/libficant_kernel.so"]),
    "web": digest_files(["web-dm/platform-shell/dist/**/*"]),
}
output.write_text(json.dumps({"artifacts": artifacts}, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
PY
  then
    printf 'reproducibility: tool/evidence error: artifact manifest %s\n' "$tag" >&2
    return 2
  fi
}

build_copy "$tmp/a" "$tmp/build-a.json" a || exit $?
build_copy "$tmp/b" "$tmp/build-b.json" b || exit $?
verify_manifests "$tmp/build-a.json" "$tmp/build-b.json" || exit $?
cat "$tmp/build-a.json" || die 'cannot emit reproducibility manifest'
printf 'reproducibility: PASS\n' >&2
