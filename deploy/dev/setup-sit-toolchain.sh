#!/usr/bin/env bash
set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
  echo "run as root inside WSL ficant-ubuntu-24.04" >&2
  exit 1
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/../.." && pwd)"
lock_file="${repo_root}/deploy/dev/toolchain.lock.toml"
work_dir="$(mktemp -d /tmp/ficant-sit-toolchain.XXXXXX)"
trap 'rm -rf -- "${work_dir}"' EXIT

for command_name in curl python3 sha256sum tar install; do
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    echo "missing prerequisite: ${command_name}" >&2
    exit 1
  fi
done

eval "$(python3 - "${lock_file}" <<'PY'
import shlex
import sys
import tomllib

with open(sys.argv[1], "rb") as stream:
    lock = tomllib.load(stream)

values = {
    "RUST_VERSION": lock["rust"]["version"],
    "CMAKE_VERSION": lock["cmake"]["version"],
    "CMAKE_URL": lock["cmake"]["url"],
    "CMAKE_SHA256": lock["cmake"]["sha256"],
    "NINJA_VERSION": lock["ninja"]["version"],
    "NINJA_URL": lock["ninja"]["url"],
    "NINJA_SHA256": lock["ninja"]["sha256"],
}
for key, value in values.items():
    print(f"{key}={shlex.quote(str(value))}")
PY
)"

export RUSTUP_HOME=/opt/rustup
export CARGO_HOME=/opt/cargo
RUST_TOOLCHAIN_DIR="${RUSTUP_HOME}/toolchains/${RUST_VERSION}-x86_64-unknown-linux-gnu"
mkdir -p -- "${RUSTUP_HOME}" "${CARGO_HOME}"
if [[ ! -x "${RUST_TOOLCHAIN_DIR}/bin/rustc" || ! -x "${RUST_TOOLCHAIN_DIR}/bin/cargo" ]]; then
  rustup_url="https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init"
  curl --fail --location --retry 3 --connect-timeout 20 \
    "${rustup_url}" -o "${work_dir}/rustup-init"
  curl --fail --location --retry 3 --connect-timeout 20 \
    "${rustup_url}.sha256" -o "${work_dir}/rustup-init.sha256"
  rustup_sha256="$(awk 'NR == 1 { print $1 }' "${work_dir}/rustup-init.sha256")"
  printf '%s  %s\n' "${rustup_sha256}" "${work_dir}/rustup-init" | sha256sum --check --strict -
  chmod 0755 "${work_dir}/rustup-init"
  "${work_dir}/rustup-init" -y --no-modify-path --profile minimal --default-toolchain "${RUST_VERSION}"
fi
[[ -x "${RUST_TOOLCHAIN_DIR}/bin/rustc" && -x "${RUST_TOOLCHAIN_DIR}/bin/cargo" ]]
ln -sfn "${RUST_TOOLCHAIN_DIR}/bin/rustc" /usr/local/bin/rustc
ln -sfn "${RUST_TOOLCHAIN_DIR}/bin/cargo" /usr/local/bin/cargo
ln -sfn "${CARGO_HOME}/bin/rustup" /usr/local/bin/rustup
export PATH="/usr/local/bin:${CARGO_HOME}/bin:${PATH}"

bash "${repo_root}/.github/scripts/setup-llvm-toolchain.sh" --install
ln -sfn /usr/bin/clang-18 /usr/local/bin/clang
ln -sfn /usr/bin/clang++-18 /usr/local/bin/clang++

if [[ -e "/opt/cmake-${CMAKE_VERSION}" ]]; then
  if [[ "$(/opt/cmake-${CMAKE_VERSION}/bin/cmake --version | awk 'NR == 1 { print $3 }')" != "${CMAKE_VERSION}" ]]; then
    echo "/opt/cmake-${CMAKE_VERSION} exists but does not match the lock; inspect it manually" >&2
    exit 1
  fi
else
  curl --fail --location --retry 3 --connect-timeout 20 \
    "${CMAKE_URL}" -o "${work_dir}/cmake.tar.gz"
  printf '%s  %s\n' "${CMAKE_SHA256}" "${work_dir}/cmake.tar.gz" | sha256sum --check --strict -
  mkdir "${work_dir}/cmake"
  tar -xzf "${work_dir}/cmake.tar.gz" --strip-components=1 -C "${work_dir}/cmake"
  mv "${work_dir}/cmake" "/opt/cmake-${CMAKE_VERSION}"
fi
for cmake_tool in cmake ctest cpack; do
  ln -sfn "/opt/cmake-${CMAKE_VERSION}/bin/${cmake_tool}" "/usr/local/bin/${cmake_tool}"
done

curl --fail --location --retry 3 --connect-timeout 20 \
  "${NINJA_URL}" -o "${work_dir}/ninja.zip"
printf '%s  %s\n' "${NINJA_SHA256}" "${work_dir}/ninja.zip" | sha256sum --check --strict -
python3 - "${work_dir}/ninja.zip" "${work_dir}/ninja" <<'PY'
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1]) as archive:
    payload = archive.read("ninja")
with open(sys.argv[2], "wb") as stream:
    stream.write(payload)
PY
install -m 0755 "${work_dir}/ninja" /usr/local/bin/ninja

rustc --version
cargo --version
clang --version | head -n 1
cmake --version | head -n 1
ninja --version

[[ "$(rustc --version | awk '{ print $2 }')" == "${RUST_VERSION}" ]]
clang --version | head -n 1 | grep -Eq '(^| )18\.1\.8( |$)'
[[ "$(cmake --version | awk 'NR == 1 { print $3 }')" == "${CMAKE_VERSION}" ]]
[[ "$(ninja --version)" == "${NINJA_VERSION}" ]]

echo "ficant SIT toolchain matches deploy/dev/toolchain.lock.toml"
