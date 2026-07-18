# Interface Review — DMQuant iteration-1 baseline

## Review Status

The two UI-DM inputs cover the intended business loop well enough to form a durable interface proposal. They remain design evidence only. No production behavior, API availability, accessibility conformance or visual fidelity has been verified.

Target role runtime is GPT-5.6 Terra with high reasoning. Model application is **unverified** because the current runtime cannot attest it.

## Product Canvas Boundary

The product UI begins at the framed `DMQuant experience (dm-ai)` artboard: left strategy/version tree, center parameters/results, and right AI conversation. The prototype review toolbar in the gray striped area above it—`REVIEW`, `仅评审`, state, chat, role and coverage toggles—is static-prototype scaffolding for exposing mutually exclusive examples. It must be removed rather than recreated in the application. Runtime state is driven only by authenticated platform data and user actions.

## Primary User Journey

1. **Authenticate:** unauthenticated users enter the shared platform login. A valid platform identity establishes tenant and role context; `401` returns the user to login.
2. **Select or start work:** the user opens a strategy/version from the left tree or starts a new conversation. Each strategy is one AI conversation output; each saved edit creates a new immutable version rather than overwriting history.
3. **Generate a draft:** the user describes intent. The right pane streams content and reports code generation, static checks and dry run as inspectable steps. Sending is disabled while the request is active. Failure provides a retry, `code` and copyable `trace_id`.
4. **Apply or save:** “应用为回测参数” transfers draft parameters into the center form and visibly marks AI-filled fields without locking them. “保存为策略版本” creates a version. “查看 diff” compares the proposed version with its parent.
5. **Complete parameters:** common fields stay in the compact band; full `RunSpecSubmit` fields live in a collapsed advanced section with explicit defaults. Instrument selection supports search/multi-select or a rule-based universe. Coverage warnings appear before submission.
6. **Authorize and submit:** viewer sees a disabled run action plus the missing `researcher` role. An authorized researcher submits with an idempotency key. The UI distinguishes local validation, submission, queueing, running, cache hit and failure.
7. **Track execution:** the run task shows phase and percentage. After 30 seconds queued, it explains a platform-supplied reason such as quota or unavailable worker. The global task badge opens recent running-task details.
8. **Load terminal result:** success loads run details and series, then displays metrics, validation checks, reproducibility and product tabs. Cache hit shows the reused `run_id` and does not imply recomputation. Failure preserves diagnostics and strategy-source access.
9. **Inspect evidence:** users can inspect portfolio/benchmark/drawdown, signals, filled and unfilled orders, and generated files. Percentages convert API decimals only at presentation; money uses grouping; duration includes `Y`; time is shown in Beijing time with the timezone stated.
10. **Edit and rerun:** any strategy version, including draft or failed, can expose its strategy source where authorized. Editing and “保存并重新回测” creates a new version, then submits a new run; it never mutates the old version or result.
11. **Audit-sensitive actions:** export, download and destructive delete show scope before confirmation and create platform audit evidence. Deleting a strategy explains all affected versions; deleting a version explains associated run/artifact retention according to the eventual canonical contract.

## State Model

### Authentication and authorization

- `unauthenticated` → shared login.
- `authenticated.researcher` → read plus authorized create/save/run actions, subject to object-level RBAC/ABAC.
- `authenticated.viewer` → read-only; no strategy mutation, run submission or destructive action.
- `unauthorized/expired` → preserve safe local context if permitted, return to login, and never show raw platform errors or a false success.

### Conversation and draft generation

- `empty`: prompt guidance and example chips.
- `ready`: composer enabled with token count and keyboard guidance.
- `streaming`: incremental content and typing indicator; duplicate send disabled.
- Generation step substates: `pending`, `running`, `passed`, `warning`, `failed` for code, static check and dry run.
- `done`: code/actions become available.
- `failed`: actionable message, `code`, copyable `trace_id`, retry; existing conversation remains readable.

### Strategy/version

- Strategy tree: collapsed/expanded, selected/unselected.
- Version states: `draft`, `queued/running`, `succeeded`, `failed`.
- Status must use text/icon plus color. Every version has an accessible name and a stable strategy/version reference.
- Strategy source is available for any persisted version if the user's object permissions allow it. Result-derived artifacts are unavailable until success; the UI says “未生成” rather than showing empty fake files.

### Backtest result region

- `empty`: no run selected or no run submitted; explanatory call to action.
- `loading`: an existing run/result is being fetched; skeletons must not expose stale metrics as current.
- `submitting`: run action disabled and labeled “提交中…”.
- `queued`: task accepted, queue status shown; after 30 seconds show a server-provided reason.
- `running`: phase plus progress (`validating` → `backtest_engine` → `computing_metrics` → `artifacts`).
- `cache_hit`: successful cached run is loaded and labeled with `cached_run_id`/`run_id`.
- `succeeded`: validation badges, metrics, reproducibility and result tabs appear.
- `failed`: error banner and state body show `code`, reason and `trace_id`; strategy file remains reachable from “生成文件”, while run artifacts remain “未生成”.

These are mutually exclusive states for the primary result region. The production app must not contain a manual state selector.

### Per-tab states

Each of 组合走势, 买卖信号, 交易流水 and 生成文件 needs its own empty, loading, loaded and error handling. A failure in one series/file request must not erase already verified run metadata. Filled and unfilled trades are separately labeled; unfilled records expose a reason. Pagination and export state must remain visible.

## Roles and Permission Presentation

| Capability | Researcher | Viewer | Interface rule |
|---|---|---|---|
| View permitted strategies, versions, runs and evidence | Yes, subject to object policy | Yes, subject to object policy | Never infer tenant access from the UI role alone. |
| Generate/apply draft or save a new version | Intended | No | Disable or hide only after platform permission resolution; explain the required permission. |
| Submit/rerun backtest | Requires `researcher` plus object permission | No | Disable with a clear `researcher` requirement before request; backend remains authoritative. |
| Edit strategy source | Requires mutation permission | No | Save as a new version; never overwrite history. |
| Export/download | Requires corresponding audit/object permission | Read-only policy remains unresolved | Confirm scope, show progress/outcome and audit the action. |
| Delete strategy/version | Requires explicit destructive permission | No | Confirm impact; retention semantics must come from the canonical contract. |

Platform administration and role assignment are outside the DMQuant product canvas. The current prototype does not authorize an admin UI.

## API-to-UI Mapping

The method labels below are UI client abstractions from the design input. Architecture must bind them to canonical Protobuf services/messages and gRPC-Web server-streaming or unary calls before implementation.

| UI step | Design-level client/event | Fields/events | UI destination |
|---|---|---|---|
| Identity | `AuthContext` / JWT claims | `tenantName`, `role`, `roles`, `logout` | Shared login boundary, single-line user area, role gating and logout. |
| Authentication failure | unauthorized event | `401` | Return to shared login with an accessible session-expiry message. |
| AI draft stream | `createAiDraft` | `token`, `code`, `check`, `dryrun`, `done` | Incremental bubble, code block, expandable generation steps and completed draft actions. |
| AI/API failure | `ApiError` | `code`, `traceId`/`trace_id`, safe message | Error bubble/banner, copy trace, retry or corrective action. |
| Save strategy | `saveStrategy` | `strategy_id`, `version` | Strategy/version tree, advanced strategy reference and new version after edit. |
| Submit run | `submitBacktest` | idempotency key, `task_id`, `cache_hit`, `cached_run_id` | Submitting state, task polling, cache banner and run selection. |
| Poll task | `getTask` | `status`, `phase`, `progress_pct`, queue reason | Task badge and queued/running progress card; stop polling on terminal state. The proposed 1.5-second interval remains a client policy to validate. |
| Get run | `getRun` | `metrics[]`, `check_report.blocks[]`, `reproducible`, `fingerprint`, safe error and artifact references | Metric band, validation chips, reproducibility badge/card, errors, file availability. |
| Get time series | `getSeries(run, kind)` | `nav`, benchmark/drawdown if contracted, `signals` | Portfolio and signal charts; absent series receives a per-tab state. |
| Orders/unfilled | canonical run or paged order query, unresolved | filled orders, unfilled orders and reason | Filled/unfilled segments, count, table, pagination and audited export. |
| Instruments | `listInstruments` | search results and rule-capable universe values | Search/multi-select and rule-mode instrument control. |
| Tasks list | `listTasks` | running task count and recent tasks | Global “运行中 N” badge and task detail. |
| Download/export/delete | canonical audit-capable methods, unresolved | object reference, scope, audit reference and terminal outcome | Confirmed action, progress, success/failure and audit acknowledgment. |

### `RunSpecSubmit` form mapping

| Visible field | Contract concept | Presentation |
|---|---|---|
| 策略 ID / 版本 | `strategy.strategy_id`, `strategy.version` | Advanced; generated from selected version, not free-form in final UI. |
| 资产池 / 具体标的 | `universe: string[] | {rules:[]}` | Compact search/multi-select with rule-mode entry. |
| 回测行情频率 | `clock.freq` | `1分…1日` displayed, mapped to `1m/5m/15m/30m/1h/1d`. |
| 时间范围 | canonical clock range fields, exact names unresolved | Compact date range plus coverage warning. |
| 交易日历 | `clock.master_calendar` | Advanced: `IB`, `EXCH`, `CFFEX`; incompatible combinations fail before submit where possible. |
| 撮合模型 | `execution.fill_model` | Advanced: `next_quote` or `next_bar_vwap`. |
| 再投资利率 | `execution.reinvest_rate` | Advanced with explicit source or fixed rate. |
| 初始资金 / 杠杆上限 | `accounts.init_cash`, `leverage_cap` | Advanced; numeric units and limits stated. |
| 模式 / 随机种子 | `mode`, `seed` | Advanced; seed is surfaced as reproducibility evidence. |

`K线数量` appears in the prototype but has no explicit canonical mapping in the design table. It must be derived from the selected range/frequency or bound to a Protobuf field before implementation; it must not become an untracked parallel parameter.

## Error and Recovery Requirements

- Every platform error shows a safe user message, stable `code` and copyable `trace_id`; never expose a stack trace, secret, raw database message or model prompt.
- Field/contract validation stays next to the responsible control and moves focus to a summary on submit. Preserve user input.
- Permission failures state the missing role or capability and a next step; never present only `403`.
- Session expiry returns to shared login and distinguishes expiry from a failed backtest.
- Long tasks expose phase/progress. Unknown progress uses a named phase and elapsed time, not an endless unlabeled spinner.
- Queue delay explanation comes from platform state; the UI must not manufacture an ETA.
- Cache hit, success and failure are distinct outcomes. Never show cached data as a fresh computation.
- Retry is offered only for retry-safe operations. Run submission reuses or rotates an idempotency key according to the canonical contract to prevent duplicate runs.
- Partial result failures are contained per tab. The user can still copy diagnostics and inspect confirmed metadata.
- Destructive actions explain impact and require confirmation; export/download expose audited success or failure.

## Accessibility Requirements

- All operations must be keyboard reachable with a visible focus indicator. DOM order follows left strategy tree → center workflow → right conversation without forcing pointer use.
- Use real buttons/links/form controls. Strategy tree rows, inline version actions, tabs, generation-step disclosures and splitters need correct keyboard behavior; clickable `<div>`/`<i>` elements in the static prototype are not implementation guidance.
- Associate every label with its control. Frequency uses a radio-group or `aria-pressed` semantics; tabs use tablist/tab/tabpanel; disclosures expose `aria-expanded`; splitters expose separator/value semantics and keyboard resize.
- Announce streamed responses, submit results, progress and errors through appropriately throttled live regions. Do not announce every token. Move focus to a result/error heading only when it helps rather than disrupting typing.
- Never encode status, buy/sell direction, validation result or reproducibility by red/green alone. Pair color with text, icon/shape and accessible names. Chart series require non-color distinctions.
- Charts require an accessible summary and a table/data alternative for key values, signals and drawdown. Tooltips cannot be the only way to access metric definitions.
- Error text is programmatically connected to its control. Copy-trace actions and retry buttons have descriptive names.
- Modal confirmations trap focus, have named headings/descriptions, return focus to the invoker and support Escape where safe.
- Loading skeletons are hidden from assistive technology; progress uses determinate or indeterminate progress semantics as appropriate.
- Verify text, control and focus contrast against WCAG 2.2 AA. The prototype palette is not yet attested. Respect reduced-motion preference for spinners, blinking dots and chart animation.
- At narrow viewport/zoom, the three-column layout must reflow or provide an operable pane switcher without clipping actions. Validate at 200% zoom and with long Chinese/English identifiers.
- Logo and icon-only controls need meaningful accessible names; decorative icons are hidden. `title` attributes are supplementary, not the only label.

## Findings Requiring Coordination

### Important

1. **One contract source:** UI-DM mentions SSE and OpenAPI-generated TypeScript, conflicting with the README's Protobuf/gRPC-Web baseline. Architecture must define canonical streaming, service and message mappings; Interface will then replace aliases in the durable reference.
2. **Missing contract details:** failed-run strategy-source reference, paged orders/unfilled reasons, task queue-reason shape, audited export/download/delete outcomes, exact time-range fields and `K线数量` are not canonicalized.
3. **Authorization matrix:** viewer is clearly unable to run, but export/download visibility and object-level mutation/delete permissions need Product/Architecture/Quality agreement.
4. **Accessibility:** the static HTML demonstrates layout and states but does not meet an attested accessibility standard. The later engineering checklist must include keyboard, semantics, live-region, contrast, zoom and chart-alternative evidence.

### Notes

- The four-tab scope is coherent for the first DMQuant experience. Attribution, DV01 exposure, heatmaps, distributions and up-to-five-run comparison remain deferred ideas, not iteration-1 commitments.
- The prototype includes a “对比” button despite the design note deferring comparison. It must be removed/disabled until Product places comparison in an approved checklist and the contract exists.
- Static sample dates, names, identifiers, values, metrics and files are illustrative and must not become hardcoded production data.

## Validity

Valid: iteration-1 only
