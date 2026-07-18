# Orchestrator Decisions

## D-001 — Project classification

Classify ficant as a new project with authoritative design inputs. Initialize iteration-1 governance directly; do not run iteration-0 baseline recovery because there is no legacy production implementation to recover.

## D-002 — Current iteration scope

iteration-1 establishes governance and Phase 0 contract readiness only. It does not create production code and does not claim README Phase 0 delivery.

## D-003 — Source authority

`README.md` controls system facts. `UI-DM/` controls DMQuant experience design only. When the two conflict, the WebApp must adapt to the platform baseline.

## D-004 — Model policy evidence

Store the requested Sol/Terra routing as a hard dispatch policy. Because the current dispatch interface has no model-selection or attestation field, record runtime application as unverified instead of claiming enforcement.

## D-005 — External systems and secrets

Record GitHub owner, test host, and key-directory location for later Delivery work. Governance initialization does not access GitHub remotes, the test host, or key files.

## D-006 — Auxiliary planning documents

PROQAID governance plans belong under `.proqaid/orchestrator/`. Ephemeral execution memory belongs under `.planning/`; do not recreate `docs/superpowers/`.

## D-007 — Adjacent model fallback

If a requested target model is unavailable, at capacity, or fails allocation, Orchestrator may retry with one adjacent model tier lower or higher. The dispatch record must retain the original target, fallback direction/reason, and actual model when attested. If the runtime cannot expose selection or attestation, record `unverified fallback` rather than claiming a specific fallback model.

## D-008 — GitHub first-version allowlist

Create the private repository `github.com/kayz/ficant` and publish a clean `main` root history containing only `.gitignore`, `README.md`, `src/`, `docs/`, and `result/`. All other files remain local and ignored. `.gitignore` is the sole technical file outside the user's content-path list because it enforces the exclusion policy.

## D-009 — iteration-2 scope and layout

Merge README Phase 0 and Phase 1 into iteration-2. Root `interface/` replaces the previous `proto/` entry as the sole backend/Protobuf contract source; root `web-dm/` colocates page design and WebApp code. Phase 2 numerical behavior and full DMQuant implementation remain out of scope.

## D-010 — Chinese docs, TDD, and archival

All iteration-2 natural-language outputs under `docs/` are Chinese. Quality runs before implementation and may repeat during/final verification. Workers use TDD, real business evidence, isolated scopes, and cleanup. When an iteration closes, its `.proqaid` iteration-specific artifacts move under `.proqaid/archive/iteration-N/`; durable charter/context/decisions remain current only while valid.

## D-011 — 事件驱动的完整 Agent 状态板

Orchestrator 在 `.proqaid/orchestrator/agent-status.md` 维护当前迭代的完整角色状态快照。状态板始终列出七个常驻角色和所有已派发的临时 worker；已完成的常驻角色不隐藏，worker 清理后保留记录并标记为 `cleaned`。状态板同时记录截至最后事件的命令耗时、完整 tasklist 执行情况，以及按明确 scope/base 分开的文件数和新增/删除/合计行数。仅在初始化、派发、状态转换、阻断、review 交接、完成、清理或迭代归档等真实事件发生时更新；不为刷新文档轮询 agent、不另起监控 agent，也不伪造心跳。用户直接读取该文档查看全局状态。

## D-012 — iteration-2 首次契约基线

首次集成的唯一 Protobuf 契约基线固定为 commit `591dfcaf46eb9fdc8a68d879edbc542dd9ded448`，descriptor SHA-256 固定为 `d1832ff40a3057d9ae11c7e7dcc8c847efbf13c76f4e18a14f8d905be3fdf1d0`。由于 `main` 的 `42f570f309e20c867f65cffbce76e7f6d64d65d5` 不含 `interface/`，首次 `Q2-CTR-02` 只能记 `not-applicable-initial-contract-baseline`；从本 commit 之后的任何契约变化，`buf breaking` 必须使用该 exact ref 和 `subdir=interface`，不得改用浮动 branch。

## D-013 — 内部第九个领域错误码

依据 Architecture iteration-2 round-4 的 `approve-invalid-value` 裁决，批准 `DomainErrorCode::InvalidValue` 为内部第九个稳定错误码。它只用于无法诚实归入 ID、unit/time/version/hash/lineage/state/journal 八个专用码的普通领域值校验；专用边界必须优先。其唯一外部映射固定为现有 `ficant.core.v1.ERROR_CODE_VALIDATION_FAILED`、gRPC `INVALID_ARGUMENT`、`retryable=false`，不得新增 Proto `INVALID_VALUE`、不得向外暴露 Rust variant 名。Task 3 锁定九码与专用码优先测试；application/API 在后续任务实现无 wildcard 的穷尽映射、安全 field violations/trace，契约测试继续禁止平行外部码。

## D-014 — 新版 PROQAID 运行规则（supersedes D-011）

自 Task 4 候选完成的安全事件边界起，iteration-2 使用 2026-07-11 读取的新版 PROQAID Full 规则；本决定正式 supersede D-011。停止更新 `.proqaid/orchestrator/agent-status.md`，该文件只作为此前流程的历史证据保留到 iteration-2 归档。运行中只保留有明确下游读者与决策用途的权威文档，不再默认生成 status、latest/stamped 或重复过程副本。测试采用唯一责任门：Worker 负责一次有效业务 RED、同命令 GREEN 与模块回归；Review 只对 replay/security/migration/money/contract 等变化风险做 fresh 子集；Quality 每个集成波次执行一次跨模块真实业务验收；Delivery 只做环境/部署/容器专项；iteration exit 只形成一份完整测试和真实业务闭环证据集。Review 节奏改为 Design Freeze、高风险候选、集成波次与最终退出，不默认审每个微任务。Orchestrator 聚焦并行调度、依赖路由、串行集成和异常处理，不重复 worker 代码工作。现有 Architecture、Interface、Quality 冻结结果继续作为本轮 Design Freeze，不重做；iteration-2 结束前不合入或推送 GitHub `main`。

## D-015 — PROQAID 单一治理层（supersedes 并行 Superpowers 流程）

iteration-2 只使用 PROQAID 作为项目计划、agent 调度、开发波次、Review 节奏和迭代记忆的权威系统，不同时启动 Superpowers 或其他 planning/executing/subagent-development/code-review 治理流程。自本决定起禁止继续创建或更新 `.superpowers/`；现有内容仅作只读历史证据，在 iteration-2 退出时随本轮治理产物归档或清理。TDD、worktree 隔离、系统化调试与完成前验证仍可在 PROQAID 已分配的 worker/gate 范围内作为底层技术使用，但不得产生第二套计划、状态、dispatcher、worker hierarchy 或 Review owner。仍需保留的 Task 事实必须进入当前 PROQAID authority document 或唯一 worker evidence，不得双写。

## D-016 — Task 7 身份、revision 与 Journal presence 语义（局部 supersede Design Freeze）

Task 7 实现前的风险分诊确认三项冻结冲突，按最小边界局部 supersede 旧表述，不重做其余 Design Freeze，也不修改已冻结 Proto。第一，Artifact 与 SignalSet 是两个独立根对象，必须使用不同 ULID；SignalSet 通过 content-addressed `artifact` LineageRef 引用实际 Artifact，引用的 object ID、content hash、owner、blob hash/size 与完整 lineage 必须一致，Artifact kind 必须为 `SignalSet`，禁止要求 `signal_set_id == artifact_id`。Architecture 旧表述“`signal_set_id = Artifact ref`”与 Domain/Application/Storage 的同 ID 约束由 W2/W3 前向纠正；Quality 固定 F18 Artifact 与 F19 SignalSet 保持不变。第二，ExperimentRun revision 使用已集成的正整数合同：CREATED/RUNNING/SUCCEEDED 为 `1 → 2 → 3`，`expected_revision` 表示变更前的当前 revision。第三，RunJournal sequence 1 的 `prev_hash` 必须不存在：Rust `None`、Proto absent、PostgreSQL `NULL`；sequence 大于 1 时才携带上一事件的实际 hash。Canonical FJRN v1 继续区分 absent presence byte `0` 与 present `1 || 32-byte hash`，不得用 32 个零字节冒充 absent。Task 7 在 W2 身份修正经 Review/集成、W3 前向 Migration/Repository 同步后恢复。

## D-017 — D-016 的 pre-release legacy-data 与 lineage 一致性策略

ficant 尚未发布，D-016 之前不存在受支持的生产 SignalSet 数据；旧 schema 的 same-ID SignalSet payload 无法只靠 SQL 安全转换为两个独立 content-addressed 根对象，因为转换任一根 ID 都会改变 canonical payload/hash、对象引用及外部 identity。0007 因此禁止以 `artifact_id = signal_set_id` 伪装升级：从空的 0001..0006 schema 正常前向升级；若 `research.signal_sets` 已有任何旧行，Migration 必须在变更 schema 前原子 fail closed，保留旧表和数据不变，并要求显式离线导出/重建流程，iteration-2 不声称兼容未发布的旧 SignalSet 行。Repository 发布新 SignalSet 时，除校验独立 Artifact ref、tenant、owner、kind、hash 与 verified blob 外，还必须验证 lineage 集合一致：SignalSet 中除承载 Artifact 自身 ref 之外的完整 lineage，必须与持久化 Artifact 的完整 lineage 相等；不得只验证所有 target 各自存在。任何缺失、额外或漂移 ref 都返回稳定 lineage 错误且不产生 Signal/blob/lineage/idempotency 半状态。

## D-018 — Snapshot 按类型持有 durable blob proof

Task 7 的真实 PostgreSQL/MinIO 业务 RED 证明 DataSnapshot manifest 虽已 verify/promote，却因公开 Snapshot command 只消费 data blob proof 而永久停留为 orphan。按冻结 README、Architecture 与 Quality 的五个正式内容地址对象要求，DataSnapshot 必须同时持有两个不同角色的 durable verified blob refs：`blob_content_hash` 对应 data/Parquet，`manifest_hash` 对应 Snapshot Manifest；两者都校验 tenant、精确 owner、hash 与 size，并在同一发布事务建立正式引用。UniverseSnapshot 只有一个成员 Manifest ref，由其 `content_hash` 表示；`schema_hash` 在 iteration-2 仅为摘要，不另建正式对象。W2 以按 Snapshot 类型表达的 proof 修正 Staged/Publish command 并升级 fingerprint；W3 以前向 Migration/FK/index 与 repository 事务修正 Data 两条、Universe 一条引用。冻结 Proto 已含所需字段，不修改；Quality 的五个对象以及 orphan/staging 为零的断言保持，不得用 acceptance SQL、直接 MinIO 或修改计数补绿。

## D-019 — MarketFact Unit 语义采用 opaque resolved proof 与持久定义复核

Q2-INV-01 的真实负向 RED 证明 `UnitRef(id,version)` 足以构造结构合法的 Domain fact，却不足以判断字段所需 dimension；不在 Domain constructor 硬编码 Unit ID、fixture code 或基础设施查询，也不修改冻结 Proto/DecimalValue/MarketFact 形状。W2 在 Application 定义稳定 field roles：Cashflow amount=`currency`；Quote bid/ask=`price`；Trade price=`price`、quantity=`notional`；iteration-2 Valuation values=`price`，其他 measure 在后续 Domain Pack 明确前 fail closed。Application 经 DefinitionRepository 解析同 tenant 的精确 Unit version，验证 dimension、Decimal scale≤Unit scale、有效 precision≤Unit precision，并生成 opaque resolved proof；proof 绑定 fact canonical bytes、scope/tenant、fact kind/ID、role/ordinal、UnitRef 与解析出的 dimension/scale/precision，无公开 fields/variants/unchecked constructor。Append、correction 与 Phase1 只接受 validated fact/proof并在任何 BlobStore、idempotency 或事务 I/O 前重验 shape/binding；合法 fact 继续使用包含 UnitRef 的 `append-market-fact/v1` fingerprint，proof 是准入能力，不复制进业务 intent。W3 在写事务、任何 idempotency/fact insert 前按 tenant+unit ID+version 查询真实持久 Unit Definition并复核 proof；missing/wrong kind/tenant/role/dimension/scale/precision/proof 统一映射 `InvalidUnit → ValidationFailed, retryable=false`，且不得产生 PG/MinIO/Run/Journal 副作用。Storage 复核是纵深验证，不得成为唯一语义边界；本决定不引入换算、汇率、价格归一化或任何 Phase 2 数值能力。

## D-020 — RulePack effective 采用 opaque exact-version proof 与 DataSnapshot.as_of run time

Q2-INV-12 的合同分诊确认：Valuation 使用自身 `valuation_at`，ExperimentRun/Phase1 的 run market time 固定为其绑定 DataSnapshot 的 `as_of`；RulePack effective 统一为半开区间 `effective_from <= subject_time < effective_to`。不得改用执行 Clock、Journal occurred_at、Snapshot visible_at、Signal valid_from或某一笔 Trade 时间。W2 在 Application 解析显式 exact RulePack version，生成无公开 fields/variants/unchecked constructor 的 opaque proof：Valuation proof 绑定 scope/tenant、fact identity/canonical digest、valuation_at、exact ref、持久 effective interval与 binding hash；Run proof 还绑定 run identity/canonical digest、DataSnapshot identity/content hash/canonical digest/as_of、完整有序 RulePack refs及各 interval。Valuation Append/Correction只能消费同时完成 Unit 与 RulePack proof 的 fully validated fact；CreateRun/Phase1只能消费 validated run，并重验 missing/extra/duplicate及 fact/run/snapshot swap。所有解析必须在 mutating BlobStore、Clock/ID、idempotency 与 transaction write 之前完成；Definition/Snapshot只读查询是 proof 所需 I/O。W3 在 Valuation/Run/Phase1 写事务第一步按 tenant+rule ID+version复核持久 RulePack 与 interval，之后才允许 idempotency或正式写入。coverage miss 映射 `InvalidEffectiveTime → ValidationFailed, retryable=false`；missing/wrong kind/identity/version/tenant或proof shape映射 `BrokenLineage → LineageIncomplete, retryable=false`。冻结 Proto、Domain 与合法 `append-market-fact/v1`、`create-experiment-run/v2`、`phase1-atomic-work/v2` fingerprint保持不变；proof binding hash不进入业务 intent。本决定不执行RulePack内容，不增加定价/规则引擎，不扩展 Curve/Futures/Quote/Trade/Cashflow 或 Phase 2 能力。

## D-021 — Phase1 首次 Snapshot→Run 使用 pre-stage candidate resolver

D-020 的 persisted-only Run resolver 无法组成首次原子 Snapshot→Run：本次 DataSnapshot 在 Phase1 transaction commit 前尚不可从 SnapshotRepository读取，而先发布再解析会制造事务外副作用；`StagedSnapshot` 又已发生 BlobStore mutation，不能作为 pre-I/O resolver 的前置。Standalone CreateRun 继续使用 persisted Snapshot resolver与 `ValidatedExperimentRun`。Phase1 新增 definitions-only candidate resolver，直接消费 pre-stage `&DataSnapshot`、raw ExperimentRun与 DefinitionRepository，在任何 staging/Clock/ID/idempotency/transaction write 前验证 exact RulePack interval并返回独立 opaque `Phase1ValidatedExperimentRun`；该 sealed wrapper不得进入 standalone `CreateExperimentRun::new`。完成 staging 后，Phase1 input将 candidate proof与 `StagedSnapshot` 的 snapshot identity、content hash、canonical digest、as_of、tenant/owner精确配对并重验，禁止 swap。W3 在同一 transaction先持久 Snapshot，再在 persist Run 前读取该未提交行并按 D-020 proof复核真实 snapshot与持久 RulePack。冻结 Proto、Domain及 `append-market-fact/v1`、`create-experiment-run/v2`、`phase1-atomic-work/v2` fingerprint不变；禁止以预发布 Snapshot 或 fake persisted repository补绿。

## D-022 — 业务错误与 transport trace 采用同 SHA 双层证据

Phase1 Application/Storage 边界的 `ApplicationError` 只拥有 category 与 retryable；trace_id 是 transport 安全可观察量。本轮不新增 Phase1 业务 RPC、不修改 Proto，也不要求 W3 伪造 trace 或调用无业务入口的 Platform mapper。W3 对每个 Q2-INV 独立负责真实业务 category、retryable 与 PG/MinIO/Run/Journal 零副作用。W1 在 `ficant-api` 增加独立 core business error mapper/status builder，对 12 个 `ApplicationErrorCategory` 进行无 wildcard 穷尽映射，输出冻结的 `ficant.core.v1.ErrorDetail`、tonic gRPC code、Application retryable语义与安全非空 trace；不得复用会把业务错误压成 app `INVALID_REQUEST` 的 Platform `SafeErrorMapper`。映射固定为：Validation→ValidationFailed/InvalidArgument；NotFound→NotFound；AlreadyExists→AlreadyExists；VersionConflict/ConcurrencyConflict→各自 core code/Aborted；ImmutableViolation→ImmutableViolation/FailedPrecondition；HashMismatch→HashMismatch/DataLoss；LineageIncomplete→LineageIncomplete/FailedPrecondition；Unauthenticated→Unauthenticated；Forbidden→PermissionDenied；StorageUnavailable→Unavailable。`StateConflict` 不新增外部枚举，映射既有 `ImmutableViolation/FailedPrecondition/retryable=false`，不得降级为 Validation。Quality 只在同一 integration SHA 组合 W3 case evidence 与 W1 category mapping evidence，形成唯一 Q2-INV→category→core code→gRPC→retryable→side-effect表；两层不要求来自同一次业务 RPC。mapper安全 message/trace不得泄露 SQL、bucket、credential、stack、raw cause或敏感输入。

## D-023 — optional probe 与 required published-content read 分离

已发布 metadata/resource 不存在仍是普通 NotFound；但 metadata 与 tenant-scoped durable ref 已存在时，MinIO object missing、bytes hash漂移或expected size漂移是 published integrity loss，固定返回 `ApplicationErrorCategory::HashMismatch, retryable=false`，由 D-022 映射 `HASH_MISMATCH/DATA_LOSS`，不得伪装 NotFound。无法判定missing/corruption的网络/MinIO故障仍为 StorageUnavailable。Storage 现有 optional `read_verified -> Option` 改名 `probe_verified`，仅用于 orphan/reconciliation/module probe与尚未发布场景，不进入Application正式业务读。W2新增 `VerifiedBlobReader` required port与opaque `RequiredVerifiedBlobRead`，绑定 AccessScope、tenant/owner、business resource kind/id、blob role、expected hash/size；required结果不返回Option，必须复核 durable ref后读取并重算hash/size。新增正式 `read_verified_artifact`、`read_verified_signal`、`read_verified_snapshot`：Signal通过精确Artifact ref读取同一payload；Data同时读取data/Parquet与Manifest，Universe读取members Manifest，全部required role成功后才返回。W2同时定义结构化 `IntegrityEventSink` 与固定事件 `storage.published_content_integrity_failure`，字段只含severity、reason(`missing|hash_mismatch|size_mismatch`)、tenant、resource kind/id、blob role、expected hash/size与safe trace context；禁止bucket/key/endpoint/credential/token/raw bytes/SQL/stack/raw cause。W3在required read发现integrity loss时恰好提交一次事件后返回HashMismatch；sink失败不得改写已知错误或返回成功。Delivery/W1 composition提供有schema的production adapter/出口，Quality在同一integration SHA观察真实delete/replace后的required业务读、精确事件、D-022映射与零额外副作用。不修改Proto/RPC/write fingerprint。

## D-024 — iteration-2 尾段切换至 PROQAID `7497980c`（运行治理决定，不新增设计边界）

切换发生在 2026-07-12 Task 10 Ubuntu 24.04 环境准备返回 `BLOCKED` 的安全事件边界：当时没有运行中的 apt、dpkg、curl、下载、构建、测试或扫描进程，未提交代码、有效 worktree 与外部证据均未被重置或清理。自该边界起，iteration-2 剩余工作使用发布版本 `7497980caaa8ff4b545c8b2c4c4d5e47ef3b7f6f` 的 PROQAID Full、Tool-First、Human Operator 和外部执行合同；本决定不重开 iteration、不重新执行规模门、Design Freeze、D-015～D-023、已通过 Review 或测试，也不把 D-024 视为 D-023 之后的新业务/接口设计边界。

切换时 integration 为 clean `b93255767c92e73f28206f3af0910032b6b15d26`。继续有效的证据包括：业务 SHA `dbcff34793e79e73ed63872e28ed6298feedfbc4` 的 Quality 14/14、派生 `python-node-runtime` OCI manifest `sha256:8e97031468b2ad51ab8484d06d8af9d63f1b73f8c04654f17be40ac629076cd9`、里程碑一 Delivery/Review `PASS — C0 / I0 / M0`；以及已集成至 `b932557` 的 Task 10 Runtime、Web、Gates、十项 CI、allowlist/deny 与 GateRecovery 候选及各自 focused Review PASS。此前 Contract、Supply、Repro 的环境阻断执行只作为失败证据保留，不得重标为通过。

剩余影响矩阵：Orchestrator 继续负责外部执行路由、证据绑定、串行集成与清理；Delivery 受影响，负责 CI/CD、固定工具、供应链、构建、Migration 与唯一 Compose 专项；Quality 仅在 Task 10/退出候选上判断完整证据，不重复里程碑一业务 wave；Review 保持独立并在交付波次与最终退出审计；Product、Architecture、Interface 在最终中文权威文档与实际实现一致性收敛时受影响，但不重新设计或重跑已通过 Interface gate。下一门是 Tool-First 外部执行 preflight：优先使用现有 GitHub CI/CD 在非 `main` 候选分支运行十项确定性 gate；若 CI/CD 不可用，再把 Clang 18 与 Syft 固定资产等所有宿主/WSL准备合并为一次 Human Operator 包。通过后进入唯一 live Docker/Compose Delivery 专项、中文文档收敛和 iteration exit。

## D-025 — iteration-2 发布候选采用 clean-base 单提交拓扑（运行交付决定，不新增设计边界）

Review 对 Supply recovery 的真实历史探针证明：仅扫描当前树与 `HEAD` 单提交，无法排除 iteration 工作历史中“引入后删除”的 secret；而 iteration-1 已冻结 GitHub `main` 只允许显式 fast-forward 发布。iteration-2 因此固定以下发布拓扑：可信基线为当前远端 `main` 的精确提交 `42f570f309e20c867f65cffbce76e7f6d64d65d5`；最终退出候选必须从该基线创建唯一一个 squashed 发布提交，其 tree 必须与通过最终门的 clean integration tree 完全一致，且 `main` 仅在最终 Review 通过后 fast-forward 到该单提交。integration 的详细任务提交继续作为本地 iteration 证据归档，但不得作为 GitHub `main` 的发布 ancestry。

Supply gate 必须同时证明：可信基线的已发布历史扫描 clean；当前发布 tree 的目录扫描 clean；`trusted-base..candidate` 精确发布区间扫描 clean；candidate 的唯一父提交等于可信基线且区间提交数恰好为一。以上 SHA、tree、扫描工具/数据库版本与结果必须绑定同一最终候选；禁止 `ignore`、仅扫 `-1 HEAD`、浮动 `origin/main`、隐式 merge、把临时 CI 分支当作发布分支或在最终 Review 前更新 `main`。若远端 `main` 在发布前偏离该可信基线，发布必须 fail closed 并重新进入 Delivery/Review，而不是自动 rebase、force-push 或扩大历史范围。本决定只关闭 Task 10 的发布拓扑与 secret-history 证据，不修改 D-015～D-023、Design Freeze、业务模型、接口或 Phase 2 范围。

## D-026 — iteration-2 许可证与无修复依赖的限定处理（用户接受的运行交付决定）

用户于 2026-07-12 批准 Task 10 许可证闭环采用以下限定合同。Syft 识别出的 13 个 Ficant 一方包作为内部自有组件处理，必须在机器可验清单中逐项绑定精确 name、version、purl 与 release-tree source；禁止以 `ficant-*`、scope、路径或其他前缀/正则批量豁免。该分类不构成开源许可、对外授权或未来包的继承规则。第三方 inventory 与一方清单必须不相交，二者并集必须精确等于同一候选的 Syft package key 全集。

第三方 SPDX 表达式必须按真实 `OR`、`AND`、括号与 `WITH` 语义求值并 fail closed。`r-efi` 只允许由其真实表达式中的 MIT 或 Apache-2.0 分支满足本轮策略，不接受 LGPL-2.1-or-later 分支。`CDLA-Permissive-2.0` 与 `CC-BY-4.0` 不加入可被未来依赖继承的全局 allowlist，只能作为当前 SBOM 中精确 package、version、purl/source asset 与 source integrity 绑定的 scoped allowance，并生成相应许可证文本和归属记录；任一版本、来源、资产或完整性漂移必须重新审查。

`async-std 1.13.2` 的停止维护风险仅在 iteration-2 被用户接受，必须继续标记 `accepted-unfixed`，绑定其精确 purl/source/checksum及经 `minio` 到 Ficant storage 的可达链；禁止标记 fixed、ignored 或让其他版本/包继承。必须在 iteration-3 入口或首次外部发布前（以较早者为准）重新评估替换方案。Cargo lock-only OSV 原始结果继续保留；仅当固定 Cargo 1.96.1 的 locked/all-features/all-targets resolved graph 证明包不可达时，才可附带 `unreachable_lock_only` 证据，不得删除原始 finding 或把可达 finding 降级。本决定不扩大业务、接口或 Phase 2 范围。

## D-027 — 用户明确授权跳过 iteration-2 剩余 Review

状态：`Review skipped by explicit human authorization`。用户明确授权跳过本轮剩余的候选 focused Review 与最终 Review audit；既有 Review 证据继续有效，但不再追加新的 Review 轮次、等待 Review verdict 或伪造 `Review pass`。Review 跳过不豁免任何确定性质量、业务、Migration、数据完整性、Supply、secret、许可证、漏洞、Compose 安全、清理或 D-025 单提交发布拓扑门。全部确定性门通过并完成发布后，iteration-2 的最终状态固定为 `closed-with-human-approved-review-deviation`。

## Validity

Valid: long-term until superseded
