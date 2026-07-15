# PROQAID Execution Contracts

Read this file only when creating or changing a worker, external executor,
Human Operator, SIT, or release contract.

## Worker Contract

Use a compact core shared by Development Worker, Test Author, Test Executor, and
other approved executor profiles:

```text
task/checklist/case/defect ID
worker profile, executor, actual model, permission, OS and shell
exact base and isolated workspace
allowed and excluded paths
frozen contracts, expected/Oracle references, forbidden changes
required self-test or execution commands
risk-based regression commands
timeout and recovery budget
structured result and evidence fields
Mentor, escalation, integration and cleanup rules
```

Profile-specific requirements:

- **Development Worker**: implement the bounded change and run its self-test.
- **Test Author**: implement fixtures/tests/mappings against a frozen Quality contract
  and self-test the harness.
- **Test Executor**: modify no source; execute the frozen inventory and return actual
  counts and evidence.
- **Environment/SIT Executor**: operate only named project resources and return health,
  isolation, identity, and cleanup evidence.
- **Release Executor**: Delivery-only target access, artifact identity, deployment,
  health, rollback readiness, and credential isolation.

A defect reproduction before modification is optional. Do not require RED/GREEN
ceremony. Required commands must actually execute; runner records override prose.

## Result

Return a compact machine-readable result containing at least:

```json
{
  "status": "ready|blocked|failed",
  "base_sha": "...",
  "candidate_sha": "...",
  "changed_files": [],
  "commands": [],
  "tests": {"expected": 0, "passed": 0, "failed": 0},
  "evidence": [],
  "recovery_cycles": 0,
  "blockers": [],
  "cleanup": "...",
  "summary": "..."
}
```

Record actual model, OS/shell, permission, command/argv, cwd, start/end or duration,
exit code, inventory/counts, evidence digest/location, and escalation reason where
applicable. Worker prose cannot change deterministic facts or supply a Mentor verdict.

## Recovery

Keep the candidate and relevant failure evidence while the same worker performs
checklist-approved bounded correction. Stop and return to the Orchestrator when:

- a frozen boundary, expected value, Oracle, tolerance, or acceptance must change;
- the requested write scope expands;
- root cause is unknown across modules or enters a high-risk overlay;
- assertions or evidence would be weakened;
- the recovery budget is exhausted.

Environment or runner failures stay with the same execution tier. Repair the
environment or executor rather than changing models without a capability reason.

## Phase-Specific Environment Contract

### Development

- Use the project's declared primary OS and shell for the main model and workers.
- Check only tools needed by the current task.
- Prefer minimum/compatible versions; pin exactly only for a recorded output, ABI,
  security, or reproducibility reason.
- Keep worker scratch, caches, and ordinary dependency repair outside Delivery's
  per-task workflow.

### SIT

Create a SIT contract only when acceptance requires integrated persistence,
services, topology, deployment packaging, or target-like behavior. Record service
identity, data manifest, isolation namespace, health checks, acceptance commands,
and cleanup. An iteration that does not need SIT has no SIT admission work.

### Release/UAT

Delivery records artifact identity, target, configuration, migration, rollback,
health, credential handling, and release evidence. Ordinary workers do not receive
release credentials.

## Human Operator

Ask the human only for privileged host actions, accounts, secrets, organization
systems, or another action outside agent authority that blocks the current phase.
Batch known actions, provide a copyable command when safe, define success evidence,
and never request secret values. Do not front-load future SIT or release setup into
development admission.

## Evidence Retention

Keep raw logs, reports, screenshots, traces, and retry history in the execution
system. Put only candidate-bound summaries and durable artifact references in the
iteration checklist. Reuse unchanged evidence until its command, dependency,
fixture, toolchain, runtime, or candidate binding changes.
