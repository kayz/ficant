# HOQA Contracts

Load only the contract needed for the current participant or Worker.

## Initial Human Alignment

Use one concise exchange:

```text
outcome and acceptance
non-goals and irreversible actions
model-owned work
human-owned actions or decisions
inputs, access locations, and environment preparation
safety and rollback constraints
success evidence
next human checkpoint, if any
```

Ask for secret locations or injection methods, never secret values. Ask again only
when a material change or previously unagreed human action appears.

## Human Operator Package

Provide one current-phase package:

```text
purpose and timing
target and bounded scope
exact human action or decision
copyable command or configuration when safe
security constraints and rollback
success condition and minimum evidence
alternative if the action cannot be completed
feedback required to continue
```

Before requesting action, complete the analysis, scenario validation, command
preparation, and rollback design the model can perform reliably. Reuse unchanged
human evidence; do not repeatedly recheck or re-request it.

## Worker Contract

```text
task ID and exact base
executor and isolated workspace
allowed and excluded paths
frozen contracts and forbidden changes
required implementation or test work
self-test and risk-based regression
result fields and evidence
timeout, escalation, and cleanup
```

Development Workers report code and self-test evidence to Orchestrator. Test Workers
report test scripts, execution evidence, and observed defects to Quality. All Workers
report `ready`, `blocked`, or `failed`, with changed files, commands, counts, evidence,
blockers, and cleanup. Worker prose cannot change deterministic facts or provide a
Quality/Audit verdict.

## Quality Test Lead Contract

Quality receives a frozen testing objective and owns the automated test effort:

```text
frozen acceptance and candidate/system boundary
test risks, required test types, and environment constraints
existing test assets and allowed test paths
framework choice and concise rationale
automation inventory, Oracles, fixtures, and tolerances
bounded Test Worker assignments
script changes and execution commands
actual counts, evidence, and test report
bug list and retest status
```

Each bug-list item contains ID, severity, candidate, expected behavior, observed
behavior, minimal reproduction or command, evidence location, and affected scope.
Quality submits the bug list to Orchestrator; Orchestrator decides implementation
routing and priority. Quality may direct Test Workers only inside the bounded test
effort. It does not supervise Development Workers, judge agent performance, change
requirements, or own build tooling, host configuration, credentials, packaging, or
deployment mechanics.

## Audit Request

Audit receives an exit-ready, read-only document package:

```text
human intent sources and frozen acceptance
final output documents and their intended audiences
final candidate identity and material decisions
code, public interfaces, configuration, and runtime behavior used as evidence
test, release, and Human Operator evidence used by document claims
requested consistency verdict and prioritized findings
```

Audit checks that documents reflect human intent, describe the implemented
architecture and interfaces accurately, give executable setup/operation guidance,
and make only claims supported by code and evidence. Each finding identifies the
document and location, claim, intent or code evidence, discrepancy, and severity.

Audit does not edit files, redesign the solution, assess agent performance, direct
Workers, assign remediation, or join recovery. It returns a consistency verdict and
findings to Orchestrator.
