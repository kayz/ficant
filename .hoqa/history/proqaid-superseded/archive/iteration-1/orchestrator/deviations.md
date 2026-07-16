# Orchestrator Deviations

## iteration-1

### DEV-001 — Per-agent model selection unavailable

- **Requested:** Orchestrator and Review use GPT-5.6 Sol/high; other standing roles and workers use GPT-5.6 Terra/high.
- **Observed capability:** The available agent dispatch API accepts task name, prompt, and context fork only; it exposes no model or reasoning-strength selector and returns no model attestation.
- **Treatment:** Preserve the requested policy in tool constraints and dispatch prompts; mark each actual runtime model as unverified. After the round-4 Review capacity failure, the human authorized retrying one adjacent model tier lower or higher when a target is unavailable, at capacity, or fails allocation.
- **Scope impact:** None to the governance documents. Model enforcement must be revisited when the runtime provides selection or attestation.
- **Human status:** User authorized adjacent-tier fallback on 2026-07-11; no claim of a specific fallback is allowed without runtime attestation.

### DEV-002 — Previous auxiliary design path removed

- **Observed:** The prior tracked `docs/superpowers/specs/2026-07-11-proqaid-initialization-design.md` was deleted by the user.
- **Treatment:** Preserve the deletion and replace its durable intent with PROQAID-owned records.
- **Scope impact:** None; prevents document ownership pollution.

### DEV-003 — UI-DM contract terminology

- **Observed:** UI-DM names OpenAPI-generated types and SSE events, while README fixes Protobuf as the sole cross-boundary contract and gRPC-Web as the browser API.
- **Treatment:** Preserve UI names only as provisional interface aliases; require Phase 0 canonical Protobuf mappings and do not authorize a parallel REST/OpenAPI contract.
- **Scope impact:** No iteration-1 scope change. Blocks later DMQuant implementation until contracts are frozen.

### DEV-004 — DMQuant roadmap placement unresolved

- **Observed:** The inputs do not establish whether DMQuant is a presentation of Rates Research Lab/CGB Futures Lab, a shared experience across them, or a separate named product stage.
- **Treatment:** Record as a human decision required before the first DMQuant engineering checklist.
- **Scope impact:** No governance-initialization blocker; no implementation worker is authorized now.

### DEV-005 — Initial Git evidence gap

- **Observed:** `README.md`, `UI-DM/`, and nearly all initialization artifacts are untracked; existing history cannot prove their pre-initialization state or exact delta.
- **Treatment:** Record the exact current inventory and qualify all change-history claims. Ask the human whether to establish the current snapshot as the initial tracked baseline.
- **Scope impact:** Documentation can be audited for present consistency, but iteration exit cannot claim Git-proven provenance until the human chooses a baseline action.
- **Human status:** Resolved for forward provenance: the human authorized a clean allowlisted initial baseline and push. Historical pre-state remains unproven and is not claimed.

## Validity

Valid: iteration-1 only
