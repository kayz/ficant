#!/usr/bin/env bash
# HOQA status: superseded historical WSL compatibility runner. It is not an ordinary
# development/test entry and may run only under an explicitly authorized compatibility gate.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/../.." && pwd)"
profiles_file="${script_dir}/profiles.toml"
capabilities_file="${script_dir}/environment-capabilities.toml"
contract_schema="${script_dir}/schemas/contract.schema.json"
result_schema="${script_dir}/schemas/result.schema.json"
toolchain_lock="${repo_root}/deploy/dev/toolchain.lock.toml"
cache_root="${XDG_CACHE_HOME:-${HOME}/.cache}/ficant"
build_root="${XDG_STATE_HOME:-${HOME}/.local/state}/ficant/build"
evidence_root="${FICANT_EVIDENCE_ROOT:-${XDG_STATE_HOME:-${HOME}/.local/state}/ficant/evidence}"
canary_repo="${cache_root}/preflight-canary/repo"
export FICANT_CACHE_ROOT="${cache_root}"
export FICANT_BUILD_ROOT="${build_root}"
export CARGO_HOME="${cache_root}/cargo-home"
export RUSTUP_HOME="/opt/rustup"

now_ms() {
  python3 -c 'import time; print(time.monotonic_ns() // 1000000)'
}

die() {
  printf 'execution-runner: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "missing required WSL command: $1"
}

validate_config() {
  require_command python3
  python3 - "${profiles_file}" "${capabilities_file}" "${contract_schema}" "${result_schema}" "${toolchain_lock}" <<'PY'
import json
import pathlib
import sys
import tomllib

profiles_path, capabilities_path, contract_path, result_path, lock_path = map(pathlib.Path, sys.argv[1:])
profiles = tomllib.loads(profiles_path.read_text(encoding="utf-8"))
capabilities = tomllib.loads(capabilities_path.read_text(encoding="utf-8"))
contract = json.loads(contract_path.read_text(encoding="utf-8"))
result = json.loads(result_path.read_text(encoding="utf-8"))
tomllib.loads(lock_path.read_text(encoding="utf-8"))

if profiles.get("schema_version") != 1 or capabilities.get("schema_version") != 1:
    raise SystemExit("unsupported execution configuration schema")

roles = profiles["planes"]["windows_decision"]["roles"]
expected_roles = ["Orchestrator", "Product", "Architecture", "Interface", "Delivery", "Quality", "Review"]
if roles != expected_roles:
    raise SystemExit("seven-role boundary drift")

worker_profiles = profiles["worker_profiles"]
if set(worker_profiles) != {"strong", "medium", "fast"}:
    raise SystemExit("worker profile pool must be strong/medium/fast")
if worker_profiles["strong"]["executor"] != "codex" or worker_profiles["strong"]["requested_model"] != "gpt-5.6-sol":
    raise SystemExit("strong profile drift")
if worker_profiles["fast"]["executor"] != "codex" or worker_profiles["fast"]["requested_model"] != "gpt-5.3-codex-spark":
    raise SystemExit("fast profile drift")
if worker_profiles["medium"]["executor"] != "claude":
    raise SystemExit("medium profile drift")
if worker_profiles["medium"]["model_identity_policy"] != "provider-reported-actual-required":
    raise SystemExit("medium actual-model policy must be provider reported")
self_recovery = profiles.get("routing", {}).get("self_recovery", {})
expected_corrections = {"fast": 1, "medium": 2, "strong": 2}
for name, expected in expected_corrections.items():
    if self_recovery.get(f"{name}_correction_cycles") != expected:
        raise SystemExit(f"{name} correction-cycle budget drift")
if self_recovery.get("blind_retry_allowed") is not False or self_recovery.get("preserve_recoverable_candidate") is not True:
    raise SystemExit("bounded self-recovery policy drift")
cadence = profiles.get("governance", {}).get("intervention_cadence", {})
if cadence.get("quality_interventions") != ["test_contract_freeze", "completed_test_batch"]:
    raise SystemExit("Quality intervention cadence drift")
if cadence.get("review_interventions") != ["design_freeze", "iteration_exit"]:
    raise SystemExit("Review intervention cadence drift")

permissions = profiles["permission_profiles"]
expected_permissions = {"test-executor", "test-author", "development", "quality-review", "release", "environment-sit"}
if set(permissions) != expected_permissions:
    raise SystemExit("permission profile drift")
if permissions["test-executor"]["sandbox"] != "read-only":
    raise SystemExit("test executor must be read-only")
if permissions["test-author"]["sandbox"] != "workspace-write" or permissions["development"]["sandbox"] != "workspace-write":
    raise SystemExit("write profile drift")
if permissions["quality-review"]["sandbox"] != "read-only":
    raise SystemExit("quality/review must be read-only")
if permissions["release"].get("credential_owner") != "Delivery":
    raise SystemExit("release credential owner drift")
environment_permission = permissions["environment-sit"]
if environment_permission.get("docker_socket") is not True or environment_permission.get("remote_access") is not False:
    raise SystemExit("Delivery environment permission drift")
environment_executor = profiles.get("managed_executors", {}).get("environment-sit", {})
if environment_executor.get("owner") != "Delivery" or environment_executor.get("model_required") is not False or environment_executor.get("ordinary_worker_access") is not False:
    raise SystemExit("Delivery environment executor drift")
if environment_executor.get("current_actions") != ["prepare_caches", "toolchain_preflight", "container_preflight"]:
    raise SystemExit("Delivery environment executor action drift")
worktree_executor = profiles.get("managed_executors", {}).get("worktree", {})
if worktree_executor.get("owner") != "Orchestrator" or worktree_executor.get("actions") != ["PrepareWorktree", "IntegrateCandidate"]:
    raise SystemExit("Orchestrator worktree executor action drift")
model_admission = profiles.get("admission", {}).get("model", {})
if model_admission.get("model_invocation_revision") != 2 or model_admission.get("test_executor_invocation_revision") != 3 or not model_admission.get("invalidation"):
    raise SystemExit("model admission policy drift")
execution_storage = capabilities.get("execution_storage", {})
if execution_storage.get("owner") != "Delivery" or execution_storage.get("filesystem") != "WSL Linux filesystem":
    raise SystemExit("execution storage ownership drift")
test_executor = profiles.get("managed_executors", {}).get("test-executor", {})
if test_executor.get("permission") != "test-executor" or test_executor.get("source_access") != "read-only" or test_executor.get("command_mode") != "runner-managed" or test_executor.get("model_required") is not True or test_executor.get("brief_model") != "gpt-5.3-codex-spark":
    raise SystemExit("runner-managed Test Executor boundary drift")
container_runtime = capabilities.get("container_runtime", {})
if container_runtime.get("owner") != "Delivery" or "Docker Desktop" not in container_runtime.get("host_runtime", ""):
    raise SystemExit("container runtime ownership drift")
if container_runtime.get("distribution") != profiles["planes"]["wsl_execution"]["distribution"]:
    raise SystemExit("container runtime distribution drift")

blocking = {name for name, value in capabilities["capabilities"].items() if value.get("blocking")}
required_blocking = {"windows_decision", "wsl_codex_read", "wsl_codex_write", "wsl_claude_write", "toolchain", "test_data", "sit_services"}
if blocking != required_blocking:
    raise SystemExit("development-blocking capability set drift")

for schema_name, schema in (("contract", contract), ("result", result)):
    if schema.get("$schema") != "https://json-schema.org/draft/2020-12/schema":
        raise SystemExit(f"{schema_name} schema draft drift")
    if schema.get("type") != "object" or schema.get("additionalProperties") is not False:
        raise SystemExit(f"{schema_name} schema must be a closed object")
    required = schema.get("required", [])
    if not required or len(required) != len(set(required)):
        raise SystemExit(f"{schema_name} schema required-field drift")

print("EXECUTION_CONFIG_OK")
PY
}

fingerprint() {
  validate_config >/dev/null
  python3 - "${script_dir}" "${profiles_file}" "${capabilities_file}" "${toolchain_lock}" <<'PY'
import datetime
import hashlib
import json
import os
import pathlib
import platform
import pwd
import shutil
import subprocess
import sys

script_dir = pathlib.Path(sys.argv[1])
files = [pathlib.Path(value) for value in sys.argv[2:]] + [script_dir / "run.sh", script_dir / "invoke-wsl.ps1"]

def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def capture(command):
    executable = shutil.which(command[0])
    if executable is None:
        return {"status": "missing", "path": None, "version": None, "sha256": None}
    completed = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=20)
    first = completed.stdout.strip().splitlines()
    return {
        "status": "ok" if completed.returncode == 0 else "error",
        "path": str(pathlib.Path(executable).resolve()),
        "version": first[0] if first else "",
        "sha256": sha256(pathlib.Path(executable).resolve()),
    }

commands = {
    "git": ["git", "--version"],
    "python3": ["python3", "--version"],
    "codex": ["codex", "--version"],
    "claude": ["claude", "--version"],
    "rustc": ["rustc", "--version"],
    "cargo": ["cargo", "--version"],
    "clang": ["clang", "--version"],
    "cmake": ["cmake", "--version"],
    "ninja": ["ninja", "--version"],
    "docker": ["docker", "--version"],
}

tools = {name: capture(command) for name, command in commands.items()}
config_hashes = {str(path.relative_to(script_dir.parent.parent)): sha256(path) for path in files}

def stable_id(value):
    canonical = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
    return "sha256:" + hashlib.sha256(canonical).hexdigest()

components = {
    "runner": stable_id({"run.sh": sha256(script_dir / "run.sh"), "invoke-wsl.ps1": sha256(script_dir / "invoke-wsl.ps1")}),
    "toolchain": stable_id({"lock": sha256(pathlib.Path(sys.argv[4])), "tools": {name: tools[name] for name in ("git", "python3", "rustc", "cargo", "clang", "cmake", "ninja")}}),
    "container": stable_id({"capabilities": sha256(pathlib.Path(sys.argv[3])), "docker": tools["docker"]}),
}

payload = {
    "schema": "ficant.execution-environment-fingerprint.v1",
    "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "distribution": os.environ.get("WSL_DISTRO_NAME", "unknown"),
    "runner_identity": {"uid": os.getuid(), "user": pwd.getpwuid(os.getuid()).pw_name, "home": str(pathlib.Path.home())},
    "host": {"system": platform.system(), "release": platform.release(), "machine": platform.machine()},
    "runner": {"entry": str(script_dir / "run.sh"), "sha256": sha256(script_dir / "run.sh")},
    "tools": tools,
    "config_hashes": config_hashes,
    "components": components,
}
identity_payload = dict(payload)
identity_payload.pop("captured_at")
payload["fingerprint_id"] = stable_id(identity_payload)
print(json.dumps(payload, ensure_ascii=False, sort_keys=True))
PY
}

ensure_canary_repo() {
  require_command git
  require_command flock
  mkdir -p -- "${cache_root}/locks" "${cache_root}/preflight-canary" "${cache_root}/cargo-home" "${build_root}"
  (
    flock -x 9
    if [[ ! -d ${canary_repo}/.git ]]; then
      rm -rf -- "${canary_repo}"
      mkdir -p -- "${canary_repo}/tests"
      git -C "${canary_repo}" init --quiet
      git -C "${canary_repo}" config user.name "ficant environment admission"
      git -C "${canary_repo}" config user.email "environment-admission@localhost"
      printf '%s\n' 'ficant minimal WSL model and permission preflight canary' >"${canary_repo}/README.md"
      : >"${canary_repo}/tests/.gitkeep"
      git -C "${canary_repo}" add README.md tests/.gitkeep
      git -C "${canary_repo}" commit --quiet -m 'initialize preflight canary'
    fi
    [[ -z "$(git -C "${canary_repo}" status --porcelain)" ]] || die "preflight canary is not clean"
    git -C "${canary_repo}" rev-parse --verify HEAD >/dev/null
  ) 9>"${cache_root}/locks/preflight-canary.lock"
}

prepare_caches() {
  validate_config >/dev/null
  ensure_canary_repo
  python3 - "${cache_root}" "${build_root}" "${canary_repo}" <<'PY'
import datetime
import json
import os
import pathlib
import sys

cache_root, build_root, canary_repo = map(pathlib.Path, sys.argv[1:])
payload = {
    "schema": "ficant.execution-storage-result.v1",
    "status": "ready",
    "checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "owner": "Delivery",
    "distribution": os.environ.get("WSL_DISTRO_NAME", "unknown"),
    "cache_root": str(cache_root.resolve()),
    "build_root": str(build_root.resolve()),
    "canary_repo": str(canary_repo.resolve()),
    "shared": ["cargo-home", "preflight-canary"],
    "isolated_per_contract": ["worktree", "build-directory", "cargo-target", "test-data", "service-instance"],
}

print(json.dumps(payload, ensure_ascii=False, sort_keys=True))
PY
}

prepare_worktree() {
  [[ $# -eq 3 ]] || die "prepare-worktree requires WORKTREE_PATH BRANCH_NAME BASE_SHA"
  local WORKTREE_PATH=$1 BRANCH_NAME=$2 BASE_SHA=$3
  local worktree_root="${repo_root}/worktrees"
  [[ ${WORKTREE_PATH} == "${worktree_root}/"* ]] || die "worktree path must remain below ${worktree_root}"
  [[ ${WORKTREE_PATH} != *"/../"* && ${WORKTREE_PATH} != *"/./"* ]] || die "worktree path must be normalized"
  [[ ${BASE_SHA} =~ ^[0-9a-f]{40}$ ]] || die "base SHA must be exact"
  [[ ${BRANCH_NAME} =~ ^codex/[A-Za-z0-9._/-]+$ ]] || die "branch must use the codex/ prefix"
  [[ ! -e ${WORKTREE_PATH} ]] || die "worktree path already exists"
  git -C "${repo_root}" cat-file -e "${BASE_SHA}^{commit}"
  ! git -C "${repo_root}" show-ref --verify --quiet "refs/heads/${BRANCH_NAME}" || die "worktree branch already exists"
  mkdir -p -- "${worktree_root}"
  git -C "${repo_root}" worktree add -b "${BRANCH_NAME}" "${WORKTREE_PATH}" "${BASE_SHA}" >/dev/null

  local status head gitdir_line
  status="$(git -C "${WORKTREE_PATH}" status --porcelain=v1 --untracked-files=all)"
  head="$(git -C "${WORKTREE_PATH}" rev-parse HEAD)"
  gitdir_line="$(head -n 1 -- "${WORKTREE_PATH}/.git")"
  if [[ -n ${status} || ${head} != "${BASE_SHA}" || ${gitdir_line} != "gitdir: /mnt/"* ]]; then
    git -C "${repo_root}" worktree remove --force "${WORKTREE_PATH}" || true
    git -C "${repo_root}" branch -D "${BRANCH_NAME}" >/dev/null 2>&1 || true
    die "WSL-native worktree postcondition failed"
  fi
  python3 - "${WORKTREE_PATH}" "${BRANCH_NAME}" "${BASE_SHA}" "${gitdir_line#gitdir: }" <<'PY'
import json
import sys
print(json.dumps({
    "schema": "ficant.worktree-preparation-result.v1",
    "status": "ready",
    "owner": "Orchestrator",
    "worktree": sys.argv[1],
    "branch": sys.argv[2],
    "base_sha": sys.argv[3],
    "head_sha": sys.argv[3],
    "gitdir": sys.argv[4],
    "worktree_clean": True,
}, sort_keys=True))
PY
}

integrate_candidate() {
  [[ $# -eq 3 ]] || die "integrate-candidate requires WORKTREE_PATH RESULT_PATH COMMIT_MESSAGE"
  local WORKTREE_PATH=$1 RESULT_PATH=$2 COMMIT_MESSAGE=$3
  local worktree_root="${repo_root}/worktrees"
  [[ ${WORKTREE_PATH} == "${worktree_root}/"* ]] || die "worktree path must remain below ${worktree_root}"
  [[ ${WORKTREE_PATH} != *"/../"* && ${WORKTREE_PATH} != *"/./"* ]] || die "worktree path must be normalized"
  [[ -d ${WORKTREE_PATH} ]] || die "candidate worktree does not exist"
  [[ -f ${RESULT_PATH} ]] || die "candidate result does not exist"
  [[ -n ${COMMIT_MESSAGE} && ${COMMIT_MESSAGE} != *$'\n'* ]] || die "commit message must be one non-empty line"
  validate_result "${RESULT_PATH}" >/dev/null

  python3 - "${WORKTREE_PATH}" "${RESULT_PATH}" "${COMMIT_MESSAGE}" <<'PY'
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

worktree = pathlib.Path(sys.argv[1]).resolve()
result_path = pathlib.Path(sys.argv[2]).resolve()
message = sys.argv[3]
result = json.loads(result_path.read_text(encoding="utf-8"))

def git(*args, env=None, check=True, text=False):
    return subprocess.run(
        ["git", "-C", str(worktree), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=check,
        env=env,
        text=text,
    )

if result["status"] != "ready" or result["candidate_state"] != "verified-diff":
    raise SystemExit("only a ready verified-diff result can be integrated")
base = result["base_sha"]
if result["candidate_sha"] != base:
    raise SystemExit("candidate result must still point at its exact base")
head = git("rev-parse", "HEAD", text=True).stdout.strip()
if head != base:
    raise SystemExit("candidate worktree HEAD does not match result base")

status_output = git("status", "--porcelain=v1", "-z", "--untracked-files=all").stdout
records = status_output.split(b"\0")
changed = set()
index = 0
while index < len(records):
    record = records[index]
    if not record:
        index += 1
        continue
    code = record[:2].decode("ascii", errors="replace")
    changed.add(record[3:].decode("utf-8", errors="surrogateescape"))
    index += 2 if "R" in code or "C" in code else 1
if sorted(changed) != sorted(result["changed_files"]):
    raise SystemExit("candidate changed-file identity mismatch")
if not changed:
    raise SystemExit("verified candidate has no working-tree changes")

common_raw = git("rev-parse", "--git-common-dir", text=True).stdout.strip()
common = pathlib.Path(common_raw)
if not common.is_absolute():
    common = (worktree / common).resolve()
main_objects = (common / "objects").resolve()
with tempfile.TemporaryDirectory(prefix="ficant-integrate-candidate-") as scratch_raw:
    scratch = pathlib.Path(scratch_raw)
    object_directory = scratch / "objects"
    (object_directory / "info").mkdir(parents=True)
    (object_directory / "info" / "alternates").write_text(str(main_objects) + "\n", encoding="utf-8")
    alternate_env = os.environ.copy()
    alternate_env.update({
        "GIT_INDEX_FILE": str(scratch / "index"),
        "GIT_OBJECT_DIRECTORY": str(object_directory),
        "GIT_ALTERNATE_OBJECT_DIRECTORIES": str(main_objects),
    })
    git("read-tree", base, env=alternate_env)
    git("add", "-A", "--", *sorted(changed), env=alternate_env)
    candidate_tree = git("write-tree", env=alternate_env, text=True).stdout.strip()
    patch = git("diff", "--cached", "--binary", "--full-index", base, "--", env=alternate_env).stdout
    patch_sha = hashlib.sha256(patch).hexdigest()

if candidate_tree != result["candidate_tree"]:
    raise SystemExit("candidate tree identity mismatch")
if patch_sha != result["candidate_diff_sha256"]:
    raise SystemExit("candidate patch identity mismatch")

try:
    git("add", "-A", "--", *sorted(changed))
    staged_tree = git("write-tree", text=True).stdout.strip()
    if staged_tree != candidate_tree:
        raise RuntimeError("staged candidate tree identity mismatch")
    git("commit", "-m", message)
except Exception:
    git("reset", "--mixed", base, check=False)
    raise

commit = git("rev-parse", "HEAD", text=True).stdout.strip()
parent = git("rev-parse", "HEAD^", text=True).stdout.strip()
committed_tree = git("rev-parse", "HEAD^{tree}", text=True).stdout.strip()
branch = git("symbolic-ref", "--short", "HEAD", text=True).stdout.strip()
clean = not git("status", "--porcelain=v1", "--untracked-files=all").stdout
if parent != base or committed_tree != candidate_tree or not clean:
    raise SystemExit("committed candidate postcondition failed")

print(json.dumps({
    "schema": "ficant.candidate-integration-result.v1",
    "status": "ready",
    "owner": "Orchestrator",
    "branch": branch,
    "base_sha": base,
    "commit_sha": commit,
    "candidate_tree": committed_tree,
    "candidate_diff_sha256": patch_sha,
    "changed_files": sorted(changed),
    "worktree": str(worktree),
    "worktree_clean": clean,
}, ensure_ascii=False, sort_keys=True))
PY
}

toolchain_preflight() {
  validate_config >/dev/null
  local evidence_dir="${evidence_root}/toolchain-preflight-$(date -u +%Y%m%dT%H%M%SZ)-$$"
  local fingerprint_file="${evidence_dir}/fingerprint.json"
  mkdir -p -- "${evidence_dir}"
  fingerprint >"${fingerprint_file}"
  local status
  status="$(python3 - "${fingerprint_file}" "${toolchain_lock}" <<'PY'
import json
import pathlib
import sys
import tomllib

fingerprint = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
lock = tomllib.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8"))
expected = {
    "rustc": lock["rust"]["version"],
    "cargo": lock["rust"]["version"],
    "clang": lock["clang"]["version"],
    "cmake": lock["cmake"]["version"],
    "ninja": lock["ninja"]["version"],
}
ready = True
for name, version in expected.items():
    tool = fingerprint["tools"][name]
    if tool["status"] != "ok" or version not in (tool.get("version") or ""):
        ready = False
print("ready" if ready else "blocked")
PY
)"
  python3 - "${status}" "${fingerprint_file}" "${toolchain_lock}" <<'PY'
import datetime
import json
import pathlib
import sys
import tomllib

status, fingerprint_path, lock_path = sys.argv[1:]
fingerprint = json.loads(pathlib.Path(fingerprint_path).read_text(encoding="utf-8"))
lock = tomllib.loads(pathlib.Path(lock_path).read_text(encoding="utf-8"))
expected = {
    "rustc": lock["rust"]["version"],
    "cargo": lock["rust"]["version"],
    "clang": lock["clang"]["version"],
    "cmake": lock["cmake"]["version"],
    "ninja": lock["ninja"]["version"],
}
checks = {}
for name, version in expected.items():
    tool = fingerprint["tools"][name]
    observed = tool.get("version")
    passed = tool["status"] == "ok" and version in (observed or "")
    checks[name] = {"expected": version, "observed": observed, "path": tool.get("path"), "status": "pass" if passed else "fail"}
failed = [name for name, check in checks.items() if check["status"] == "fail"]
payload = {
    "schema": "ficant.toolchain-capability-result.v1",
    "status": status,
    "checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "owner": "Delivery",
    "environment_fingerprint": fingerprint["fingerprint_id"],
    "toolchain_component": fingerprint["components"]["toolchain"],
    "checks": checks,
    "blockers": [] if not failed else ["runner identity does not match locked toolchain: " + ", ".join(failed)],
}
print(json.dumps(payload, ensure_ascii=False, sort_keys=True))
PY
  [[ ${status} == ready ]]
}

container_preflight() {
  validate_config >/dev/null
  local evidence_dir="${evidence_root}/container-preflight-$(date -u +%Y%m%dT%H%M%SZ)-$$"
  local fingerprint_file="${evidence_dir}/fingerprint.json"
  local version_file="${evidence_dir}/docker-version.txt"
  local compose_file="${evidence_dir}/docker-compose-version.txt"
  local info_file="${evidence_dir}/docker-info.txt"
  mkdir -p -- "${evidence_dir}"
  fingerprint >"${fingerprint_file}"

  local version_exit=127 compose_exit=127 info_exit=127
  if command -v docker >/dev/null 2>&1; then
    version_exit=0
    docker version >"${version_file}" 2>&1 || version_exit=$?
    compose_exit=0
    docker compose version >"${compose_file}" 2>&1 || compose_exit=$?
    info_exit=0
    docker info >"${info_file}" 2>&1 || info_exit=$?
  else
    printf '%s\n' 'docker command not found in WSL runner identity' >"${version_file}"
    cp -- "${version_file}" "${compose_file}"
    cp -- "${version_file}" "${info_file}"
  fi

  local status=blocked
  if [[ ${version_exit} -eq 0 && ${compose_exit} -eq 0 && ${info_exit} -eq 0 ]]; then
    status=ready
  fi
  python3 - "${status}" "${fingerprint_file}" "${version_exit}" "${compose_exit}" "${info_exit}" <<'PY'
import datetime
import json
import pathlib
import sys

status, fingerprint_path, version_exit, compose_exit, info_exit = sys.argv[1:]
fingerprint = json.loads(pathlib.Path(fingerprint_path).read_text(encoding="utf-8"))
checks = {
    "docker_client_server": {"exit_code": int(version_exit), "status": "pass" if int(version_exit) == 0 else "fail"},
    "docker_compose": {"exit_code": int(compose_exit), "status": "pass" if int(compose_exit) == 0 else "fail"},
    "docker_daemon_info": {"exit_code": int(info_exit), "status": "pass" if int(info_exit) == 0 else "fail"},
}
blockers = [] if status == "ready" else [
    "Docker Desktop is not reachable from the ficant WSL runner identity; Delivery must request Human Operator startup/WSL Integration when required"
]
payload = {
    "schema": "ficant.container-capability-result.v1",
    "status": status,
    "checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "owner": "Delivery",
    "executor": "environment-sit",
    "host_runtime": "Windows Docker Desktop with WSL2 Linux engine",
    "distribution": fingerprint["distribution"],
    "environment_fingerprint": fingerprint["fingerprint_id"],
    "checks": checks,
    "blockers": blockers,
    "ordinary_worker_docker_socket": False,
    "human_operator_boundary": "Docker Desktop GUI/startup/administrator/WSL Integration only",
}
print(json.dumps(payload, ensure_ascii=False, sort_keys=True))
PY
  [[ ${status} == ready ]]
}

load_route() {
  local profile=$1 permission=$2
  eval "$(python3 - "${profiles_file}" "${profile}" "${permission}" <<'PY'
import shlex
import sys
import tomllib

with open(sys.argv[1], "rb") as stream:
    config = tomllib.load(stream)
profile_name, permission_name = sys.argv[2:]
try:
    profile = config["worker_profiles"][profile_name]
    permission = config["permission_profiles"][permission_name]
except KeyError as error:
    raise SystemExit(f"unknown execution route: {error}")
values = {
    "PROFILE_NAME": profile_name,
    "EXECUTOR": profile["executor"],
    "REQUESTED_MODEL": profile["requested_model"],
    "MODEL_IDENTITY_POLICY": profile["model_identity_policy"],
    "PERMISSION_NAME": permission_name,
    "SANDBOX": permission["sandbox"],
    "CORRECTION_CYCLES": config["routing"]["self_recovery"][f"{profile_name}_correction_cycles"],
}
for key, value in values.items():
    print(f"{key}={shlex.quote(str(value))}")
PY
)"
}

model_admission_fingerprint() {
  local actual_model=$1
  local executable
  executable="$(command -v "${EXECUTOR}")"
  python3 - "${profiles_file}" "${PROFILE_NAME}" "${PERMISSION_NAME}" "${actual_model}" "${executable}" <<'PY'
import hashlib
import json
import pathlib
import subprocess
import sys
import tomllib

profiles_path, profile_name, permission_name, actual_model, executable = sys.argv[1:]
profiles = tomllib.loads(pathlib.Path(profiles_path).read_text(encoding="utf-8"))
path = pathlib.Path(executable).resolve()
version = subprocess.run([str(path), "--version"], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=20).stdout.strip().splitlines()
revision_key = "test_executor_invocation_revision" if permission_name == "test-executor" else "model_invocation_revision"
payload = {
    "schema": "ficant.model-admission-input.v1",
    "model_invocation_revision": profiles["admission"]["model"][revision_key],
    "profile_name": profile_name,
    "profile": profiles["worker_profiles"][profile_name],
    "permission_name": permission_name,
    "permission": profiles["permission_profiles"][permission_name],
    "actual_model": actual_model,
    "cli": {
        "path": str(path),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "version": version[0] if version else "",
    },
}
canonical = json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
print("sha256:" + hashlib.sha256(canonical).hexdigest())
PY
}

invoke_model() {
  local cwd=$1 prompt_file=$2 raw_file=$3 timeout_seconds=$4
  local exit_code=0
  if [[ ${EXECUTOR} == codex ]]; then
    local codex_sandbox=${SANDBOX}
    [[ ${codex_sandbox} == read-only || ${codex_sandbox} == workspace-write ]] || die "unsupported Codex sandbox: ${codex_sandbox}"
    (
      cd -- "${cwd}"
      timeout "${timeout_seconds}" codex exec --ephemeral --model "${REQUESTED_MODEL}" --sandbox "${codex_sandbox}" --json - <"${prompt_file}"
    ) >"${raw_file}" 2>&1 || exit_code=$?
  elif [[ ${EXECUTOR} == claude ]]; then
    local permission_mode
    case ${SANDBOX} in
      read-only) permission_mode=plan ;;
      workspace-write) permission_mode=acceptEdits ;;
      *) die "unsupported Claude permission: ${SANDBOX}" ;;
    esac
    (
      cd -- "${cwd}"
      timeout "${timeout_seconds}" claude -p --model "${REQUESTED_MODEL}" --output-format json --permission-mode "${permission_mode}" <"${prompt_file}"
    ) >"${raw_file}" 2>&1 || exit_code=$?
  else
    die "unsupported executor: ${EXECUTOR}"
  fi
  printf '%s\n' "${exit_code}"
}

model_identity() {
  local raw_file=$1 output_file=$2
  python3 - "${EXECUTOR}" "${REQUESTED_MODEL}" "${raw_file}" "${output_file}" <<'PY'
import json
import pathlib
import sys

executor, requested, raw_path, output_path = sys.argv[1:]
text = pathlib.Path(raw_path).read_text(encoding="utf-8", errors="replace")
objects = []
try:
    objects.append(json.loads(text))
except json.JSONDecodeError:
    for line in text.splitlines():
        try:
            objects.append(json.loads(line))
        except json.JSONDecodeError:
            pass

actual = None
source = "unverified"
if executor == "claude":
    for obj in objects:
        if isinstance(obj, dict) and isinstance(obj.get("modelUsage"), dict):
            candidates = [key for key in obj["modelUsage"] if key and not key.startswith("<")]
            if candidates:
                actual = sorted(candidates)[0]
                source = "provider-reported-actual"
                break
else:
    actual = requested
    source = "explicit-cli-selection"

payload = {"actual_model": actual or "unverified", "model_identity_source": source}
pathlib.Path(output_path).write_text(json.dumps(payload, sort_keys=True), encoding="utf-8")
PY
}

preflight() {
  [[ $# -eq 2 ]] || die "preflight requires PROFILE PERMISSION"
  local total_start
  total_start="$(now_ms)"
  validate_config >/dev/null
  load_route "$1" "$2"
  [[ ${PERMISSION_NAME} != release ]] || die "release profile is Delivery-only"
  require_command git
  require_command flock
  require_command timeout
  require_command "${EXECUTOR}"

  local run_id="$(date -u +%Y%m%dT%H%M%SZ)-${PROFILE_NAME}-${PERMISSION_NAME}-$$"
  local evidence_dir="${evidence_root}/${run_id}"
  local worktree="/tmp/ficant-preflight-worktrees/${run_id}"
  local prompt_file="${evidence_dir}/prompt.txt"
  local raw_file="${evidence_dir}/raw-output.jsonl"
  local fingerprint_file="${evidence_dir}/fingerprint.json"
  local identity_file="${evidence_dir}/model-identity.json"
  local marker="tests/execution-preflight-${run_id}.txt"
  local prepare_start prepare_end model_start model_end validation_start validation_end
  prepare_start="$(now_ms)"
  ensure_canary_repo
  mkdir -p -- "${evidence_dir}" "$(dirname -- "${worktree}")"
  fingerprint >"${fingerprint_file}"
  local base_sha
  base_sha="$(git -C "${canary_repo}" rev-parse HEAD)"
  flock -x "${cache_root}/locks/preflight-canary.lock" git -C "${canary_repo}" worktree add --detach "${worktree}" "${base_sha}" >/dev/null
  prepare_end="$(now_ms)"

  local worktree_removed=false
  cleanup_preflight() {
    rm -f -- "${worktree}/${marker}" || true
    if flock -x "${cache_root}/locks/preflight-canary.lock" git -C "${canary_repo}" worktree remove --force "${worktree}" >/dev/null 2>&1; then
      worktree_removed=true
    fi
  }
  trap cleanup_preflight RETURN

  if [[ ${SANDBOX} == read-only ]]; then
    cat >"${prompt_file}" <<'EOF'
Do not call tools and do not write files. Respond with exactly: EXECUTION_READ_ONLY_OK
EOF
  else
    printf '%s\n' \
      "Use the file editing tool to create exactly ${marker} with the single line EXECUTION_WORKSPACE_WRITE_OK." \
      "Do not modify any other path. Do not commit, create a worktree, switch branches, or access credentials." \
      "After writing the file, respond with exactly: EXECUTION_WORKSPACE_WRITE_OK" >"${prompt_file}"
  fi

  local model_exit
  model_start="$(now_ms)"
  model_exit="$(invoke_model "${worktree}" "${prompt_file}" "${raw_file}" 300)"
  model_end="$(now_ms)"
  validation_start="$(now_ms)"
  model_identity "${raw_file}" "${identity_file}"
  local marker_ok=false response_ok=false identity_ok=false worktree_clean=false
  if [[ ${SANDBOX} == read-only ]]; then
    grep -Fq 'EXECUTION_READ_ONLY_OK' "${raw_file}" && response_ok=true
    [[ -z "$(git -C "${worktree}" status --porcelain)" ]] && marker_ok=true
  else
    grep -Fq 'EXECUTION_WORKSPACE_WRITE_OK' "${raw_file}" && response_ok=true
    [[ -f ${worktree}/${marker} ]] && grep -Fxq 'EXECUTION_WORKSPACE_WRITE_OK' "${worktree}/${marker}" && marker_ok=true
    rm -f -- "${worktree}/${marker}"
  fi
  [[ -z "$(git -C "${worktree}" status --porcelain)" ]] && worktree_clean=true
  python3 - "${identity_file}" "${MODEL_IDENTITY_POLICY}" <<'PY' && identity_ok=true
import json, pathlib, sys
identity = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
policy = sys.argv[2]
if policy == "provider-reported-actual-required" and identity["model_identity_source"] != "provider-reported-actual":
    raise SystemExit(1)
if identity["actual_model"] == "unverified":
    raise SystemExit(1)
PY

  cleanup_preflight
  trap - RETURN
  local status=failed passed=0 failed=1 cleanup_ok=false
  if [[ ${model_exit} -eq 0 && ${response_ok} == true && ${marker_ok} == true && ${identity_ok} == true && ${worktree_clean} == true && ${worktree_removed} == true ]]; then
    status=ready
    passed=1
    failed=0
    cleanup_ok=true
  fi
  local actual_model admission_fingerprint
  actual_model="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["actual_model"])' "${identity_file}")"
  admission_fingerprint="$(model_admission_fingerprint "${actual_model}")"
  local base_tree
  base_tree="$(git -C "${canary_repo}" rev-parse "${base_sha}^{tree}")"
  validation_end="$(now_ms)"
  local worktree_prepare_ms model_execution_ms runner_validation_ms total_ms
  worktree_prepare_ms=$((prepare_end - prepare_start))
  model_execution_ms=$((model_end - model_start))
  runner_validation_ms=$((validation_end - validation_start))
  total_ms=$((validation_end - total_start))

  python3 - "${status}" "${PROFILE_NAME}" "${EXECUTOR}" "${REQUESTED_MODEL}" "${identity_file}" \
    "${PERMISSION_NAME}" "${fingerprint_file}" "${base_sha}" "${base_tree}" "${model_exit}" "${passed}" "${failed}" \
    "${raw_file}" "${evidence_dir}" "${worktree_clean}" "${cleanup_ok}" "${admission_fingerprint}" \
    "${worktree_prepare_ms}" "${model_execution_ms}" "${runner_validation_ms}" "${total_ms}" "${CORRECTION_CYCLES}" <<'PY'
import hashlib
import json
import pathlib
import sys

(status, profile, executor, requested, identity_path, permission, fingerprint_path, base_sha, base_tree,
 model_exit, passed, failed, raw_path, evidence_dir, worktree_clean, cleanup_ok, admission_fingerprint,
 worktree_prepare_ms, model_execution_ms, runner_validation_ms, total_ms, correction_cycles) = sys.argv[1:]
identity = json.loads(pathlib.Path(identity_path).read_text(encoding="utf-8"))
fingerprint = json.loads(pathlib.Path(fingerprint_path).read_text(encoding="utf-8"))
def digest(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()
payload = {
    "schema_version": 4,
    "status": status,
    "checklist_id": "I3-ENV-ADMISSION",
    "task_id": f"PRE-{profile.upper()}-{permission.upper()}",
    "profile": profile,
    "executor": executor,
    "command_executor": "worker-direct",
    "requested_model": requested,
    "actual_model": identity["actual_model"],
    "model_identity_source": identity["model_identity_source"],
    "spark_brief": None,
    "permission_profile": permission,
    "environment": "wsl",
    "environment_fingerprint": fingerprint["fingerprint_id"],
    "model_admission_fingerprint": admission_fingerprint,
    "base_sha": base_sha,
    "candidate_sha": base_sha,
    "candidate_state": "base-clean",
    "candidate_tree": base_tree,
    "candidate_diff_sha256": None,
    "changed_files": [],
    "commands": [{"kind": "preflight", "argv": [executor, "non-interactive", "explicit-model", permission], "cwd": "/tmp/ficant-preflight-worktrees", "exit_code": int(model_exit), "duration_ms": int(model_execution_ms), "expected_tests": 1}],
    "tests": {"passed": int(passed), "failed": int(failed), "skipped": 0, "total": int(passed) + int(failed)},
    "evidence": [
        {"path": raw_path, "sha256": digest(raw_path)},
        {"path": fingerprint_path, "sha256": digest(fingerprint_path)}
    ],
    "escalated": False,
    "escalation_reason": None,
    "blockers": [] if status == "ready" else ["runner preflight did not satisfy model, permission, write/read, or cleanup contract"],
    "summary": f"{profile}/{permission} WSL runner preflight {status}",
    "recovery": {"correction_budget": int(correction_cycles), "correction_cycles_used": 0, "events": []},
    "timings": {
        "external_queue_ms": None,
        "worktree_prepare_ms": int(worktree_prepare_ms),
        "model_execution_ms": int(model_execution_ms),
        "runner_validation_ms": int(runner_validation_ms),
        "environment_wait_ms": 0,
        "mentor_validation_ms": None,
        "total_ms": int(total_ms),
    },
    "cleanup": {"worktree_clean": worktree_clean == "true", "temporary_paths_removed": cleanup_ok == "true"},
}
print(json.dumps(payload, ensure_ascii=False, sort_keys=True))
PY
  [[ ${status} == ready ]]
}

validate_contract() {
  local contract=$1
  python3 - "${contract}" "${profiles_file}" <<'PY'
import json
import pathlib
import sys
import tomllib

contract = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
profiles = tomllib.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8"))
required = {
    "schema_version", "checklist_id", "task_id", "ids", "profile", "permission_profile",
    "model", "model_admission_fingerprint", "environment_fingerprint", "base_sha", "worktree", "allowed_paths",
    "forbidden_paths", "frozen_contracts", "expected_oracle", "context_files", "commands",
    "timeout_seconds", "result_path", "cleanup", "recovery_policy", "mentor", "escalation_conditions", "fallback",
}
missing = sorted(required - contract.keys())
if missing:
    raise SystemExit("contract missing: " + ", ".join(missing))
if contract["schema_version"] != 4:
    raise SystemExit("contract schema version drift")
profile = profiles["worker_profiles"].get(contract["profile"])
permission = profiles["permission_profiles"].get(contract["permission_profile"])
if profile is None or permission is None:
    raise SystemExit("unknown profile or permission")
model = contract["model"]
expected_model = "gpt-5.3-codex-spark" if contract["permission_profile"] == "test-executor" else profile["requested_model"]
if model.get("requested") != expected_model or model.get("actual_identity_required") is not True:
    raise SystemExit("contract model route drift")
if profile["model_identity_policy"] == "explicit-cli-selection" and model.get("actual") != model.get("requested"):
    raise SystemExit("Codex contract actual model must equal the explicit model slug")
if profile["model_identity_policy"] == "provider-reported-actual-required" and model.get("actual") in {None, "", "claude", "sonnet", "unverified", "provider-reported-at-runtime"}:
    raise SystemExit("Claude contract must contain the admitted provider-reported actual model")
if not isinstance(contract["model_admission_fingerprint"], str) or not contract["model_admission_fingerprint"].startswith("sha256:") or len(contract["model_admission_fingerprint"]) != 71:
    raise SystemExit("contract model admission fingerprint drift")
if not contract["worktree"].startswith("/") or not contract["result_path"].startswith("/"):
    raise SystemExit("contract paths must be WSL POSIX paths")
if len(contract["base_sha"]) != 40 or any(ch not in "0123456789abcdef" for ch in contract["base_sha"]):
    raise SystemExit("contract base SHA is not exact")
if not contract["allowed_paths"] or not contract["forbidden_paths"]:
    raise SystemExit("contract path boundaries must be non-empty")
recovery = contract["recovery_policy"]
expected_cycles = profiles["routing"]["self_recovery"][f'{contract["profile"]}_correction_cycles']
if recovery.get("max_correction_cycles") != expected_cycles:
    raise SystemExit("contract correction-cycle budget does not match Worker profile")
if recovery.get("preserve_candidate_on_recoverable_blocker") is not True or not recovery.get("immediate_stop_conditions"):
    raise SystemExit("contract recovery policy drift")
for kind in ("green", "regression"):
    if not contract["commands"].get(kind):
        raise SystemExit(f"contract requires {kind} commands")
for values in contract["commands"].values():
    if contract["permission_profile"] == "test-executor":
        if not all(isinstance(value, dict) and set(value) == {"argv", "cwd", "timeout_seconds", "expected_tests"} for value in values):
            raise SystemExit("test-executor requires runner-managed structured commands with expected_tests")
    elif not all(isinstance(value, str) and value for value in values):
        raise SystemExit("worker-direct commands must remain strings")
print("EXECUTION_CONTRACT_OK")
PY
}

validate_result() {
  local result=$1
  python3 - "${result}" "${result_schema}" "${profiles_file}" <<'PY'
import json
import pathlib
import re
import sys
import tomllib

document = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
schema = json.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8"))
profiles = tomllib.loads(pathlib.Path(sys.argv[3]).read_text(encoding="utf-8"))
required = set(schema["required"])
properties = set(schema["properties"])
missing = sorted(required - document.keys())
extra = sorted(document.keys() - properties)
if missing or extra:
    raise SystemExit(f"result shape drift missing={missing} extra={extra}")
if document["schema_version"] != 4 or document["status"] not in {"ready", "blocked", "failed"}:
    raise SystemExit("result identity/status drift")
if document["profile"] not in profiles["worker_profiles"] or document["permission_profile"] not in profiles["permission_profiles"]:
    raise SystemExit("result route drift")
profile = profiles["worker_profiles"][document["profile"]]
expected_executor = "codex" if document["permission_profile"] == "test-executor" else profile["executor"]
expected_command_executor = "runner-managed" if document["permission_profile"] == "test-executor" else "worker-direct"
if document["executor"] != expected_executor or document["command_executor"] != expected_command_executor:
    raise SystemExit("result executor/model drift")
if document["profile"] == "medium":
    if document["model_identity_source"] != "provider-reported-actual" or document["actual_model"] in {"claude", "sonnet", "unverified"}:
        raise SystemExit("medium result lacks provider-reported actual model")
elif document["model_identity_source"] != "explicit-cli-selection" or document["actual_model"] != document["requested_model"]:
    raise SystemExit("Codex result lacks explicit model identity")
if document["environment"] != "wsl" or not re.fullmatch(r"sha256:[0-9a-f]{64}", document["environment_fingerprint"]):
    raise SystemExit("result environment identity drift")
if not re.fullmatch(r"sha256:[0-9a-f]{64}", document["model_admission_fingerprint"]):
    raise SystemExit("result model admission identity drift")
for field in ("base_sha", "candidate_sha", "candidate_tree"):
    if not re.fullmatch(r"[0-9a-f]{40}", document[field]):
        raise SystemExit(f"result {field} drift")
if document["candidate_state"] not in {"base-clean", "committed", "uncommitted-diff", "verified-diff", "blocked-with-candidate"}:
    raise SystemExit("result candidate state drift")
diff_sha = document["candidate_diff_sha256"]
if diff_sha is not None and not re.fullmatch(r"[0-9a-f]{64}", diff_sha):
    raise SystemExit("result candidate diff identity drift")
if document["candidate_state"] in {"uncommitted-diff", "verified-diff", "blocked-with-candidate", "committed"} and diff_sha is None:
    raise SystemExit("candidate diff state lacks diff identity")
if document["candidate_state"] == "verified-diff":
    if document["status"] != "ready" or document["candidate_sha"] != document["base_sha"] or document["cleanup"]["worktree_clean"]:
        raise SystemExit("verified diff handoff boundary drift")
    if not any(item["path"].endswith("/candidate.patch") and item["sha256"] == diff_sha for item in document["evidence"]):
        raise SystemExit("verified diff lacks candidate.patch evidence")
if document["candidate_state"] == "blocked-with-candidate":
    if document["status"] != "blocked" or document["candidate_sha"] != document["base_sha"] or document["cleanup"]["worktree_clean"] or not document["blockers"]:
        raise SystemExit("blocked candidate preservation boundary drift")
    if not any(item["path"].endswith("/candidate.patch") and item["sha256"] == diff_sha for item in document["evidence"]):
        raise SystemExit("blocked candidate lacks candidate.patch evidence")
recovery = document["recovery"]
expected_budget = profiles["routing"]["self_recovery"][f'{document["profile"]}_correction_cycles']
if recovery["correction_budget"] != expected_budget or recovery["correction_cycles_used"] > recovery["correction_budget"]:
    raise SystemExit("result correction-cycle budget drift")
if len(recovery["events"]) != recovery["correction_cycles_used"]:
    raise SystemExit("result recovery event count drift")
for index, event in enumerate(recovery["events"], start=1):
    if event["cycle"] != index:
        raise SystemExit("result recovery event sequence drift")
tests = document["tests"]
if tests["total"] != tests["passed"] + tests["failed"] + tests["skipped"]:
    raise SystemExit("test count arithmetic drift")
if not document["commands"] or tests["total"] == 0:
    raise SystemExit("result must include executed commands and non-zero test/check count")
for command in document["commands"]:
    if set(command) != {"kind", "argv", "cwd", "exit_code", "duration_ms", "expected_tests"} or not command["cwd"].startswith("/") or not isinstance(command["duration_ms"], int) or command["duration_ms"] < 0 or not isinstance(command["expected_tests"], int) or command["expected_tests"] < 0:
        raise SystemExit("command evidence drift")
if document["permission_profile"] == "test-executor":
    expected_total = sum(command["expected_tests"] for command in document["commands"])
    if document["status"] == "ready" and (document["spark_brief"] is None or tests["passed"] != expected_total or tests["failed"] != 0 or tests["total"] != expected_total):
        raise SystemExit("runner-managed expected-tests/Spark brief drift")
timings = document["timings"]
required_timings = {"external_queue_ms", "worktree_prepare_ms", "model_execution_ms", "runner_validation_ms", "environment_wait_ms", "mentor_validation_ms", "total_ms"}
if set(timings) != required_timings:
    raise SystemExit("result timing shape drift")
for name, value in timings.items():
    if value is not None and (not isinstance(value, int) or value < 0):
        raise SystemExit(f"result timing value drift: {name}")
if not document["evidence"]:
    raise SystemExit("result evidence missing")
for evidence in document["evidence"]:
    if not evidence["path"].startswith("/") or not re.fullmatch(r"[0-9a-f]{64}", evidence["sha256"]):
        raise SystemExit("result evidence identity drift")
print("EXECUTION_RESULT_OK")
PY
}

run_managed_test_executor() {
  local contract=$1 result_target=$2 current_environment=$3 current_admission=$4
  mkdir -p -- "${evidence_root}" "${build_root}"
  python3 - "${contract}" "${result_target}" "${evidence_root}" "${build_root}" "${current_environment}" "${current_admission}" "${REQUESTED_MODEL}" <<'PY'
import datetime, hashlib, json, os, pathlib, shutil, subprocess, sys, time

contract_path, result_path, evidence_root, build_root, current_environment, current_admission, spark_model = sys.argv[1:]
c = json.loads(pathlib.Path(contract_path).read_text(encoding="utf-8"))
source = pathlib.Path(c["worktree"]).resolve()
if c["permission_profile"] != "test-executor":
    raise SystemExit("managed executor requires test-executor permission")
for kind in ("red", "green", "regression"):
    for command in c["commands"].get(kind, []):
        if not isinstance(command, dict) or set(command) != {"argv", "cwd", "timeout_seconds", "expected_tests"}:
            raise SystemExit("runner-managed commands require exact structured argv/cwd/timeout_seconds/expected_tests")
        if not command["argv"] or not all(isinstance(v, str) and v and "\x00" not in v for v in command["argv"]):
            raise SystemExit("malformed argv")
        if not isinstance(command["expected_tests"], int) or isinstance(command["expected_tests"], bool) or command["expected_tests"] < 0:
            raise SystemExit("malformed expected_tests")
        if command["cwd"] != "${SOURCE}" and not command["cwd"].startswith("${SOURCE}/"):
            raise SystemExit("unsupported cwd/placeholders")

def git(*args, text=True):
    return subprocess.run(["git", "-C", str(source), *args], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=text).stdout

if git("rev-parse", "HEAD").strip() != c["base_sha"]:
    raise SystemExit("stale source snapshot identity")
before = git("status", "--porcelain=v1", "--untracked-files=all")
run_id = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-test-executor-" + str(os.getpid())
scratch = pathlib.Path(build_root) / run_id
snapshot = scratch / "source-snapshot"
evidence = pathlib.Path(evidence_root) / run_id
snapshot.mkdir(parents=True)
evidence.mkdir(parents=True)
archive = subprocess.Popen(["git", "-C", str(source), "archive", "--format=tar", c["base_sha"]], stdout=subprocess.PIPE)
extract = subprocess.run(["tar", "-xf", "-", "-C", str(snapshot)], stdin=archive.stdout)
archive.stdout.close()
archive_status = archive.wait()
if archive_status or extract.returncode:
    raise SystemExit("source snapshot creation failed")

records, command_failed = [], False
brief = None
model_ms = 0
try:
    for kind in ("red", "green", "regression"):
        for index, command in enumerate(c["commands"].get(kind, []), 1):
            relative = command["cwd"][len("${SOURCE}"):].lstrip("/")
            cwd = (snapshot / relative).resolve()
            if cwd != snapshot and snapshot not in cwd.parents:
                raise SystemExit("unsupported cwd/placeholders")
            if not cwd.is_dir():
                raise SystemExit("managed command cwd is missing")
            started = time.monotonic_ns()
            timed_out = False
            try:
                completed = subprocess.run(command["argv"], cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=command["timeout_seconds"])
                exit_code = completed.returncode
                stdout, stderr = completed.stdout, completed.stderr
            except subprocess.TimeoutExpired as error:
                timed_out, exit_code = True, 124
                stdout, stderr = error.stdout or b"", error.stderr or b""
            duration = (time.monotonic_ns() - started) // 1_000_000
            stem = f"{len(records)+1:03d}-{kind}"
            out_path, err_path = evidence / f"{stem}.stdout", evidence / f"{stem}.stderr"
            out_path.write_bytes(stdout); err_path.write_bytes(stderr)
            record = {"kind": kind, "argv": command["argv"], "cwd": str(cwd), "exit_code": exit_code, "duration_ms": duration,
                      "expected_tests": command["expected_tests"],
                      "stdout": {"path": str(out_path), "sha256": hashlib.sha256(stdout).hexdigest()},
                      "stderr": {"path": str(err_path), "sha256": hashlib.sha256(stderr).hexdigest()}}
            records.append(record)
            if timed_out:
                raise SystemExit("managed command timeout")
            if exit_code != 0:
                command_failed = True
                break
        if command_failed:
            break
    after = git("status", "--porcelain=v1", "--untracked-files=all")
    if after != before:
        raise SystemExit("original worktree changed during runner-managed execution")
    command_evidence = evidence / "command-evidence.json"
    command_evidence.write_text(json.dumps(records, sort_keys=True) + "\n", encoding="utf-8")
    evidence_items = [{"path": str(command_evidence), "sha256": hashlib.sha256(command_evidence.read_bytes()).hexdigest()}]
    for record in records:
        evidence_items.extend([record["stdout"], record["stderr"]])
    shutil.rmtree(scratch)
    if scratch.exists():
        raise SystemExit("runner scratch cleanup failed")
    tree = git("rev-parse", "HEAD^{tree}").strip()
    if not command_failed:
        compact = [{k: r[k] for k in ("kind", "argv", "exit_code", "duration_ms", "expected_tests")} for r in records]
        prompt = evidence / "spark-prompt.txt"
        raw = evidence / "spark-raw.jsonl"
        prompt.write_text("Do not call tools or modify files. Consume this runner evidence and return one JSON object with exactly status, summary, observations. status must be candidate. Runner evidence is authoritative: " + json.dumps(compact, separators=(",", ":")), encoding="utf-8")
        started = time.monotonic_ns()
        completed = subprocess.run(["codex", "exec", "--ephemeral", "--model", spark_model, "--sandbox", "read-only", "--json", "-"], cwd=source, input=prompt.read_bytes(), stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=min(c["timeout_seconds"], 300))
        model_ms = (time.monotonic_ns() - started) // 1_000_000
        raw.write_bytes(completed.stdout)
        if completed.returncode:
            raise SystemExit("Spark brief invocation failed")
        objects = []
        for text in [completed.stdout.decode("utf-8", "replace"), *completed.stdout.decode("utf-8", "replace").splitlines()]:
            try: objects.append(json.loads(text))
            except json.JSONDecodeError: pass
        def visit(value):
            if isinstance(value, dict):
                if set(value) == {"status", "summary", "observations"}: return value
                for child in value.values():
                    found = visit(child)
                    if found: return found
            if isinstance(value, list):
                for child in value:
                    found = visit(child)
                    if found: return found
            if isinstance(value, str):
                try: return visit(json.loads(value))
                except (json.JSONDecodeError, TypeError): return None
            return None
        for obj in objects:
            brief = visit(obj)
            if brief: break
        if not brief or brief.get("status") != "candidate" or not isinstance(brief.get("summary"), str) or not isinstance(brief.get("observations"), list):
            raise SystemExit("missing/invalid Spark brief")
        evidence_items.extend([{"path": str(raw), "sha256": hashlib.sha256(raw.read_bytes()).hexdigest()}, {"path": str(prompt), "sha256": hashlib.sha256(prompt.read_bytes()).hexdigest()}])
    expected_total = sum(r["expected_tests"] for r in records)
    passed = expected_total if not command_failed else 0
    failed = expected_total if command_failed else 0
    result = {
      "schema_version": 4, "status": "ready" if not command_failed else "failed", "checklist_id": c["checklist_id"], "task_id": c["task_id"],
      "profile": c["profile"], "executor": "codex", "command_executor": "runner-managed", "requested_model": spark_model, "actual_model": spark_model,
      "model_identity_source": "explicit-cli-selection", "permission_profile": "test-executor", "environment": "wsl",
      "spark_brief": brief, "environment_fingerprint": current_environment, "base_sha": c["base_sha"], "model_admission_fingerprint": current_admission,
      "candidate_sha": c["base_sha"], "candidate_state": "base-clean", "candidate_tree": tree, "candidate_diff_sha256": None, "changed_files": [],
      "commands": [{k: r[k] for k in ("kind", "argv", "cwd", "exit_code", "duration_ms", "expected_tests")} for r in records],
      "tests": {"passed": passed, "failed": failed, "skipped": 0, "total": expected_total}, "evidence": evidence_items,
      "escalated": False, "escalation_reason": None, "blockers": [] if not command_failed else ["runner-managed command failed"],
      "summary": "Runner evidence is authoritative; Spark supplied a candidate brief only.",
      "recovery": {"correction_budget": c["recovery_policy"]["max_correction_cycles"], "correction_cycles_used": 0, "events": []},
      "timings": {"external_queue_ms": None, "worktree_prepare_ms": 0, "model_execution_ms": model_ms, "runner_validation_ms": 0, "environment_wait_ms": 0, "mentor_validation_ms": None, "total_ms": sum(r["duration_ms"] for r in records) + model_ms},
      "cleanup": {"worktree_clean": True, "temporary_paths_removed": True}}
    pathlib.Path(result_path).parent.mkdir(parents=True, exist_ok=True)
    pathlib.Path(result_path).write_text(json.dumps(result, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(result, sort_keys=True))
finally:
    shutil.rmtree(scratch, ignore_errors=True)
PY
}

run_contract() {
  [[ $# -ge 1 && $# -le 2 ]] || die "run requires CONTRACT [RESULT_PATH]"
  local total_start
  total_start="$(now_ms)"
  local contract=$1 requested_result=${2:-}
  [[ -f ${contract} ]] || die "contract does not exist: ${contract}"
  validate_config >/dev/null
  validate_contract "${contract}" >/dev/null

  eval "$(python3 - "${contract}" <<'PY'
import json, pathlib, shlex, sys
c = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
for key, value in {
    "CONTRACT_PROFILE": c["profile"], "CONTRACT_PERMISSION": c["permission_profile"],
    "CONTRACT_ACTUAL_MODEL": c["model"]["actual"], "CONTRACT_MODEL_ADMISSION": c["model_admission_fingerprint"], "CONTRACT_ENVIRONMENT": c["environment_fingerprint"],
    "CONTRACT_BASE": c["base_sha"], "CONTRACT_WORKTREE": c["worktree"],
    "CONTRACT_RESULT": c["result_path"], "CONTRACT_TIMEOUT": c["timeout_seconds"],
}.items():
    print(f"{key}={shlex.quote(str(value))}")
PY
)"
  if [[ ${CONTRACT_PERMISSION} == test-executor ]]; then
    load_route fast test-executor
  else
    load_route "${CONTRACT_PROFILE}" "${CONTRACT_PERMISSION}"
  fi
  local current_model_admission
  current_model_admission="$(model_admission_fingerprint "${REQUESTED_MODEL}")"
  [[ ${current_model_admission} == "${CONTRACT_MODEL_ADMISSION}" ]] || die "contract model admission fingerprint is stale; rerun only this profile/permission preflight"
  local current_environment
  current_environment="$(fingerprint | python3 -c 'import json,sys; print(json.load(sys.stdin)["fingerprint_id"])')"
  [[ ${current_environment} == "${CONTRACT_ENVIRONMENT}" ]] || die "contract environment fingerprint is stale; rerun environment admission"
  if [[ ${CONTRACT_PERMISSION} == test-executor ]]; then
    run_managed_test_executor "${contract}" "${requested_result:-${CONTRACT_RESULT}}" "${current_environment}" "${current_model_admission}"
    return
  fi
  [[ ${PERMISSION_NAME} != release ]] || die "release execution requires a Delivery-managed executor"
  [[ -d ${CONTRACT_WORKTREE} ]] || die "runner only accepts an existing isolated worktree"
  [[ "$(git -C "${CONTRACT_WORKTREE}" rev-parse HEAD)" == "${CONTRACT_BASE}" ]] || die "worktree HEAD does not match contract base SHA"
  [[ "$(git -C "${CONTRACT_WORKTREE}" rev-parse --show-toplevel)" == "${CONTRACT_WORKTREE}" ]] || die "contract worktree path is not its Git root"
  local initial_status
  initial_status="$(git -C "${CONTRACT_WORKTREE}" status --porcelain=v1 --untracked-files=all)"
  if [[ -n ${initial_status} ]]; then
    printf '%s\n' "${initial_status}" >&2
    die "contract worktree must be clean before Worker execution"
  fi
  require_command "${EXECUTOR}"
  require_command timeout

  local run_id="$(date -u +%Y%m%dT%H%M%SZ)-${PROFILE_NAME}-$$"
  local evidence_dir="${evidence_root}/${run_id}"
  local prompt_file="${evidence_dir}/contract-prompt.txt"
  local raw_file="${evidence_dir}/raw-output.jsonl"
  local fingerprint_file="${evidence_dir}/fingerprint.json"
  local identity_file="${evidence_dir}/model-identity.json"
  mkdir -p -- "${evidence_dir}" "$(dirname -- "${requested_result:-${CONTRACT_RESULT}}")"
  fingerprint >"${fingerprint_file}"
  python3 - "${contract}" >"${prompt_file}" <<'PY'
import json, pathlib, sys
c = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
print("Execute this bounded contract exactly. Do not create worktrees, switch branches, acquire credentials, expand scope, change frozen expected/Oracle/tolerance, or bypass permissions.")
print(f"You have {c['recovery_policy']['max_correction_cycles']} in-contract correction cycle(s). A cycle is failure -> diagnosis -> allowlisted adjustment -> rerun. Do not blindly retry an unchanged command or candidate. Record each used cycle under recovery.events.")
print("Do not stage, commit, or write Git metadata. For a successful write task, leave the verified allowlisted working-tree diff in place. For a recoverable environment/evidence/command blocker, preserve the allowlisted candidate for blocked-with-candidate handoff; restore the exact base only for unsafe or out-of-contract state.")
print("Actually execute every applicable RED, GREEN, and regression command. Return one JSON object with status, commands including argv/cwd/exit_code/duration_ms, tests passed/failed/skipped/total, blockers, summary, escalation request, recovery {correction_budget, correction_cycles_used, events}, and cleanup evidence.")
print("If an immediate stop/escalation condition occurs, stop without consuming a correction cycle and return blocked. The Worker result is only a candidate for Mentor verdict.")
print(json.dumps(c, ensure_ascii=False, indent=2, sort_keys=True))
PY
  local model_exit
  local model_start model_end model_execution_ms validation_start
  model_start="$(now_ms)"
  model_exit="$(invoke_model "${CONTRACT_WORKTREE}" "${prompt_file}" "${raw_file}" "${CONTRACT_TIMEOUT}")"
  model_end="$(now_ms)"
  model_execution_ms=$((model_end - model_start))
  validation_start="$(now_ms)"
  model_identity "${raw_file}" "${identity_file}"

  local result_target=${requested_result:-${CONTRACT_RESULT}}
  python3 - "${contract}" "${profiles_file}" "${raw_file}" "${identity_file}" "${fingerprint_file}" \
    "${EXECUTOR}" "${REQUESTED_MODEL}" "${model_exit}" "${CONTRACT_WORKTREE}" "${result_target}" \
    "${current_model_admission}" "${model_execution_ms}" "${validation_start}" "${total_start}" <<'PY'
import fnmatch
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import time

(contract_path, profiles_path, raw_path, identity_path, fingerprint_path, executor,
 requested, model_exit, worktree, result_target, current_model_admission, model_execution_ms,
 validation_start, total_start) = sys.argv[1:]
c = json.loads(pathlib.Path(contract_path).read_text(encoding="utf-8"))
identity = json.loads(pathlib.Path(identity_path).read_text(encoding="utf-8"))
fingerprint = json.loads(pathlib.Path(fingerprint_path).read_text(encoding="utf-8"))

def git_bytes(*args, env=None):
    return subprocess.run(
        ["git", "-C", worktree, *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        check=True, env=env,
    ).stdout

status_output = git_bytes("status", "--porcelain=v1", "-z", "--untracked-files=all")
committed_output = git_bytes("diff", "--name-only", "-z", f'{c["base_sha"]}..HEAD')
changed_set = {value.decode("utf-8", errors="surrogateescape") for value in committed_output.split(b"\0") if value}
records = status_output.split(b"\0")
index = 0
while index < len(records):
    record = records[index]
    if not record:
        index += 1
        continue
    code = record[:2].decode("ascii", errors="replace")
    changed_set.add(record[3:].decode("utf-8", errors="surrogateescape"))
    index += 2 if "R" in code or "C" in code else 1
changed = sorted(changed_set)
violations = [path for path in changed if not any(fnmatch.fnmatch(path, pattern) for pattern in c["allowed_paths"]) or any(fnmatch.fnmatch(path, pattern) for pattern in c["forbidden_paths"])]
candidate = git_bytes("rev-parse", "HEAD").decode().strip()
candidate_tree = git_bytes("rev-parse", "HEAD^{tree}").decode().strip()
candidate_state = "base-clean"
candidate_diff_sha256 = None
candidate_patch_path = None
candidate_manifest_path = None

evidence_directory = pathlib.Path(raw_path).parent
if status_output:
    candidate_state = "uncommitted-diff"
    scratch = evidence_directory / "candidate-index"
    shutil.rmtree(scratch, ignore_errors=True)
    object_directory = scratch / "objects"
    (object_directory / "info").mkdir(parents=True)
    common_raw = git_bytes("rev-parse", "--git-common-dir").decode().strip()
    common_directory = pathlib.Path(common_raw)
    if not common_directory.is_absolute():
        common_directory = (pathlib.Path(worktree) / common_directory).resolve()
    main_objects = (common_directory / "objects").resolve()
    (object_directory / "info" / "alternates").write_text(str(main_objects) + "\n", encoding="utf-8")
    alternate_env = os.environ.copy()
    alternate_env.update({
        "GIT_INDEX_FILE": str(scratch / "index"),
        "GIT_OBJECT_DIRECTORY": str(object_directory),
        "GIT_ALTERNATE_OBJECT_DIRECTORIES": str(main_objects),
    })
    git_bytes("read-tree", c["base_sha"], env=alternate_env)
    git_bytes("add", "-A", "--", *changed, env=alternate_env)
    candidate_tree = git_bytes("write-tree", env=alternate_env).decode().strip()
    patch_bytes = git_bytes("diff", "--cached", "--binary", "--full-index", c["base_sha"], "--", env=alternate_env)
    candidate_patch_path = evidence_directory / "candidate.patch"
    candidate_patch_path.write_bytes(patch_bytes)
    candidate_diff_sha256 = hashlib.sha256(patch_bytes).hexdigest()
    shutil.rmtree(scratch, ignore_errors=True)
elif candidate != c["base_sha"]:
    candidate_state = "committed"
    patch_bytes = git_bytes("diff", "--binary", "--full-index", f'{c["base_sha"]}..HEAD', "--")
    candidate_patch_path = evidence_directory / "candidate.patch"
    candidate_patch_path.write_bytes(patch_bytes)
    candidate_diff_sha256 = hashlib.sha256(patch_bytes).hexdigest()

def digest(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()

def extract_final_object():
    text = pathlib.Path(raw_path).read_text(encoding="utf-8", errors="replace")
    messages = []
    if executor == "claude":
        try:
            root = json.loads(text)
            if isinstance(root, dict) and isinstance(root.get("result"), str):
                messages.append(root["result"])
        except json.JSONDecodeError:
            pass
    else:
        for line in text.splitlines():
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            item = event.get("item") if isinstance(event, dict) else None
            if isinstance(item, dict) and item.get("type") == "agent_message" and isinstance(item.get("text"), str):
                messages.append(item["text"])
    decoder = json.JSONDecoder()
    for message in reversed(messages):
        stripped = message.strip()
        if stripped.startswith("```json") and stripped.endswith("```"):
            stripped = stripped[7:-3].strip()
        try:
            value = json.loads(stripped)
            if isinstance(value, dict):
                return value
        except json.JSONDecodeError:
            for index, character in enumerate(stripped):
                if character != "{":
                    continue
                try:
                    value, _ = decoder.raw_decode(stripped[index:])
                except json.JSONDecodeError:
                    continue
                if isinstance(value, dict):
                    return value
    return None

worker = extract_final_object()
structured_blockers = []
commands = []
tests = {"passed": 0, "failed": 1, "skipped": 0, "total": 1}
worker_status = "failed"
worker_summary = "Worker did not return parseable structured command/test evidence."
worker_escalated = False
worker_escalation_reason = None
temporary_paths_removed = False
recovery = {"correction_budget": c["recovery_policy"]["max_correction_cycles"], "correction_cycles_used": 0, "events": []}
if worker is None:
    structured_blockers.append("missing structured Worker result")
else:
    worker_status = worker.get("status", "failed")
    worker_summary = str(worker.get("summary", ""))
    worker_escalated = bool(worker.get("escalated", False))
    worker_escalation_reason = worker.get("escalation_reason")
    structured_blockers.extend(str(value) for value in worker.get("blockers", []) if str(value))
    raw_commands = worker.get("commands", [])
    for command in raw_commands if isinstance(raw_commands, list) else []:
        if not isinstance(command, dict):
            continue
        kind = command.get("kind")
        argv = command.get("argv")
        cwd = command.get("cwd")
        exit_code = command.get("exit_code")
        duration_ms = command.get("duration_ms")
        expected_tests = command.get("expected_tests", 0)
        if kind in {"red", "green", "regression", "worker"} and isinstance(argv, list) and all(isinstance(value, str) for value in argv) and isinstance(cwd, str) and cwd.startswith("/") and isinstance(exit_code, int) and isinstance(duration_ms, int) and duration_ms >= 0:
            commands.append({"kind": kind, "argv": argv, "cwd": cwd, "exit_code": exit_code, "duration_ms": duration_ms, "expected_tests": expected_tests})
    raw_tests = worker.get("tests")
    if isinstance(raw_tests, dict) and all(isinstance(raw_tests.get(key), int) and raw_tests[key] >= 0 for key in ("passed", "failed", "skipped", "total")):
        tests = {key: raw_tests[key] for key in ("passed", "failed", "skipped", "total")}
    cleanup = worker.get("cleanup")
    if isinstance(cleanup, dict):
        temporary_paths_removed = cleanup.get("temporary_paths_removed") is True
    raw_recovery = worker.get("recovery")
    if isinstance(raw_recovery, dict):
        budget = raw_recovery.get("correction_budget")
        used = raw_recovery.get("correction_cycles_used")
        events = raw_recovery.get("events")
        valid_events = isinstance(events, list) and all(
            isinstance(event, dict)
            and set(event) == {"cycle", "classification", "adjustment", "outcome"}
            and event.get("cycle") == index
            and event.get("classification") in {"mechanical", "implementation", "environment", "evidence"}
            and isinstance(event.get("adjustment"), str) and bool(event["adjustment"])
            and event.get("outcome") in {"recovered", "blocked", "escalated"}
            for index, event in enumerate(events, start=1)
        )
        if budget == recovery["correction_budget"] and isinstance(used, int) and 0 <= used <= budget and valid_events and len(events) == used:
            recovery = {"correction_budget": budget, "correction_cycles_used": used, "events": events}
        else:
            structured_blockers.append("Worker result has invalid recovery evidence")
    else:
        structured_blockers.append("Worker result omitted recovery evidence")

required_kinds = {kind for kind in ("red", "green", "regression") if c["commands"].get(kind)}
observed_kinds = {command["kind"] for command in commands}
if not required_kinds.issubset(observed_kinds):
    structured_blockers.append("Worker result omitted required RED/GREEN/regression command evidence")
if tests["total"] != tests["passed"] + tests["failed"] + tests["skipped"] or tests["total"] == 0:
    structured_blockers.append("Worker result has invalid or empty test counts")

unsafe_blockers = ["changed path outside contract: " + path for path in violations]
operational_blockers = []
if int(model_exit) != 0:
    operational_blockers.append("worker CLI exited non-zero")
if candidate != c["base_sha"]:
    unsafe_blockers.append("Worker wrote Git metadata; candidate integration is Orchestrator-owned")
if status_output and c["permission_profile"] in {"test-executor", "quality-review"}:
    unsafe_blockers.append("read-only Worker produced working-tree changes")
if identity["actual_model"] == "unverified":
    unsafe_blockers.append("actual model identity is unverified")
if identity["actual_model"] != c["model"]["actual"]:
    unsafe_blockers.append("runtime actual model differs from the admitted contract model")
all_blockers = unsafe_blockers + operational_blockers + structured_blockers
final_status = worker_status if worker_status in {"ready", "blocked", "failed"} else "failed"
if all_blockers:
    final_status = "failed" if unsafe_blockers else "blocked"
if final_status == "ready" and candidate_state == "uncommitted-diff":
    candidate_state = "verified-diff"
elif final_status in {"blocked", "failed"} and candidate_state == "uncommitted-diff" and not unsafe_blockers:
    final_status = "blocked"
    candidate_state = "blocked-with-candidate"

candidate_manifest_path = evidence_directory / "candidate-manifest.json"
candidate_manifest_path.write_text(json.dumps({
    "schema": "ficant.worker-candidate.v1",
    "state": candidate_state,
    "base_sha": c["base_sha"],
    "head_sha": candidate,
    "candidate_tree": candidate_tree,
    "candidate_diff_sha256": candidate_diff_sha256,
    "changed_files": changed,
    "allowed_paths": c["allowed_paths"],
    "forbidden_paths": c["forbidden_paths"],
    "violations": violations,
}, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")

evidence = [{"path": raw_path, "sha256": digest(raw_path)}, {"path": fingerprint_path, "sha256": digest(fingerprint_path)}, {"path": str(candidate_manifest_path), "sha256": digest(candidate_manifest_path)}]
if candidate_patch_path is not None:
    evidence.append({"path": str(candidate_patch_path), "sha256": digest(candidate_patch_path)})

now = time.monotonic_ns() // 1_000_000
runner_validation_ms = max(0, now - int(validation_start))
total_ms = max(0, now - int(total_start))

payload = {
    "schema_version": 4,
    "status": final_status,
    "checklist_id": c["checklist_id"], "task_id": c["task_id"], "profile": c["profile"],
    "executor": executor, "command_executor": "worker-direct", "requested_model": requested,
    "actual_model": identity["actual_model"], "model_identity_source": identity["model_identity_source"], "spark_brief": None,
    "permission_profile": c["permission_profile"], "environment": "wsl",
    "environment_fingerprint": fingerprint["fingerprint_id"], "base_sha": c["base_sha"],
    "model_admission_fingerprint": current_model_admission,
    "candidate_sha": candidate, "candidate_state": candidate_state,
    "candidate_tree": candidate_tree, "candidate_diff_sha256": candidate_diff_sha256,
    "changed_files": changed,
    "commands": commands or [{"kind": "worker", "argv": [executor, "non-interactive", "explicit-model", c["permission_profile"]], "cwd": worktree, "exit_code": int(model_exit), "duration_ms": int(model_execution_ms), "expected_tests": 0}],
    "tests": tests,
    "evidence": evidence,
    "escalated": worker_escalated, "escalation_reason": worker_escalation_reason,
    "blockers": all_blockers,
    "summary": worker_summary + " Worker claims remain candidate evidence pending Mentor verdict.",
    "recovery": recovery,
    "timings": {
        "external_queue_ms": None,
        "worktree_prepare_ms": 0,
        "model_execution_ms": int(model_execution_ms),
        "runner_validation_ms": runner_validation_ms,
        "environment_wait_ms": 0,
        "mentor_validation_ms": None,
        "total_ms": total_ms,
    },
    "cleanup": {"worktree_clean": not bool(status_output), "temporary_paths_removed": temporary_paths_removed},
}
pathlib.Path(result_target).write_text(json.dumps(payload, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
print(json.dumps(payload, ensure_ascii=False, sort_keys=True))
PY
  validate_result "${result_target}" >/dev/null
  [[ "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["status"])' "${result_target}")" == ready ]]
}

case ${1:-} in
  validate-config)
    [[ $# -eq 1 ]] || die "validate-config takes no arguments"
    validate_config
    ;;
  fingerprint)
    [[ $# -eq 1 ]] || die "fingerprint takes no arguments"
    fingerprint
    ;;
  prepare-caches)
    [[ $# -eq 1 ]] || die "prepare-caches takes no arguments"
    prepare_caches
    ;;
  prepare-worktree)
    shift
    prepare_worktree "$@"
    ;;
  integrate-candidate)
    shift
    integrate_candidate "$@"
    ;;
  toolchain-preflight)
    [[ $# -eq 1 ]] || die "toolchain-preflight takes no arguments"
    toolchain_preflight
    ;;
  container-preflight)
    [[ $# -eq 1 ]] || die "container-preflight takes no arguments"
    container_preflight
    ;;
  validate-contract)
    [[ $# -eq 2 ]] || die "validate-contract requires CONTRACT"
    validate_contract "$2"
    ;;
  validate-result)
    [[ $# -eq 2 ]] || die "validate-result requires RESULT"
    validate_result "$2"
    ;;
  preflight)
    shift
    preflight "$@"
    ;;
  admission-fingerprint)
    [[ $# -eq 4 ]] || die "admission-fingerprint requires PROFILE PERMISSION ACTUAL_MODEL"
    load_route "$2" "$3"
    model_admission_fingerprint "$4"
    ;;
  run)
    shift
    run_contract "$@"
    ;;
  *)
    die "usage: run.sh validate-config | validate-contract CONTRACT | validate-result RESULT | fingerprint | prepare-caches | prepare-worktree WORKTREE_PATH BRANCH_NAME BASE_SHA | integrate-candidate WORKTREE_PATH RESULT_PATH COMMIT_MESSAGE | toolchain-preflight | container-preflight | preflight PROFILE PERMISSION | run CONTRACT [RESULT_PATH]"
    ;;
esac
