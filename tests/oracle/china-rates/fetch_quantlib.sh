#!/usr/bin/env bash
# Human-operator-only official QuantLib 1.42.1 download/build/execute workflow.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
VERSION="1.42.1"
URL="https://github.com/lballabio/QuantLib/releases/download/v${VERSION}/QuantLib-${VERSION}.tar.gz"
DEFAULT_CACHE_HOME="${XDG_CACHE_HOME:-${HOME:?HOME must be set}/.cache}"
WORK_REQUESTED="${FICANT_QUANTLIB_WORKDIR:-$DEFAULT_CACHE_HOME/ficant/iteration-3/quantlib-${VERSION}}"

resolve_path() {
  resolved="$(python - "$1" <<'PY'
import sys
from pathlib import Path
print(Path(sys.argv[1]).expanduser().resolve().as_posix())
PY
)"
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -u "$resolved"
  else
    printf '%s\n' "$resolved"
  fi
}

WORK="$(resolve_path "$WORK_REQUESTED")"
REPO_ROOT="$(resolve_path "$REPO_ROOT")"
case "$WORK" in
  "$REPO_ROOT"|"$REPO_ROOT"/*)
    echo "REFUSING_REPO_LOCAL_WORKDIR: resolved workdir is inside repository: $WORK" >&2
    exit 2
    ;;
esac

ARCHIVE="$WORK/QuantLib-${VERSION}.tar.gz"
SOURCE="$WORK/QuantLib-${VERSION}"
BUILD="$WORK/build"
INSTALL="$WORK/install"
PROGRAM="$WORK/quantlib_oracle"
OUTPUT="$WORK/quantlib-output.json"
MANIFEST="$WORK/build-manifest.json"
COMPILE_IDENTITY="$WORK/compile-command.json"
HOST_ENVIRONMENT="$WORK/host-environment.json"
AGGREGATE_MANIFEST="$WORK/toolchain-build-environment.json"
EVIDENCE="$WORK/quantlib-evidence.json"
ORACLE_SOURCE="$HERE/quantlib_oracle.cpp"
ORACLE_INPUT="$REPO_ROOT/tests/golden-cases/china-rates/expected/cgb-reference-v1-expected.json"
TOOLCHAIN_LOCK="$REPO_ROOT/deploy/dev/toolchain.lock.toml"
SENTINEL="$WORK/.ficant-quantlib-workdir"

if [ -e "$WORK" ] && [ ! -d "$WORK" ]; then
  echo "REFUSING_UNSAFE_WORKDIR: workdir exists and is not a directory: $WORK" >&2
  exit 3
fi
if [ -d "$WORK" ]; then
  if [ -f "$SENTINEL" ]; then
    grep -Fxq "ficant-quantlib-workdir:$VERSION" "$SENTINEL" || {
      echo "REFUSING_UNSAFE_WORKDIR: invalid workflow sentinel: $SENTINEL" >&2
      exit 3
    }
  elif [ -n "$(find "$WORK" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
    echo "REFUSING_UNSAFE_WORKDIR: nonempty override lacks workflow sentinel: $WORK" >&2
    exit 3
  fi
fi
mkdir -p "$WORK"
if [ ! -f "$SENTINEL" ]; then
  printf 'ficant-quantlib-workdir:%s\n' "$VERSION" > "$SENTINEL"
fi
curl --fail --location --proto '=https' --tlsv1.2 --output "$ARCHIVE" "$URL"
rm -rf "$SOURCE" "$BUILD" "$INSTALL"
tar -xzf "$ARCHIVE" -C "$WORK"

VERSION_HEADER="$SOURCE/ql/version.hpp"
grep -Fxq '#define QL_VERSION "1.42.1"' "$VERSION_HEADER"
CMAKE_PATH="$(resolve_path "$(command -v cmake)")"
"$CMAKE_PATH" -S "$SOURCE" -B "$BUILD" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$INSTALL" \
  -DQL_BUILD_BENCHMARK=OFF \
  -DQL_BUILD_EXAMPLES=OFF \
  -DQL_BUILD_TEST_SUITE=OFF
"$CMAKE_PATH" --build "$BUILD" --parallel
"$CMAKE_PATH" --install "$BUILD"

CXX="${CXX:-c++}"
CXX_PATH="$(resolve_path "$(command -v "$CXX")")"
LIBDIR="$INSTALL/lib"
if [ ! -d "$LIBDIR" ]; then LIBDIR="$INSTALL/lib64"; fi
INSTALLED_LIBRARY="$(find "$INSTALL" -type f \( -name 'libQuantLib.so*' -o -name 'libQuantLib.a' -o -name 'libQuantLib.dylib' \) -print | sort | head -n 1)"
if [ -z "$INSTALLED_LIBRARY" ]; then
  echo "installed QuantLib library not found below $INSTALL" >&2
  exit 4
fi
COMPILE_COMMAND=(
  "$CXX_PATH" -std=c++20 -O2 "$ORACLE_SOURCE"
  -I"$INSTALL/include" "$INSTALLED_LIBRARY"
  -Wl,-rpath,"$LIBDIR" -o "$PROGRAM"
)
python - "$COMPILE_IDENTITY" "${COMPILE_COMMAND[@]}" <<'PY'
import json
import sys
from pathlib import Path
Path(sys.argv[1]).write_text(json.dumps(sys.argv[2:]) + "\n", encoding="utf-8")
PY
"${COMPILE_COMMAND[@]}"
"$PROGRAM" > "$OUTPUT"

export VERSION URL ARCHIVE VERSION_HEADER BUILD INSTALL PROGRAM OUTPUT MANIFEST
export COMPILE_IDENTITY HOST_ENVIRONMENT AGGREGATE_MANIFEST
export ORACLE_SOURCE ORACLE_INPUT TOOLCHAIN_LOCK INSTALLED_LIBRARY CXX
export OS_IDENTITY="$(uname -srm)"
export ARCHITECTURE="$(uname -m)"
export BUILD_MODE="host-toolchain"
export CONTAINER_IMAGE_DIGEST=""
export CMAKE_PATH
export CMAKE_VERSION="$("$CMAKE_PATH" --version | head -n 1)"
export CMAKE_GENERATOR="$(sed -n 's/^CMAKE_GENERATOR:INTERNAL=//p' "$BUILD/CMakeCache.txt" | head -n 1)"
export CXX_PATH
export CXX_VERSION="$("$CXX_PATH" --version | head -n 1)"
export NINJA_PATH=""
export NINJA_VERSION=""
if [[ "$CMAKE_GENERATOR" == Ninja* ]]; then
  NINJA_CACHE_PATH="$(sed -n 's/^CMAKE_MAKE_PROGRAM:[^=]*=//p' "$BUILD/CMakeCache.txt" | head -n 1)"
  NINJA_PATH="$(resolve_path "$NINJA_CACHE_PATH")"
  NINJA_VERSION="$("$NINJA_PATH" --version)"
  export NINJA_PATH NINJA_VERSION
fi

python - <<'PY'
import hashlib
import json
import os
from pathlib import Path

def resolved(name):
    return Path(os.environ[name]).resolve()

def artifact(name):
    path = resolved(name)
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    return {"path": str(path), "sha256": digest}

def executable(path_name, version_name):
    return {**artifact(path_name), "version": os.environ[version_name]}

def canonical_write(path, payload):
    path.write_bytes(
        (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
    )

compile_command = json.loads(resolved("COMPILE_IDENTITY").read_text(encoding="utf-8"))
os_release_path = Path("/etc/os-release")
host_environment = {
    "schema": "ficant.test-oracle.host-environment.v1",
    "build_mode": os.environ["BUILD_MODE"],
    "uname": os.environ["OS_IDENTITY"],
    "architecture": os.environ["ARCHITECTURE"],
    "os_release": (
        os_release_path.read_text(encoding="utf-8", errors="replace")
        if os_release_path.is_file() else None
    ),
    "environment": {
        key: os.environ.get(key)
        for key in (
            "CC", "CXX", "CFLAGS", "CXXFLAGS", "LDFLAGS", "PATH",
            "CMAKE_GENERATOR", "MACOSX_DEPLOYMENT_TARGET",
        )
    },
}
canonical_write(resolved("HOST_ENVIRONMENT"), host_environment)

manifest = {
    "schema": "ficant.test-oracle.quantlib-build-manifest.v3",
    "quantlib_version": os.environ["VERSION"],
    "build_mode": os.environ["BUILD_MODE"],
    "container_image_digest": None,
    "source": {
        "url": os.environ["URL"],
        "archive": artifact("ARCHIVE"),
        "version_header": artifact("VERSION_HEADER"),
        "version_header_identity": '#define QL_VERSION "1.42.1"',
    },
    "toolchain": {
        "compiler": executable("CXX_PATH", "CXX_VERSION"),
        "cmake": executable("CMAKE_PATH", "CMAKE_VERSION"),
        "ninja": (
            executable("NINJA_PATH", "NINJA_VERSION")
            if os.environ["NINJA_PATH"] else None
        ),
    },
    "repository": {
        "toolchain_lock": artifact("TOOLCHAIN_LOCK"),
    },
    "environment": {
        "os": os.environ["OS_IDENTITY"],
        "architecture": os.environ["ARCHITECTURE"],
        "cmake_generator": os.environ["CMAKE_GENERATOR"],
        "os_environment_manifest": artifact("HOST_ENVIRONMENT"),
        "cmake_config": {
            "CMAKE_BUILD_TYPE": "Release",
            "QL_BUILD_BENCHMARK": "OFF",
            "QL_BUILD_EXAMPLES": "OFF",
            "QL_BUILD_TEST_SUITE": "OFF",
            "CMAKE_INSTALL_PREFIX": str(resolved("INSTALL")),
        },
    },
    "build": {
        "cmake_cache": {
            "path": str((resolved("BUILD") / "CMakeCache.txt").resolve()),
            "sha256": hashlib.sha256((resolved("BUILD") / "CMakeCache.txt").read_bytes()).hexdigest(),
        },
        "installed_library": artifact("INSTALLED_LIBRARY"),
    },
    "oracle": {
        "source": artifact("ORACLE_SOURCE"),
        "input": artifact("ORACLE_INPUT"),
        "compile_command": compile_command,
        "compile_identity": artifact("COMPILE_IDENTITY"),
        "program": artifact("PROGRAM"),
        "output": artifact("OUTPUT"),
    },
}
canonical_write(resolved("AGGREGATE_MANIFEST"), manifest)
manifest["aggregate_manifest"] = artifact("AGGREGATE_MANIFEST")
resolved("MANIFEST").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
PY

python "$HERE/verify_quantlib_output.py" \
  --source-archive "$ARCHIVE" \
  --build-manifest "$MANIFEST" \
  --oracle-program "$PROGRAM" \
  --output "$OUTPUT" \
  --evidence "$EVIDENCE"

printf 'QUANTLIB_EVIDENCE=%s\n' "$EVIDENCE"
printf 'FOLLOW_UP_ENV=export FICANT_QUANTLIB_EVIDENCE=%q\n' "$EVIDENCE"
printf 'FOLLOW_UP_VALIDATOR=python tests/oracle/china-rates/validator.py\n'
