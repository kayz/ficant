---
name: proqaid
description: Use when a software project needs iteration bootstrap, recovery from governance drift, a material scope or acceptance-gate change, or formal iteration closure.
---

# PROQAID

## Purpose

Govern delivery with seven stable responsibility areas while keeping routine work
lightweight. Roles are decision lenses, workers are temporary executors, and
deterministic evidence proves results.

Do not reload PROQAID for ordinary implementation or test execution inside an
unchanged `ACTIVE` checklist. Follow the existing checklist and runner directly.

## Entry Mode

Choose the narrowest applicable mode:

| Mode | Use |
|---|---|
| `BOOTSTRAP` | establish a new governed iteration |
| `RECOVER` | restore a stalled or contradictory iteration |
| `CHANGE` | revise material scope, authority, acceptance, environment, or risk |
| `CLOSE` | judge exit gates and close the iteration |

Read only the current checklist section and affected durable decisions. Do not
replay completed entry work, reload raw history, or recreate accepted design.

## Operating Level

Use one level selected by the human and recorded in the checklist:

- **Lite**: a fast prototype or feasibility loop. It produces prototype evidence,
  not production-release assurance.
- **Governed**: the normal formal iteration. It keeps complete applicable exit
  integrity without the former Full ceremony.

There is no Full level. Add a targeted risk overlay for numerical, security,
FFI/memory, transaction/recovery, migration, or another named high-risk boundary
instead of escalating the whole iteration. Read `references/operating-levels.md`
only when choosing or changing the level or overlay.

## Seven Roles

The seven roles do not imply seven agents:

| Role | Authority | Normal executor |
|---|---|---|
| Orchestrator | scope, routing, integration, closure | main model |
| Product | user value, concepts, claims | main model role switch |
| Architecture | modules, data, dependencies, contracts | main model role switch |
| Interface | UI, API, CLI flows and states | main model role switch |
| Delivery | SIT, release, migration, runtime operations | main model plus tools |
| Quality | business test contract and result judgment | independent bounded context |
| Review | Design Freeze and exit audit | independent read-only context |

Product, Architecture, Interface, and Delivery share the Orchestrator's working
context. Switch responsibility perspective without producing handoff packets,
duplicate summaries, inboxes, or standing role agents. Record only a durable
decision delta needed by a later reader.

Invoke Quality only to freeze a changed business test contract and to judge a
completed integrated test batch. Invoke Review only at Design Freeze and formal
exit, plus one targeted pass after an accepted high-risk boundary change.

## Authoritative Checklist

Keep one `iteration-N-checklist.md` as the current state and evidence index:

```text
DRAFT -> AWAITING_USER_CONFIRMATION -> ACTIVE -> CLOSED
```

Keep it concise:

- outcome, acceptance sentence, non-goals, level, and risk overlays;
- affected roles and durable frozen boundaries;
- worker/model routing and current execution wave;
- phase-specific prerequisites and blockers;
- latest candidate, gate status, compact evidence links, and accepted risks;
- one current summary per phase rather than an append-only attempt diary.

Store raw commands, logs, reports, traces, and retry history in the execution
system. Request renewed confirmation only for a material scope, authority,
irreversible action, acceptance, environment, exit, or risk-acceptance change.

## Orchestration Telemetry Overlay

Enable orchestration telemetry only when recovering a stalled iteration,
investigating execution efficiency, or coordinating enough Workers that routing
and recovery behavior need observation. Keep it off for routine small tasks.

Write append-only JSONL through a stable Orchestrator-owned logger under
`.agents/logs/{iteration}-orchestration-events.jsonl`. Record boundary events,
not reasoning transcripts: main-model perspective switches; Worker start, report,
escalation, and close; Mentor verdict; candidate integration; and cleanup failure.

Telemetry is operational evidence, never checklist state, acceptance evidence, a
Worker result, or a role verdict. Do not read or replay it during execution unless
the human requests analysis or a declared diagnostic window is active. Do not let
logging failure block delivery. Correlate it with durable evidence by iteration,
task, attempt, Worker, base, and candidate identifiers rather than copying prompts,
diffs, test data, credentials, or raw test output into the log.

Observe flow efficiency, Worker effectiveness, verification quality, scope and
boundary drift, runner/environment health, and integration/cleanup. Add model and
resource or Delivery/SIT/UAT windows only when those phases are active. Read
`references/orchestration-telemetry.md` when enabling telemetry, defining its
schema, or analyzing execution efficiency.

## Environment and Delivery

Treat environment control as phase-specific capability, not a universal entry
ceremony:

- **Development**: check only capabilities required by the current task. Prefer
  compatible ranges; exact pins are justified only by output, ABI, security, or
  reproducibility needs.
- **SIT**: enter only when the iteration's acceptance requires integrated services,
  persistence, deployment topology, or target-like behavior.
- **Release/UAT**: apply strict artifact, migration, rollback, credential, and
  target-environment controls.

Use one project-selected development OS and shell for the main model and ordinary
workers. Cross operating-system execution is an explicit compatibility gate, not
a default worker path.

Delivery owns SIT and release contracts and updates shared environment baselines
when an operational boundary changes. It does not manage each worker's cache,
scratch space, ordinary tool invocation, or routine development loop. A local
missing tool belongs to the worker/runner; a broken shared runner belongs to the
Orchestrator; privileged host work belongs to the Human Operator.

Read `references/execution-contracts.md` only when a worker contract, external
executor, SIT/release environment, or Human Operator action is being created or
changed.

## Worker Contract and Recovery

Development and Test Workers are temporary executors, not roles. Give each worker:

- one task/checklist/case/defect ID and exact base;
- isolated workspace and allowed/excluded paths;
- frozen contracts, expected values, Oracle references, and forbidden changes;
- required self-test commands and risk-based regression commands;
- timeout, result schema, evidence fields, recovery budget, escalation, and cleanup.

Do not require ceremonial RED/GREEN evidence. A pre-change reproduction is
optional when it helps diagnose a defect. Every modifying worker must run the
specified self-test after its change. The runner, not worker prose, records actual
commands, exit codes, test inventory, counts, and evidence.

Preserve a recoverable candidate while the same worker adjusts a bounded mechanical
error. Use the checklist budget; stop and escalate when scope or frozen semantics
must change, root cause becomes cross-module or high-risk, evidence is weakened,
or the budget is exhausted. A worker completion statement is only a candidate;
the corresponding Mentor gives the verdict.

## Minimal Flow

### Bootstrap

1. State one outcome and acceptance sentence; split only genuinely independent outcomes.
2. Select Lite or Governed and any risk overlay.
3. Mark affected roles; let the main model resolve affected P/A/I/D decisions in place.
4. Freeze changed boundaries; invoke Quality and Review only when Governed requires them.
5. Record only current-phase prerequisites, then activate the checklist.

### Active execution

Use the existing checklist and runner without re-invoking PROQAID. Workers modify,
self-test, and return structured evidence. The Orchestrator validates scope and
integrates; Quality judges completed business-test batches.

### Change or recovery

Load the current blocker and its affected boundary only. Preserve useful candidate
state, revise the smallest contract or decision, and resume from that checkpoint.

### Close

Close only when applicable acceptance, Quality, Delivery, Review, cleanup, and
risk gates pass or the human explicitly accepts a documented risk. Record the
final candidate and set the checklist to `CLOSED`.

## Common Failure Corrections

| Failure | Correction |
|---|---|
| Restarting governance on every turn | Resume from the current checklist phase |
| Passing data among main-model roles | Switch responsibility perspective in place |
| Defaulting every iteration to maximum ceremony | Use Governed plus targeted overlays |
| Blocking development on unused SIT/release tools | Check the capability only when its phase begins |
| Making Delivery the development-environment operator | Keep Delivery at SIT/release boundaries |
| Requiring RED/GREEN from every worker | Require executed self-test evidence |
| Resetting a useful candidate after a small error | Preserve it within the recovery budget |
| Expanding the checklist with attempt history | Replace the phase summary and link raw evidence |
