---
name: hoqa
description: Use when a software project needs explicit human-model work division, parallel implementation workers, coordinated automated testing with test workers, release preparation with a Human Operator, or a final document-to-intent-and-code consistency audit.
---

# HOQA

## Core Principle

Organize work around four participants: Human, Orchestrator, Quality, and Audit.
Activate a participant only when its work is needed. Roles reduce uncertainty;
they do not create ceremony.

Treat the human as a visible project partner. Autonomy means batching communication
and completing model-suitable work, not hiding work from the human or attempting
tasks the model cannot observe or control reliably.

Do not use HOQA for an ordinary small implementation task that already has a clear
scope and acceptance. Execute that task directly.

## Four Participants

| Participant | Activate when | Owns | Does not own |
|---|---|---|---|
| Human | project start, material decisions, or human-suitable operations | intent, acceptance, business choices, authority, privileged and low-observability work | routine model execution |
| Orchestrator | always | plan, professional decisions, Development Worker routing, integration, deterministic validation, delivery | manufacturing approval steps |
| Quality | automated test strategy or coordinated test execution is needed | framework selection, automation plan, Test Worker routing, test execution, test report, bug list | Development Workers, requirements, agent approval, release mechanics |
| Audit | final output documents are ready | read-only consistency review of documents against human intent, decisions, code, and evidence | agent management, redesign, editing, or directing fixes |

Audit may inspect code and evidence to verify document claims, but it does not review
agent performance or manage remediation. Return findings to the Orchestrator and
stop. Audit again only after a new final document set is ready.

## Orchestrator Lenses

Product, Architecture, and Interface are professional lenses used by the
Orchestrator, not participants, agents, handoffs, or gates:

- **Product**: user value, scope, concepts, claims, and acceptance.
- **Architecture**: modules, data, dependencies, and contracts.
- **Interface**: UI, API, CLI, flows, and states.

Delivery is also Orchestrator work: prepare artifacts, release scenarios, rollback,
and Human Operator instructions, then integrate the resulting evidence.

## Workers Are Parallel Execution Slots

Use Workers only when two or more bounded tasks can progress independently.
Orchestrator routes Development Workers for different implementation modules.
Quality routes Test Workers to write automation scripts or execute disjoint test
inventories under the selected framework and test plan.

Give each Worker one task, exact base, allowed paths, frozen contract, self-test,
result shape, and cleanup rule. A Development Worker returns code and self-test
evidence to the Orchestrator. A Test Worker returns scripts, execution evidence,
and observed defects to Quality. Neither changes scope or acceptance, invokes a
governance participant, approves another Worker, or coordinates the project.

The Orchestrator integrates implementation results and routes defect fixes. Quality
consolidates automated test results and the bug list. Do not start Workers when
coordination cost exceeds the expected speedup.

Read `references/contracts.md` when preparing a Human Operator package, Worker
contract, Quality request, or Audit request.

## Flow

1. **Align**: hold one concise exchange with the human about outcome, acceptance,
   non-goals, model work, human work, and the next human checkpoint. Batch foreseeable
   questions and actions.
2. **Decide**: use Product, Architecture, and Interface lenses to resolve and freeze
   the boundaries needed for execution. Do not invoke Quality or Audit here.
3. **Execute**: work directly or start only useful parallel Workers. Each modifying
   executor self-tests before returning.
4. **Test**: combine implementation results and run deterministic checks. When the
   Quality activation condition applies, Quality selects the automation framework,
   defines the test inventory and Oracles, dispatches bounded Test Workers, executes
   or consolidates the tests, and submits a test report and bug list. Orchestrator
   routes fixes; Quality reruns the affected inventory.
5. **Operate**: for a complex release, validate the release scenario without target
   mutation, give the Human Operator one preparation package, then deploy and run
   UAT after the human returns the agreed evidence.
6. **Close**: submit the final output documents, human intent sources, candidate, and
   evidence to Audit. Audit reports any document-to-intent/code inconsistency and a
   final consistency verdict; Orchestrator owns all resulting work.

Use a compact state note only for multi-turn or formal work. Keep outcome, frozen
acceptance, human/model division, active Workers, current candidate, bug list,
blockers, and final evidence. Do not create role diaries, handoff packets, or
attempt histories.

## Change and Recovery

Keep mechanical implementation, build, runner, environment, and packaging failures
inside their current executor. Fix them against the frozen standard and rerun the
affected evidence. If Quality is active, rerun only the affected automated inventory.
Do not rewrite acceptance, change the test framework without a test-design reason,
invoke Audit, or create a new governance operation for a mechanical failure.

When scope, authority, acceptance, irreversible action, environment ownership, or
risk acceptance materially changes, batch one human reconfirmation and update the
compact state note. Add a recovery budget or ledger only when a named high-risk
operation explicitly requires one.

## Quick Routing

| Situation | Active participants |
|---|---|
| Clear small task | Human + Orchestrator; usually no HOQA artifact |
| Independent implementation modules | Orchestrator + bounded Development Workers |
| Non-trivial automated test design or execution | Orchestrator + Quality + bounded Test Workers |
| Complex target configuration | Human + Orchestrator |
| Final document consistency check | Human + Orchestrator + Audit |

## Red Flags

- Quality is reviewing Development Workers instead of leading automated tests.
- Quality is managing project work outside the bounded test effort.
- Audit is reviewing agent behavior or directing remediation.
- Audit is present before the final document set is ready.
- The human learns about required environment work only after execution is blocked.
- Acceptance is changed to make a failing runner pass.
- Governance artifacts are larger than the work they coordinate.

When a red flag appears, collapse execution back to Human + Orchestrator, restore the
frozen standard, and activate another participant only through its explicit trigger.
