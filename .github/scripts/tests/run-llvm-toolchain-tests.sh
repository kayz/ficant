#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
tool="$repo/.github/scripts/setup-llvm-toolchain.sh"
lock="$repo/deploy/dev/toolchain.lock.toml"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

expect_exit() {
  local expected=$1
  shift
  set +e
  "$@" >"$tmp/stdout" 2>"$tmp/stderr"
  actual=$?
  set -e
  if [[ $actual -ne $expected ]]; then
    printf 'llvm fixture expected exit %s, got %s: %s\n' "$expected" "$actual" "$*" >&2
    cat "$tmp/stderr" >&2 || true
    exit 1
  fi
}

cat >"$tmp/os-release-ubuntu24" <<'OS_RELEASE'
NAME="Ubuntu"
ID=ubuntu
VERSION_ID="24.04"
VERSION_CODENAME=noble
OS_RELEASE
cat >"$tmp/os-release-ubuntu26" <<'OS_RELEASE'
ID=ubuntu
VERSION_ID="26.04"
VERSION_CODENAME=questing
OS_RELEASE
cat >"$tmp/os-release-nonubuntu" <<'OS_RELEASE'
ID=debian
VERSION_ID="12"
VERSION_CODENAME=bookworm
OS_RELEASE
printf 'ID=ubuntu\nVERSION_ID="24.04\nVERSION_CODENAME=noble\n' >"$tmp/os-release-malformed"
expect_exit 0 bash "$tool" --verify-host-fixture "$tmp/os-release-ubuntu24"
expect_exit 2 bash "$tool" --verify-host-fixture "$tmp/os-release-ubuntu26"
expect_exit 2 bash "$tool" --verify-host-fixture "$tmp/os-release-nonubuntu"
expect_exit 2 bash "$tool" --verify-host-fixture "$tmp/os-release-missing"
expect_exit 2 bash "$tool" --verify-host-fixture "$tmp/os-release-malformed"
python3 - "$tool" <<'PY'
import pathlib
import sys

text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
install = text.index("for command in curl")
production = text[install:]
marker = "verify_host /etc/os-release"
if production.count(marker) != 1:
    raise SystemExit("production install must bind exactly once to /etc/os-release")
position = production.index(marker)
for mutation in ("curl --retry", "dpkg --no-act --remove", "sudo dpkg --remove", "sudo dpkg --install"):
    if position >= production.index(mutation):
        raise SystemExit(f"host verification must precede {mutation}")
if "${" in production[position:position + len(marker)]:
    raise SystemExit("production os-release path must not be overridable")
PY

python3 - "$lock" "$tmp/index.txt" <<'PY'
import pathlib
import sys
import tomllib

clang = tomllib.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))["clang"]
dependencies = {
    "clang-18": "libc6 (>= 2.34), libclang-cpp18 (>= 1:18.1.8~++20240731025043+3b5b5c1ec4a3), libllvm18 (= 1:18.1.8~++20240731025043+3b5b5c1ec4a3-1~exp1~20240731145144.92), libstdc++6 (>= 11), libstdc++-13-dev, libgcc-13-dev, libobjc-13-dev, libclang-common-18-dev (= 1:18.1.8~++20240731025043+3b5b5c1ec4a3-1~exp1~20240731145144.92), llvm-18-linker-tools (= 1:18.1.8~++20240731025043+3b5b5c1ec4a3-1~exp1~20240731145144.92), libclang1-18 (= 1:18.1.8~++20240731025043+3b5b5c1ec4a3-1~exp1~20240731145144.92), libc6-dev, binutils",
    "libclang-common-18-dev": "",
    "libclang-cpp18": "libc6 (>= 2.38), libllvm18 (= 1:18.1.8~++20240731025043+3b5b5c1ec4a3-1~exp1~20240731145144.92), libstdc++6 (>= 12)",
    "libclang1-18": "libc6 (>= 2.38), libllvm18, libstdc++6 (>= 11)",
    "libllvm18": "libc6 (>= 2.38), libedit2 (>= 2.11-20080614-0), libffi8 (>= 3.4), libgcc-s1 (>= 3.3), libstdc++6 (>= 12), libtinfo6 (>= 6), libxml2 (>= 2.7.4), libzstd1 (>= 1.5.5), zlib1g (>= 1:1.2.0)",
    "llvm-18-linker-tools": "libc6 (>= 2.38), libllvm18, libstdc++6 (>= 11)",
}
paragraphs = []
for item in clang["packages"]:
    fields = {
        "Package": item["name"], "Source": clang["source"], "Version": clang["package_version"],
        "Architecture": clang["architecture"], "Filename": item["filename"],
        "Size": str(item["size"]), "SHA256": item["sha256"],
    }
    if dependencies[item["name"]]:
        fields["Depends"] = dependencies[item["name"]]
    if item["name"] == "libllvm18":
        fields["Breaks"] = "llvm-18-dev (<< 1:18.1.8~++20240730104741)"
    paragraphs.append("\n".join(f"{key}: {value}" for key, value in fields.items()))
pathlib.Path(sys.argv[2]).write_text("\n\n".join(paragraphs) + "\n", encoding="utf-8")
PY
python3 - "$tmp/index.txt" "$tmp/Packages.gz" <<'PY'
import gzip, pathlib, sys
pathlib.Path(sys.argv[2]).write_bytes(gzip.compress(pathlib.Path(sys.argv[1]).read_bytes(), mtime=0))
PY
index_size=$(wc -c <"$tmp/Packages.gz")
index_hash=$(sha256sum "$tmp/Packages.gz" | awk '{print $1}')
expect_exit 0 bash "$tool" --verify-index-fixture "$tmp/Packages.gz" "$index_size" "$index_hash" "$tmp/manifest.json"
expect_exit 2 bash "$tool" --verify-index-fixture "$tmp/Packages.gz" "$index_size" "$(printf '0%.0s' {1..64})" "$tmp/hash.json"

mutate_index() {
  local mode=$1 output="$tmp/$1.gz"
  python3 - "$tmp/index.txt" "$output" "$mode" <<'PY'
import gzip, pathlib, sys
text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
mode = sys.argv[3]
paragraphs = text.strip().split("\n\n")
if mode == "duplicate": paragraphs.append(paragraphs[0])
elif mode == "missing": paragraphs = [p for p in paragraphs if not p.startswith("Package: libclang1-18\n")]
elif mode == "version": paragraphs[0] = paragraphs[0].replace("Version: 1:18.1.8", "Version: 1:18.1.7", 1)
elif mode == "architecture": paragraphs[0] = paragraphs[0].replace("Architecture: amd64", "Architecture: arm64", 1)
elif mode == "source": paragraphs[0] = paragraphs[0].replace("Source: llvm-toolchain-18", "Source: other", 1)
elif mode == "path": paragraphs[0] = paragraphs[0].replace("Filename: pool/main/l/llvm-toolchain-18/", "Filename: ../", 1)
elif mode == "artifact-hash": paragraphs[0] = paragraphs[0].replace("SHA256: ", "SHA256: 0", 1)
elif mode == "allowlist": paragraphs[0] = paragraphs[0].replace("Depends: ", "Depends: evil-dependency, ", 1)
elif mode == "alternatives": paragraphs[0] = paragraphs[0].replace("binutils", "binutils | evil-dependency", 1)
elif mode == "conflict": paragraphs[0] += "\nConflicts: evil-installed"
else: raise SystemExit(mode)
pathlib.Path(sys.argv[2]).write_bytes(gzip.compress(("\n\n".join(paragraphs) + "\n").encode(), mtime=0))
PY
  local size hash
  size=$(wc -c <"$output")
  hash=$(sha256sum "$output" | awk '{print $1}')
  if [[ $mode == conflict ]]; then
    expect_exit 0 bash "$tool" --verify-index-fixture "$output" "$size" "$hash" "$tmp/conflict.json"
  else
    expect_exit 2 bash "$tool" --verify-index-fixture "$output" "$size" "$hash" "$tmp/$mode.json"
  fi
}
for mode in duplicate missing version architecture source path artifact-hash allowlist alternatives conflict; do mutate_index "$mode"; done

python3 - "$tmp/manifest.json" "$tmp/system.tsv" <<'PY'
import json, pathlib, sys
manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
names = sorted({item["name"] for item in manifest["system_requirements"]})
pathlib.Path(sys.argv[2]).write_text("".join(f"{name}\t999:999\n" for name in names))
PY
expect_exit 0 bash "$tool" --verify-system-fixture "$tmp/manifest.json" "$tmp/system.tsv"
tail -n +2 "$tmp/system.tsv" >"$tmp/system-missing.tsv"
expect_exit 2 bash "$tool" --verify-system-fixture "$tmp/manifest.json" "$tmp/system-missing.tsv"
cp "$tmp/system.tsv" "$tmp/system-breaks.tsv"
printf 'llvm-18-dev\t1:18.0\n' >>"$tmp/system-breaks.tsv"
expect_exit 2 bash "$tool" --verify-system-fixture "$tmp/manifest.json" "$tmp/system-breaks.tsv"
cp "$tmp/system.tsv" "$tmp/system-conflict.tsv"
printf 'evil-installed\t1\n' >>"$tmp/system-conflict.tsv"
expect_exit 2 bash "$tool" --verify-system-fixture "$tmp/conflict.json" "$tmp/system-conflict.tsv"

cat >"$tmp/runner-status" <<'STATUS'
Package: llvm-18-dev
Version: 1:18.1.3-1ubuntu1
Architecture: amd64
Status: install ok installed
Depends:
Pre-Depends:

Package: unrelated-runtime
Version: 1
Architecture: amd64
Status: install ok installed
Depends: libc6
Pre-Depends:
STATUS
: >"$tmp/dpkg-audit"
expect_exit 0 bash "$tool" --verify-preinstalled-conflict-fixture "$tmp/manifest.json" "$tmp/runner-status" "$tmp/dpkg-audit" "$tmp/remove-plan"
[[ $(cat "$tmp/remove-plan") == llvm-18-dev ]]
sed 's/1:18.1.3-1ubuntu1/1:18.1.3-1/' "$tmp/runner-status" >"$tmp/runner-version-drift"
expect_exit 2 bash "$tool" --verify-preinstalled-conflict-fixture "$tmp/manifest.json" "$tmp/runner-version-drift" "$tmp/dpkg-audit" "$tmp/remove-version"
sed 's/Architecture: amd64/Architecture: arm64/' "$tmp/runner-status" >"$tmp/runner-arch-drift"
expect_exit 2 bash "$tool" --verify-preinstalled-conflict-fixture "$tmp/manifest.json" "$tmp/runner-arch-drift" "$tmp/dpkg-audit" "$tmp/remove-arch"
python3 - "$tmp/runner-status" "$tmp/runner-reverse-dependency" <<'PY'
import pathlib, sys
text = pathlib.Path(sys.argv[1]).read_text()
text = text.replace("Depends: libc6", "Depends: llvm-18-dev:any (>= 1:18.1.3-1ubuntu1)")
pathlib.Path(sys.argv[2]).write_text(text)
PY
expect_exit 2 bash "$tool" --verify-preinstalled-conflict-fixture "$tmp/manifest.json" "$tmp/runner-reverse-dependency" "$tmp/dpkg-audit" "$tmp/remove-reverse"
printf 'packages are not fully configured\n' >"$tmp/dpkg-audit-dirty"
expect_exit 2 bash "$tool" --verify-preinstalled-conflict-fixture "$tmp/manifest.json" "$tmp/runner-status" "$tmp/dpkg-audit-dirty" "$tmp/remove-audit"
expect_exit 2 bash "$tool" --verify-preinstalled-conflict-fixture "$tmp/conflict.json" "$tmp/runner-status" "$tmp/dpkg-audit" "$tmp/remove-unknown-conflict"

mkdir "$tmp/debs"
python3 - "$tmp/manifest.json" "$tmp/debs" <<'PY'
import json, pathlib, subprocess, sys
manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
root = pathlib.Path(sys.argv[2])
for item in manifest["packages"]:
    package_root = root / (item["name"] + "-root")
    control = package_root / "DEBIAN" / "control"
    control.parent.mkdir(parents=True)
    control.write_text(f"Package: {item['name']}\nVersion: {item['version']}\nArchitecture: {item['architecture']}\nSource: {item['source']}\nMaintainer: fixture <fixture@example.invalid>\nDescription: fixture\n")
    output = root / pathlib.PurePosixPath(item["filename"]).name
    subprocess.run(["dpkg-deb", "--build", package_root, output], check=True, stdout=subprocess.DEVNULL)
PY
python3 - "$tmp/manifest.json" "$tmp/debs" "$tmp/deb-manifest.json" <<'PY'
import hashlib, json, pathlib, sys
manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
root = pathlib.Path(sys.argv[2])
for item in manifest["packages"]:
    path = root / pathlib.PurePosixPath(item["filename"]).name
    item["size"] = path.stat().st_size
    item["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
pathlib.Path(sys.argv[3]).write_text(json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n")
PY
expect_exit 0 bash "$tool" --verify-debs-fixture "$tmp/deb-manifest.json" "$tmp/debs"
python3 - "$tmp/deb-manifest.json" "$tmp/debs" <<'PY'
import hashlib, json, pathlib, subprocess, sys
manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
item = manifest["packages"][0]
root = pathlib.Path(sys.argv[2])
package_root = root / "wrong-root"
control = package_root / "DEBIAN" / "control"
control.parent.mkdir(parents=True)
control.write_text(f"Package: {item['name']}\nVersion: 0\nArchitecture: {item['architecture']}\nSource: {item['source']}\nMaintainer: fixture <fixture@example.invalid>\nDescription: fixture\n")
output = root / pathlib.PurePosixPath(item["filename"]).name
subprocess.run(["dpkg-deb", "--build", package_root, output], check=True, stdout=subprocess.DEVNULL)
item["size"] = output.stat().st_size
item["sha256"] = hashlib.sha256(output.read_bytes()).hexdigest()
pathlib.Path(sys.argv[1]).write_text(json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n")
PY
expect_exit 2 bash "$tool" --verify-debs-fixture "$tmp/deb-manifest.json" "$tmp/debs"

if grep -Eq '(^|[;&|[:space:]])(apt|apt-get)([;&|[:space:]]|$)|keyserver|apt-f|curl[^\n]*\|[^\n]*bash' "$tool"; then
  echo 'llvm fixture: forbidden dynamic resolver/install path' >&2
  exit 1
fi
[[ $(grep -Fc 'sudo dpkg --install' "$tool") -eq 1 ]]
[[ $(grep -Fc 'dpkg --no-act --install' "$tool") -eq 1 ]]
[[ $(grep -Fc 'sudo dpkg --remove llvm-18-dev' "$tool") -eq 1 ]]
[[ $(grep -Fc 'dpkg --no-act --remove llvm-18-dev' "$tool") -eq 1 ]]
[[ $(grep -Fc 'bash .github/scripts/setup-llvm-toolchain.sh --install' "$repo/.github/workflows/ci.yml") -eq 2 ]]
printf 'llvm toolchain fixture tests: PASS\n'
