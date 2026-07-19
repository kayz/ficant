# Architecture Review: iteration-1, round-1

## Review Result

Architecture baseline captured with important contract gaps. The design is coherent enough for iteration-1 governance completion, but it is not implementation-ready until Phase 0 freezes canonical Protobuf contracts and resolves the DMQuant contract vocabulary.

No production implementation, Protobuf schema, database schema, or running service was available to verify. Every technical topology and behavior described here remains design unless explicitly marked as an out-of-scope boundary.

## Evidence Reviewed

- `README.md` — sole system technology and architecture baseline.
- `UI-DM/DM设计补齐说明.md` — DMQuant target workflow, UI states, and API-shaped mappings.
- `iteration-1-checklist.md` — governance-only iteration contract.
- `.proqaid/orchestrator/current-iteration.md` — confirmed project state, objective, and constraints.
- `.proqaid/architecture/inbox.md` and `.proqaid/architecture/inbox.iteration-1.round-1.md` — role assignment and write boundary.

No other project input was used.

## Design Versus Implementation

| Area | Design conclusion | Implementation evidence |
|---|---|---|
| Backend | Rust modular monolith with isolated worker and sandbox processes | None |
| Browser API | Protobuf-derived gRPC-Web | None; DMQuant's OpenAPI wording conflicts with this design |
| Domain contracts | Protobuf is the sole cross-boundary source | No `.proto` schema exists |
| Storage | PostgreSQL 16 metadata/journal plus MinIO artifacts and snapshots | No schema, migration, bucket, or service exists |
| Research data | Arrow/Parquet immutable snapshots with point-in-time semantics | No adapter, manifest, or snapshot exists |
| Generated code | Python 3.12 in gVisor with typed I/O and no direct stores/network/secrets | No image, scheduler, or contract exists |
| Numerical kernel | C++20 behind stable C ABI and safe Rust wrappers | No library or ABI exists |
| DMQuant | Static React experience using shared platform identity and APIs | Static design input only; no runtime behavior verified |
| Task/backtest flow | Stream draft, save version, submit idempotently, poll task, read result/series/artifacts | Method and field names are proposed UI mappings only |

## Boundary Findings

- The platform boundary is explicit: research and signal publication stop at `SignalSet` and `TargetExposure`; order management and execution remain downstream.
- WebApps cannot redefine platform facts, directly access data stores, bypass snapshots/graphs, or carry a separate backend.
- Rust owns control-plane state and validation. C++ is numerical-only. Python is research-node-only. TypeScript is experience-only.
- Generated match logic proposes candidate outcomes; only deterministic Rust simulation may create formal fill facts.
- A modular monolith is the intended first deployment. Introducing microservices or a parallel queue/database/API is outside the baseline absent an ADR.

## Domain and Data Findings

- The Definition / Run / Artifact triad gives a usable lifecycle foundation, but individual aggregate boundaries and identifiers still need schema-level decisions.
- Immutable `DataSnapshot`, `UniverseSnapshot`, versioned rule packs, graph versions, capability versions, runtime/environment digests, and the execution identity must converge into one formal lineage contract.
- DMQuant's terms `strategy`, `version`, `run`, `task`, `file`, and `series` need explicit mappings to canonical objects. UI convenience labels must not become parallel domain types.
- The main data path is directionally sound: external input is normalized and quality-checked before snapshotting; all formal execution uses the snapshot; results and evidence are immutable and content-addressed.

## Dependency Findings

- The planned dependency direction keeps domain logic independent of frameworks and infrastructure.
- The listed `infrastructure → domain` dependency requires domain-owned ports or traits so storage concerns do not leak into domain objects.
- `ai → application` and `sandbox → runtime` preserve orchestration authority, but generated code input/output must remain constrained by domain-owned Protobuf contracts.
- The C ABI and Arrow IPC boundaries require ownership, memory, error, precision, versioning, and compatibility tests before they become stable interfaces.

## Contract Findings

### Important

1. Resolve the Protobuf-versus-OpenAPI conflict before any DMQuant client generation. Protobuf remains authoritative under the current baseline.
2. Freeze the minimum Phase 0 Protobuf packages and compatibility rules for identity/auth, core metadata, strategy/version, experiment/run, task, error, artifact, AI generation stream, and series/result projections.
3. Define idempotency, task, cache, error, authorization, fingerprint, source-artifact, deletion/deprecation, and audit semantics before UI/API implementation.
4. Create `docs/architecture/data-dictionary.md` as the durable mapping between canonical platform terms and DMQuant aliases, explicitly labeling all iteration-1 content as design.

### Notes

- The requested architecture artifact boundary is appropriately narrow: Markdown only for the current data dictionary.
- No architecture concern requires a production worker in iteration-1 because this iteration explicitly excludes engineering implementation.
- The target role model policy is GPT-5.6 Terra with high reasoning; actual model application is unverified in this runtime.

## Recommended Phase 0 Contract Slice

The first contract slice should be small enough to prove generation and browser interoperability without pretending to complete all domain packs:

1. common identifiers, version references, timestamps, Decimal/unit values, tenant/actor context, error details, and trace correlation;
2. strategy definition/version plus immutable source artifact reference;
3. experiment submission, reproducibility inputs, task state, submit acknowledgment, and terminal run/result reference;
4. AI generation server stream with ordered typed events and terminal error/done semantics;
5. metrics, check report, fingerprint, NAV/signals/trades series, and artifact metadata projections;
6. service authorization rules and TypeScript/Rust/Python generation checks from the same Protobuf source.

## Validity

Valid: iteration-1 only
