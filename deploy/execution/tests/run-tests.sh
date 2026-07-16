#!/usr/bin/env bash
# HOQA status: tests for the superseded historical WSL compatibility runner only.
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)"
runner="${root}/deploy/execution/run.sh"
tmp="$(mktemp -d)"
state="${tmp}/state"
cleanup() { rm -rf -- "${tmp}"; }
trap cleanup EXIT

python3 - "${root}/deploy/execution/schemas/contract.schema.json" "${root}/deploy/execution/schemas/result.schema.json" <<'PY'
import json, pathlib, sys
c, r = (json.loads(pathlib.Path(p).read_text()) for p in sys.argv[1:])
assert c["properties"]["schema_version"]["const"] == 4
assert set(c["$defs"]["structuredCommand"]["properties"]) == {"argv", "cwd", "timeout_seconds", "expected_tests"}
assert r["properties"]["schema_version"]["const"] == 4
assert "command_executor" in r["required"] and "spark_brief" in r["required"]
PY

mkdir -p "${tmp}/bin"
printf '%s\n' '#!/usr/bin/env bash' 'if [[ ${1:-} == --version ]]; then printf "%s\n" "codex-test 1"; exit 0; fi' 'printf "%s\n" '\''{"status":"candidate","summary":"evidence consumed","observations":["runner authoritative"]}'\''' >"${tmp}/bin/codex"
chmod +x "${tmp}/bin/codex"
export PATH="${tmp}/bin:${PATH}"
export XDG_STATE_HOME="${state}"
export FICANT_EVIDENCE_ROOT="${state}/ficant/evidence"

environment_id="$(bash "${runner}" fingerprint | python3 -c 'import json,sys; print(json.load(sys.stdin)["fingerprint_id"])')"
admission_id="$(bash "${runner}" admission-fingerprint fast test-executor gpt-5.3-codex-spark)"
base="$(git -C "${root}" rev-parse HEAD)"
contract="${tmp}/contract.json"
result="${tmp}/result.json"
python3 - "${contract}" "${result}" "${root}" "${base}" "${environment_id}" "${admission_id}" <<'PY'
import json, pathlib, sys
contract, result, root, base, environment_id, admission_id = sys.argv[1:]
doc = {
 "schema_version":4,"checklist_id":"I3-NATIVE","task_id":"RUNNER-NATIVE-TEST",
 "ids":{"case":["RUNNER-TEST-EXECUTOR-SCRATCH","RUNNER-PERSISTENT-EVIDENCE","RUNNER-STRUCTURED-ARGV","RUNNER-SPARK-BRIEF"],"acceptance":["I3-ENV-OMISSION-003"],"defect":["I3-D-RUNNER-001","I3-D-RUNNER-002","I3-D-RUNNER-003"]},
 "profile":"strong","permission_profile":"test-executor",
 "model":{"requested":"gpt-5.3-codex-spark","actual":"gpt-5.3-codex-spark","actual_identity_required":True},
 "model_admission_fingerprint":admission_id,"environment_fingerprint":environment_id,
 "base_sha":base,"worktree":root,"allowed_paths":["deploy/execution/**"],"forbidden_paths":[".git/**"],
 "frozen_contracts":["test"],"expected_oracle":["test"],"context_files":["deploy/execution/run.sh"],
 "commands":{"red":[],"green":[{"argv":["bash","-c","printf isolated > managed-output.txt"],"cwd":"${SOURCE}","timeout_seconds":30,"expected_tests":7}],"regression":[{"argv":["bash","-n","deploy/execution/run.sh"],"cwd":"${SOURCE}","timeout_seconds":30,"expected_tests":0}]},
 "timeout_seconds":300,"result_path":result,"cleanup":["remove scratch"],
 "recovery_policy":{"max_correction_cycles":2,"preserve_candidate_on_recoverable_blocker":True,"immediate_stop_conditions":["boundary change"]},
 "mentor":"Orchestrator","escalation_conditions":["boundary change"],"fallback":"same Strong tier"}
pathlib.Path(contract).write_text(json.dumps(doc))
PY

before="$(git -C "${root}" status --porcelain=v1 --untracked-files=all)"
bash "${runner}" run "${contract}" "${result}" >/dev/null
bash "${runner}" validate-result "${result}" >/dev/null
after="$(git -C "${root}" status --porcelain=v1 --untracked-files=all)"
[[ ${before} == "${after}" && ! -e "${root}/managed-output.txt" ]]
python3 - "${result}" "${state}" <<'PY'
import hashlib, json, pathlib, sys
r=json.loads(pathlib.Path(sys.argv[1]).read_text())
assert r["status"] == "ready" and r["executor"] == "codex" and r["command_executor"] == "runner-managed"
assert r["actual_model"] == "gpt-5.3-codex-spark" and r["spark_brief"]["status"] == "candidate"
assert r["tests"] == {"passed":7,"failed":0,"skipped":0,"total":7}
assert [c["expected_tests"] for c in r["commands"]] == [7, 0]
for item in r["evidence"]:
 p=pathlib.Path(item["path"]); assert p.is_file() and hashlib.sha256(p.read_bytes()).hexdigest() == item["sha256"]
assert not any(pathlib.Path(sys.argv[2], "ficant/build").glob("*-test-executor-*"))
PY

# Stale identities fail before command or model execution.
python3 - "${contract}" <<'PY'
import json, pathlib, sys
p=pathlib.Path(sys.argv[1]); d=json.loads(p.read_text()); d["environment_fingerprint"]="sha256:"+"0"*64; p.write_text(json.dumps(d))
PY
if bash "${runner}" run "${contract}" "${tmp}/stale.json" >/dev/null 2>&1; then exit 1; fi
[[ ! -e "${tmp}/stale.json" ]]

printf '%s\n' 'PASS 18/18'
