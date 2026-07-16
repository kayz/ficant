# Orchestration Telemetry Overlay

Use this reference only when telemetry is explicitly enabled or its output is
being analyzed. The current checklist remains the sole iteration state and evidence
index.

## Activation and Ownership

Enable telemetry for `RECOVER`, an explicit efficiency investigation, or a
multi-Worker execution wave whose routing and recovery need observation. Do not
make it a universal iteration ceremony.

The Orchestrator owns a stable append-only logger. Prefer automatic emission at
runner boundaries over model-composed logging commands. Store the untracked log at:

```text
.agents/logs/{iteration}-orchestration-events.jsonl
```

Read it only when the human requests analysis or the checklist names a temporary
diagnostic window. Log availability or write failure never changes a gate, verdict,
or candidate status.

## Event Envelope

Use one JSON object per line with normalized enum values and an ISO-8601 timestamp:

```text
schema
timestamp
iteration_id
task_id
attempt_id
event
actor
role_or_perspective
worker_id
requested_model
actual_model
base_sha
candidate_sha
execution_status
evidence_status
recovery_cycle
reason_code
elapsed_ms
evidence_ref
```

Keep `execution_status` separate from `evidence_status`: a valid candidate with a
missing report is not an execution failure. Use consistent actor, role, event,
status, and reason enums so elapsed time can be derived without interpreting prose.

Record these boundary events when applicable:

- session or task start;
- main-model Product, Architecture, Interface, Delivery, or Orchestrator perspective switch;
- Worker start, report, escalation, and close;
- Mentor verdict;
- candidate integration or rejection;
- cleanup failure and diagnostic-window close.

## Observation Windows

| Window | Question | Minimum measures |
|---|---|---|
| Flow efficiency | Where does elapsed time go? | queue, execution, recovery, Mentor, integration |
| Worker effectiveness | Is the selected route suitable? | first-pass success, correction cycles, escalation, abandoned candidates |
| Verification quality | Is speed weakening evidence? | inventory/count deltas, failed/skipped, defect and retest counts |
| Scope and boundaries | Is complexity spreading? | out-of-scope paths, interface or expected/Oracle/tolerance changes, diff growth |
| Runner/environment | Is infrastructure consuming the iteration? | preflight/tool/process/cache failures and recovery time |
| Integration/cleanup | Can a candidate land cleanly? | SHA identity, conflicts, integration failures, residual worktrees/processes/data |
| Model/resources | Is routing economical and reliable? | requested/actual model, fallback reason, context/limit/cost signals |
| Delivery | Is operational lead time acceptable? | SIT/UAT deploy, data, health, rollback, and human-wait time |

Enable the last two windows only when model capacity/cost or Delivery phases are
actually in scope.

## Diagnostic Signals

Investigate when any of these appear:

- correction exceeds the task budget;
- Mentor time exceeds Worker execution time;
- the same runner or environment reason repeats;
- test inventory falls or skipped tests increase unexpectedly;
- a Worker changes paths or frozen semantics outside its contract;
- a fast route encounters a non-mechanical failure without escalation;
- a basic environment dependency is first discovered after execution begins;
- a completed candidate waits unusually long for verdict or integration;
- cleanup leaves a worktree, process, service, or mutable data instance behind.

Treat a single sample as a baseline, not proof of a trend. Compare like-sized task
classes and separate productive execution from queue, recovery, verification, and
human wait. Link to Worker results, test evidence, candidate manifests, and
environment evidence instead of duplicating them.

## Data Boundaries

Do not log prompts, chain-of-thought, credentials, credential paths, raw third-party
data, full diffs, or raw test output. Keep operational logs out of Git by default
and apply the project's retention policy. Analysis may produce a compact durable
decision only when it changes routing, recovery budgets, runner design, or another
governed boundary.
