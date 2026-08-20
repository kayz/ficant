# R7B 迭代 brief — 一期证据与恢复收口

**迭代：** R7B · **点亮目标：** AC30–AC33 · **execution base：** `34402344c7d2c9238dc171af52ac4db77eb6b462` · **authority commit：** `3e028bcadd9fe973450d35b092e03a7485a54881`

本 brief 是 R7B 面向 Human 的唯一设计、权限边界与最终证据载体。private authority PR #23 已把 Human 批准的目标 MANUAL、正式输出范围、确定性身份、恢复与 clean-environment 判据线性合并到上述 authority commit；该 authority snapshot 仍绑定 R7A 公共 base，并明确保持 AC30–AC33 未点亮。R7B 只把这些判据机械实现、重取证并交付精确公共候选，不创建版本、tag、镜像发布或部署。

## 1. 目标

R7B 为 v0.1 的全部正式分析输出建立同一个不可伪造的证据合同。五个 Rates RPC、Portfolio KRD、PositionViews、成功的 CapitalUse、DataHealthReport，以及 Experiment 产生的 Artifact / SignalSet，必须返回或持久化 `FormalOutputEvidence`：稳定排序的全部实际消费输入、exact Subject、公共 Git commit/tree、实际执行镜像、环境/实现摘要、参数、结果 hash 与稳定 output identity。同步 Analytics 与 ResearchGraph 使用同一 canonical identity 算法；run/request/trace id、attempt、lease 和时钟不进入 identity。

ResearchGraph 提交必须携带并在 server required-read 的 exact Subject；可复现身份显式绑定 Code。两次 run 的比较从 Data、Universe、Graph、Parameters、Runtime、Environment、Seed、RulePack、Implementation、ExternalInput、Result 共 11 维扩为再加 Subject、Code 的 13 维。Graph output trace 返回 typed、role-bearing 的正式证据图，不能再从无类型 `LineageRef` 猜对象；legacy Artifact 若没有正式证据，仍可按旧接口读取，但完整血缘查询必须失败关闭而不是冒充 AC30。

Worker 对每个输出在任何 blob stage 前持久化 publication intent；stage、promote、PostgreSQL complete、ack、lease expiry 与进程重启各冻结 crash point 均可 forward-only 恢复。完成事务精确核对 intent、fence、Artifact、manifest、formal evidence、hash/size；生产 orphan maintenance 只清理超过宽限期且既无正式引用也无活跃 intent 的对象。恢复后的最终 bytes、result hash、formal identity 与不中断基线逐位相同，且只有一个可见 Artifact 和一个 terminal result。

本地灾备入口在隔离 PostgreSQL + Ceph RGW 中生成数据库 dump 与完整 immutable-object manifest，manifest 绑定 object key/size/SHA-256、公共代码身份及运行镜像身份；销毁原状态后恢复到全新数据库和 bucket，再 required-read 一个 Graph output 与一个同步 Analytics formal output并逐字节核对。公共 clean-checkout runner 从 exact private authority commit 读取 `MANUAL.md`，不复制 private 文本入库，按顺序逐字执行每个 `ficant-manual-literal` PowerShell block。

**Acceptance sentence：**

> 给定任一冻结正式输出，其 evidence 必须可机械展开到全部实际消费的 typed input、exact Subject version/content、公共 Git commit/tree、实际运行镜像、环境/实现、参数与结果 bytes，且同一事实产生逐位相同结果与 identity、任一绑定元素漂移改变 identity；给定同一 ResearchGraph，在不中断和 publication intent 之后任一冻结 crash point 重启，最终只产生一个与基线逐位相同的 Artifact/terminal result，旧 fence、漂移 intent、缺失或篡改对象均失败关闭且生产 orphan maintenance 不删除活跃或已引用对象；给定一份隔离 PostgreSQL/Ceph 备份，销毁原状态并恢复到全新实例后，Graph 与同步 Analytics 正式输出的 bytes/evidence/identity 全部相同；给定 exact authority commit 的 MANUAL，clean-checkout runner 按字面、按顺序执行所有标记命令均成功。

## 2. 验收

| 条目 | R7B 可执行判据 |
|---|---|
| AC30 · 正式输出集合 | descriptor 与 service tests 精确证明五个 Rates result metadata、Portfolio KRD、PositionViews、CapitalUse、DataHealthReport、Artifact 与 SignalSet 都携 `FormalOutputEvidence`；基础 Definition/Fact/Snapshot 读取不携带。任何成功 service path 缺 evidence、空 Subject/hash、未排序或重复 role 都失败。 |
| AC30 · typed 完整血缘 | `FormalInputBinding` 具有稳定 role、封闭 kind、exact owner、versioned/unversioned identity、content hash 与适用时间。Graph trace 展开 output evidence 与所有上游 formal evidence；不得依据裸 ULID 猜类型。legacy Artifact 的普通读取保持兼容，full-formal trace 返回 `LineageIncomplete`。 |
| AC30 · Code / Runtime | Server 与 Worker 二进制嵌入 build-time Git commit/tree，并与部署只读设置精确相等；Docker build 显式传入二者。同步输出绑定实际 server image config digest，Graph output 绑定 worker image config digest。缺失、占位、格式漂移、compiled/deployed mismatch 在服务开始接受正式输出前失败关闭。 |
| AC31 · 单一身份算法 | domain canonical builder 以长度分隔、domain-separated bytes 绑定 schema、Subject、稳定排序 inputs、Code、Runtime、Environment、implementations、parameters、optional seed 与 result hash。相同输入跨 request/run/attempt/clock 得到相同 identity；每个字段与亚微秒时间单独漂移都改变 identity；非 canonical 顺序和 duplicate key 被拒绝。 |
| AC31 · 结果持久化 | 同步正式输出在返回前由 `FormalOutputRepository` 以 identity 幂等持久化 canonical payload 与 evidence；同 identity 不同 payload/evidence 失败关闭。Graph Artifact/SignalSet 同事务持久化 formal evidence，完整读取交叉核对 SQL、encoded domain、result blob 与 identity。 |
| Graph 13 维 | `SubmitGraphRunRequest` 必须提供 exact Subject binding，server 读取 SubjectRepository 并核 owner/version/hash；ReproducibilityIdentity 加 Subject/Code，compare 的 13 维正负矩阵逐一只漂移一维，Result 仍独立。输出 Artifact identity 不含 run id/attempt。 |
| AC32 · intent 与 crash matrix | output 执行得到确定 payload 后、任何 stage 前写 durable typed intent；覆盖 before-stage、after-stage、after-promote/before-complete、complete rollback、after-commit/before-ack、lease expiry/restart。重放只能 reuse exact intent；hash/size/evidence/fence 漂移失败；每种 crash 的最终 bytes/hash/identity 与 baseline 相同且可见行计数均为 1。 |
| AC32 · orphan maintenance | Worker 生产组合实际运行有界 maintenance；候选按 tenant/hash/key/age 稳定扫描。staging、无 intent 的 promoted orphan 可在宽限后清除；active intent、已完成正式引用、仍在宽限内、DB 查询失败均不得删除。并发 complete/clean 由数据库行锁或等价原子判据保护。 |
| AC32 · 备份恢复 | `check-recovery.ps1` 创建隔离 source/restore project；backup manifest 绑定 PG dump、全部 immutable object bytes/hash/size、code/runtime。原 source DB/volume/bucket 被销毁；全新 destination restore 后，required-read Graph Artifact 和同步 Analytics record 逐 byte/evidence/identity 相同。对象遗漏、附加、篡改或身份漂移负例失败。 |
| AC33 · MANUAL | runner 验证 authority checkout clean且 HEAD 等于冻结 SHA、manifest 三文档 hash 全对；解析全部且仅限带 marker 的 `powershell` fence，拒绝未标记 PowerShell、duplicate id、placeholder 和 forbidden destructive/remote-release 命令。临时 clean public checkout、隔离 Compose project/ports/credentials中按顺序原文执行 dev-up/down、fast/full/integration 与 recovery-proof。 |
| 回归与门禁 | fixed Buf format/lint、双新生成树、descriptor 与三语言 consumers；R4–R7A Oracle/Clang/双时间/源销毁回归；新增 formal evidence、13 维、crash/orphan/recovery/MANUAL tests；三个统一入口全部转绿。不得通过调整 Golden/Oracle/expected/容差或删除 evidence 字段制造通过。 |

RED-first 子循环按以下顺序执行，实施期保留首次真实非零命令、exit code、首个失败测试和首错；§6 最终只追加最终候选上的真实通过证据：

1. **Contract/domain RED：** 先增加 descriptor consumer 与 canonical identity tests，使缺少 evidence proto、Subject/Code 维度、排序/漂移判据失败，再实现公共 contract 与单一 domain builder。
2. **Analytics RED：** 逐服务增加“成功必须 evidence + 持久化、漂移改变 identity、repository write 失败不返回成功”的测试；先证明现有响应无 code/image/result identity，再接入正式 publisher。
3. **Graph RED：** exact Subject/Code、13 维与 full trace tests 先失败；再贯穿 submission/runtime/storage/API/Artifact/SignalSet，legacy full trace 单独失败关闭。
4. **Recovery RED：** 先让无 intent 的旧 Worker 在 after-promote 等 crash 点产生不可证明状态，并证明 OrphanCleaner 未生产组合；再增加 intent、原子 complete、maintenance 与全部 fault injections。
5. **DR / MANUAL RED：** 先以 object tamper、authority SHA drift、未标记 block 与命令文字 drift 证明 runners 失败，再执行真实 source-destroy/fresh-restore 和 clean-checkout literal run。
6. **回归：** 更新生成物与强绑定，运行 focused→fast→full→integration；最终候选只在 clean Git 状态和冻结路径审计完成后填写真实证据。

## 3. 非目标

- 不创建或接入 Portfolio360/COGA WebApp；不增加 Portfolio/Book/NAV/P&L/归因/VaR/优化等新产品域。
- 不实施 v0.2 Policy/Constraint、GeneratedNode、AI 沙箱、Python node runtime、DMQuant、完整 DataHealth 扩展或新的金融模型。
- 不把 Definition、Fact、Snapshot CRUD 读取包装成分析正式输出；不新增客户端任意 Artifact/SignalSet payload publish、presigned upload/download 或第二数据权威。
- 不宣称 PostgreSQL 与 Ceph 构成分布式事务；不建立生产 HA、PITR、跨地域容灾、定时备份运营、RPO/RTO、目标服务器操作或远程恢复。
- 不修改 private authority commit `3e028bcadd9fe973450d35b092e03a7485a54881`、ignored `SPEC.md`/`ACCEPTANCE.md`/`MANUAL.md`、Golden/Oracle/expected/数值容差或 R7A core manifest。
- 不修改远端 GitHub 权限/安全/branch protection、workflow 或 CICD 平台；不创建、移动或删除版本 tag，不发布镜像，不部署。没有 Human 版本号，因此不使用 CICD。

## 4. 公共契约变化

新增 `ficant/core/v1/evidence.proto`：

- `FormalInputKind` 是封闭类型枚举；`FormalInputBinding` 固定携带非空稳定 role、kind、exact owner，以及二选一的 exact reference：具有对象身份/version/content hash 的 `LineageRef`，或具有稳定字符串 identity/content hash 的 `NamedContentRef`；并可携 observed/visible/effective 时间。Subject 必须为 versioned object ref；Snapshot/Artifact/Position 等按 kind 强制正确 version shape；FactorDefinition 与 CurveNodeDefinition 必须使用 named ref，禁止伪造 ULID。
- `CodeBinding` 固定携带 40 位小写 Git commit、40 位小写 Git tree 与 domain-separated SHA-256；`RuntimeBinding` 携实际 image config SHA-256 与 environment digest；`FormalImplementationBinding` 以稳定 role 绑定实现 digest。
- `FormalOutputEvidence` 固定携 schema id、exact Subject、稳定排序 inputs、Code、Runtime、稳定排序 implementations、parameters hash、optional seed、result hash 与 output identity。identity 采用公共 domain v1 canonical algorithm，消息内 claimed identity 必须重算相等。

现有契约只做加法变化：

- Rates `ResultMetadata.formal_evidence = 11`。
- `PortfolioKeyRateExposure.formal_evidence = 11`；`PositionViews.formal_evidence = 6`；`CapitalUse.formal_evidence = 6`；`DataHealthReport.formal_evidence = 18`。
- `Artifact.formal_evidence = 8`；`SignalSet.formal_evidence = 11`；legacy persisted values可缺该字段，但 full-formal lineage 不接受缺失。
- `ReproducibilityIdentity.subject = 12`、`code = 13`；`NodeOutputBinding.formal_evidence = 5`。`SubmitGraphRunRequest.subject = 12`。`GraphOutputTrace.formal_outputs = 4`。
- `GraphRunComparisonDimension` 保留 1–11，新加 `SUBJECT = 12`、`CODE = 13`；不重排旧值。

Server 新增只读 `FICANT_CODE_COMMIT_SHA`、`FICANT_CODE_TREE_SHA`、`FICANT_SERVER_RUNTIME_IMAGE_DIGEST`、`FICANT_SERVER_ENVIRONMENT_ATTESTATION`；Worker 新增同一 Code 两字段。二进制的 compiled Code 必须与设置相等，所有 digest 只由部署/构建侧注入，客户端请求没有对应字段。Worker orphan grace/interval 使用有界正整数设置，不能由请求覆盖。

## 5. 需 Human 决策

Human 已以两次“批准”确认 private authority 的 R7B 合同；下列选择据此冻结。任何语义、公共字段、允许路径或 protected fact 变化都必须在首次相关写入前重新取得 Human 明确授权。

| 决策 | 冻结选择 | 排除边界 |
|---|---|---|
| D1 · 正式输出范围 | 五 Rates、Portfolio KRD、PositionViews/成功 CapitalUse、DataHealthReport、Experiment Artifact/SignalSet。 | 不把 CRUD 读取或未来 Portfolio360 输出纳入。 |
| D2 · 证据与身份 | typed role + exact Subject/input + Code commit/tree + actual Runtime + environment/implementation + parameters/seed + result hash；单一 canonical v1 identity。 | 不接受自由文本 lineage、客户端自报 code/image、只含 request fingerprint 或运行 id。 |
| D3 · Graph 对齐 | Submission required-read Subject；Reproducibility 加 Subject/Code；比较固定 13 维；Artifact/SignalSet正式 evidence 持久化。 | 不保留“Analytics 严格、Graph 只有旧 11 维”的双重标准。 |
| D4 · 同步持久化 | 正式 Analytics 成功前先以 identity 幂等持久化 canonical payload/evidence；失败不返回成功。 | 不以日志、trace 或仅内存 metadata 代替可恢复事实。 |
| D5 · crash recovery | payload 确定后、任何 stage 前 durable intent；complete 同事务消费；生产 worker实际运行 orphan maintenance。 | 不声称两存储原子事务，不允许 cleaner 仅存在于测试。 |
| D6 · 灾备 | 隔离 PG/Ceph 的 dump + 完整 object manifest，销毁 source，fresh restore 后两类正式输出 required-read。 | 不宣称 HA/PITR/RPO/RTO 或操作目标服务器。 |
| D7 · MANUAL | private MANUAL 先批准；public runner绑定 exact authority commit并在临时 clean checkout 原文执行 marker blocks。 | 不复制 MANUAL 进公共仓库，不把命令重写成等价测试，也不把 ListOnly 当通过。 |
| D8 · 交付 | R7B 公共候选完成后再提交/合并并由独立 post-merge authority 点亮 AC30–AC33；本轮无版本交付。 | 不创建 tag、发布镜像、部署或修改远端治理。 |

### Human 批准的实施扩展（2026-08-19）

Human 已明确批准以下 contract 语义修正：新增 `NamedContentRef { identity, content_hash }`；`FormalInputBinding` 以 `oneof reference` 在 `object_ref = 4` 与 `named_ref = 9` 之间二选一，时间字段继续占用 5–8；`FormalInputKind` 新增独立的 CurveNodeDefinition kind。FactorDefinition 与 CurveNodeDefinition 只允许 named ref，其余对象类型按 kind 使用 exact object ref；不得为字符串身份伪造 ULID。

Human 同时批准在下方原始冻结闭集之外追加四个实施路径；原始列表保持不变：

- `crates/ficant-storage/tests/support/mod.rs`：测试 reset 必须删除 R7B 新增的 `analytics` schema。
- `binaries/ficant-server/tests/data_source_registry_sit.rs`：只允许为新增 required Code/Runtime 设置机械迁移 fixture。
- `binaries/ficant-server/tests/factor_registry_sit.rs`：只允许为新增 required Code/Runtime 设置机械迁移 fixture。
- `binaries/ficant-server/tests/r6a_governed_input_sit.rs`：只允许为新增 required Code/Runtime 设置机械迁移 fixture。

这四个路径不得改变既有业务断言；任何进一步扩展仍需 Human 在首次写入前明确批准。

### Human 批准的 Phase 2E fixture 扩展（2026-08-20）

Human 明确批准追加 `crates/ficant-api/tests/phase2e_sdk_live.rs`，且仅允许把既有 live Python SDK parity fixture 从无正式输出仓储的 legacy Rates 构造器机械迁移到 `FormalOutputPublisher` 生产构造器；不得改变既有业务输入、数值断言或 Python SDK parity 判据。

### Human 批准的集成 fixture 隔离扩展（2026-08-20）

Human 明确批准追加 `crates/ficant-acceptance/tests/phase1_business_loop.rs`、`crates/ficant-acceptance/tests/negative_invariants.rs`、`crates/ficant-data/tests/dual_source_sit.rs` 与 `crates/ficant-data/tests/snapshot_publication_sit.rs`，且仅允许在数据库重置夹具中清理 R7B 新增的 `analytics` schema。已在允许闭集内的 `binaries/ficant-server/tests/r6a_governed_input_sit.rs` 与 `binaries/ficant-server/tests/r6b_artifact_service_sit.rs` 同步执行同一机械修正；不得改变任何业务输入、业务断言或验收判据。

## 6. 最终真实测试证据

**R7B execution base：** public `34402344c7d2c9238dc171af52ac4db77eb6b462`，Git tree `f66e03c55703837d6f2aee9959eba482612272f1`。**Human-approved authority：** private PR #23 rebase-merged commit `3e028bcadd9fe973450d35b092e03a7485a54881`；`verify-authority.ps1` exit `0`，三份文档完整且 manifest 仍绑定 execution base。公共实施从独立分支 `agent/r7b-evidence-recovery` 开始。

**实施允许写路径（冻结闭集）：**

- `docs/iterations/2026-08-r7b-evidence-recovery.md`（本文件；执行后只允许在本节追加真实最终证据和第 7 节残余风险）
- `docs/iterations/README.md`
- `README.md`
- `docs/development.md`
- `docs/product/scope.md`
- `docs/quality/evidence.md`
- `docs/architecture/layering-refactor.md`
- `docs/architecture/data-dictionary.md`
- `docs/architecture/adr/0016-analytics-service-as-first-class-execution.md`
- `docs/operations/recovery.md`（新建）
- `Cargo.lock`
- `interface/proto/ficant/core/v1/evidence.proto`（新建）
- `interface/proto/ficant/rates/v1/analytics.proto`
- `interface/proto/ficant/research/v1/artifact.proto`
- `interface/proto/ficant/research/v1/execution.proto`
- `interface/proto/ficant/research/v1/experiment.proto`
- `interface/proto/ficant/research/v1/exposure.proto`
- `interface/proto/ficant/research/v1/health.proto`
- `interface/proto/ficant/research/v1/position.proto`
- `interface/proto/ficant/research/v1/signal.proto`
- `crates/ficant-contracts/src/generated/**`
- `python/node-contracts/src/ficant_contracts/generated/**`
- `web-dm/packages/contracts-generated/src/**`
- `crates/ficant-contract-tests/Cargo.toml`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contract-tests/tests/r7b_formal_evidence.rs`（新建）
- `python/tests/test_contract_import.py`
- `web-dm/platform-shell/tests/contracts-consumer.test.ts`
- `.github/scripts/license-inventory.lock.json`（只允许既有工具机械 refresh source bindings）
- `.github/scripts/verify-contract-generation.sh`（只允许更新最终 descriptor digest）
- `crates/ficant-domain/src/lib.rs`
- `crates/ficant-domain/src/primitives/mod.rs`
- `crates/ficant-domain/src/primitives/formal_evidence.rs`（新建）
- `crates/ficant-domain/src/research/mod.rs`
- `crates/ficant-domain/src/research/artifact.rs`
- `crates/ficant-domain/src/research/signal_set.rs`
- `crates/ficant-domain/src/research/experiment_run.rs`
- `crates/ficant-runtime/src/lib.rs`
- `crates/ficant-runtime/src/native_execution.rs`
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/ports/blob_store.rs`
- `crates/ficant-application/src/ports/fingerprint.rs`
- `crates/ficant-application/src/ports/artifacts.rs`
- `crates/ficant-application/src/ports/signals.rs`
- `crates/ficant-application/src/ports/phase4_execution.rs`
- `crates/ficant-application/src/ports/formal_outputs.rs`（新建）
- `crates/ficant-application/src/use_cases/mod.rs`
- `crates/ficant-application/src/use_cases/phase4_submission.rs`
- `crates/ficant-application/src/use_cases/formal_outputs.rs`（新建）
- `crates/ficant-application/tests/r7b_formal_evidence.rs`（新建）
- `migrations/postgresql/0025_r7b_formal_evidence_recovery.sql`（新建）
- `crates/ficant-storage/src/postgres/mod.rs`
- `crates/ficant-storage/src/postgres/common.rs`
- `crates/ficant-storage/src/postgres/codec.rs`
- `crates/ficant-storage/src/postgres/artifacts.rs`
- `crates/ficant-storage/src/postgres/signals.rs`
- `crates/ficant-storage/src/postgres/phase4_execution.rs`
- `crates/ficant-storage/src/postgres/formal_outputs.rs`（新建）
- `crates/ficant-storage/src/s3/mod.rs`
- `crates/ficant-storage/src/s3/content_addressed.rs`
- `crates/ficant-storage/src/s3/staging.rs`
- `crates/ficant-storage/src/s3/orphan_cleanup.rs`
- `crates/ficant-storage/tests/migration_acceptance.rs`
- `crates/ficant-storage/tests/phase4_execution_sit.rs`
- `crates/ficant-storage/tests/postgres_repository.rs`
- `crates/ficant-storage/tests/r7b_formal_outputs_postgres.rs`（新建）
- `crates/ficant-storage/tests/r7b_backup_restore_sit.rs`（新建）
- `crates/ficant-api/Cargo.toml`
- `crates/ficant-api/src/lib.rs`
- `crates/ficant-api/src/formal_evidence.rs`（新建）
- `crates/ficant-api/src/rates.rs`
- `crates/ficant-api/src/portfolio_risk.rs`
- `crates/ficant-api/src/position_snapshot.rs`
- `crates/ficant-api/src/data_health.rs`
- `crates/ficant-api/src/experiment.rs`
- `crates/ficant-api/src/artifact.rs`
- `crates/ficant-api/tests/rates_service.rs`
- `crates/ficant-api/tests/portfolio_risk_service.rs`
- `crates/ficant-api/tests/position_snapshot_service.rs`
- `crates/ficant-api/tests/data_health_service.rs`
- `crates/ficant-api/tests/experiment_grpc.rs`
- `crates/ficant-api/tests/artifact_service.rs`
- `crates/ficant-api/tests/r7b_formal_outputs.rs`（新建）
- `binaries/ficant-server/Cargo.toml`
- `binaries/ficant-server/build.rs`（新建）
- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/src/main.rs`
- `binaries/ficant-server/tests/composition.rs`
- `binaries/ficant-server/tests/service_topology.rs`
- `binaries/ficant-server/tests/rates_sit.rs`
- `binaries/ficant-server/tests/portfolio_risk_sit.rs`
- `binaries/ficant-server/tests/position_snapshot_sit.rs`
- `binaries/ficant-server/tests/data_health_sit.rs`
- `binaries/ficant-server/tests/r6b_artifact_service_sit.rs`
- `binaries/ficant-server/tests/r7b_formal_evidence_sit.rs`（新建）
- `binaries/ficant-worker/Cargo.toml`
- `binaries/ficant-worker/build.rs`（新建）
- `binaries/ficant-worker/src/lib.rs`
- `binaries/ficant-worker/src/main.rs`
- `binaries/ficant-worker/src/production.rs`
- `binaries/ficant-worker/src/tests.rs`
- `binaries/ficant-worker/tests/phase4_worker_sit.rs`
- `binaries/ficant-worker/tests/support/mod.rs`
- `binaries/ficant-worker/tests/r7b_publication_recovery_sit.rs`（新建）
- `deploy/dev/RustService.Dockerfile`
- `deploy/dev/docker-compose.yml`
- `scripts/dev-up.ps1`
- `scripts/dev-down.ps1`
- `scripts/check-common.ps1`
- `scripts/check-fast.ps1`
- `scripts/check.ps1`
- `scripts/check-manual.ps1`（新建）
- `scripts/test-manual-check.ps1`（新建）
- `scripts/check-recovery.ps1`（新建）
- `scripts/test-recovery-check.ps1`（新建）
- `tests/recovery/**`（新建的隔离 compose/manifest/tamper fixtures only）

**受保护事实：** private authority commit 与三件套只读；R7A 的 47-file core manifest、所有 Golden/Oracle/expected/容差、C/C++/FFI/native 数值实现、RulePack 内容、tax/funding/cross-Clang fixtures、`.github/workflows/**`、`cicd.yml`、`deploy/test/**`、远端 GitHub 设置、版本/tag/镜像/部署均不修改。公共根目录 ignored authority 与原 `C:\git\ficant` 未跟踪审计报告不读写。

本节以下证据全部来自同一个最终代码候选，不把计划命令、早期候选或文档提交写成代码已通过。**最终代码候选：** commit `6d6c5d887011a69cc30d67b7164c9d2f3a44ec27`，tree `05020b19189be7ba7f5d19b51f4d7e9fe0a1026b`；其父链线性落在冻结 execution base `34402344c7d2c9238dc171af52ac4db77eb6b462` / tree `f66e03c55703837d6f2aee9959eba482612272f1` 上。最终 Human brief 的后续子提交只修改本文件；该文档提交不冒充重新执行完整 MANUAL 的代码候选。

### 6.1 Authority、clean checkout 与 MANUAL

最终执行使用 Node `v22.17.0`，设置 `PYTHONDONTWRITEBYTECODE=1`，并实际运行：

```powershell
pwsh -NoProfile -NonInteractive -File scripts/check-manual.ps1 `
  -AuthorityRoot 'C:\git\ficant-authority-r5d' `
  -ExpectedAuthorityCommit '3e028bcadd9fe973450d35b092e03a7485a54881' `
  -ExpectedPublicCommit '6d6c5d887011a69cc30d67b7164c9d2f3a44ec27'
```

命令 exit `0`，并输出 `FICANT MANUAL literal clean-checkout execution passed.`。runner 验证 private authority 工作树 clean、HEAD 精确等于批准 SHA、三件套 manifest 完整；随后创建 exact public commit/tree 的临时 clean checkout，按 private MANUAL 顺序原文执行六个 literal block：`dev-up`、`dev-down`、`check-fast`、`check-full`、`check-integration`、`recovery-proof`。Web 依赖准备使用 `corepack pnpm@10.12.4 install --offline --frozen-lockfile`，结果为 178 个包 reused、0 downloaded；依赖准备后 checkout 仍无 tracked/untracked drift。隔离开发拓扑中的 PostgreSQL、Ceph RGW、Server、Worker、UI 均健康，UI 使用动态隔离端口 `57337`，Platform Shell 与真实 gRPC-Web session 验证通过；literal `dev-down` 删除容器且按合同保留命名卷，runner 最终删除隔离容器、卷、网络与临时 checkout。清理过程曾输出一次延迟目录删除警告，但进程 exit `0` 后复核临时根目录不存在、Git worktree 无残项、相关 Docker 容器/卷均为 0。

### 6.2 本地检查与测试计数

| 实际入口/证据 | 结果 |
|---|---|
| `scripts/check-fast.ps1`（由 literal runner 原文执行） | exit `0`；descriptor coverage 68 个 reachable arms；coverage fixture 7 个真实负例加既有 6 个负例；MANUAL fixture 1 positive / 6 negative；recovery fixture 1 positive / 5 negative；R5D layering 3/3、R7A 2/2、R7B formal contract 2/2、R6B topology 3/3、storage lib 7/7、data 6/6 + snapshot codec 3/3，以及全部非环境门控 workspace tests/doc-tests通过。 |
| `scripts/check.ps1` | exit `0`；layering 51 个 fixture assertions；strict Clippy、workspace build 与全部非环境门控 tests/doc-tests通过；descriptor 20/20、R5D 3/3、R7A 2/2、R7B 2/2；C++ 9/9；Cross-Clang 71 行逐位相同，manifest SHA-256 `9d8699f60ab92943f8339ec2485f09396794c602b23d1835eae31eecb718929b`；Q001–Q036 36/36、Phase 2B 16/16、Phase 2C 18/18、Phase 2D 18/18；Decimal Oracles 3/3 + 3/3 + 3/3 + 13/13；license binding digest `081e05c91d8d1d458cf058c79997fbfb91b4ca14f281b81d31243c3c94472fdd`；Python generated consumer 1 passed / 1 environment-gated skipped，独立 Phase 2E live SDK 1/1；Web build 181 modules、tests 35/35。 |
| `scripts/check.ps1 -IncludeIntegration` | exit `0`；PostgreSQL migrations 7/7、lease 1/1、execution closure 3/3、production Worker 1/1、Phase 1 业务闭环 1/1、negative invariants 13/13；Carry/Delivery/Hedge、DataSource、双源一致性、immutable snapshot、R6A、R6B 各 1/1；集成内 recovery source/restore required-read 共 4 次 1/1，manifest SHA-256 `FB6D8A5789DBBEA4A8FAD69371A434A91D07C386D3C8272B22BF6F7A99112D13`。 |
| 独立 literal `recovery-proof` | exit `0`；source 基线 required-read 两次、销毁 source 容器/数据库卷/Ceph 卷与 bucket、在不同 fresh restore project 恢复后 required-read 两次，四次均 1/1；Graph Artifact 与同步 Analytics 输出 bytes/evidence/identity 逐位一致；最终 manifest SHA-256 `482FE692CC596B10D8B1FC57EAA416D74E1753CCB343A972CEEBE463B282570D`。 |

Crash matrix 与生产清理器证据包含 before-stage、after-stage、after-promote/before-complete、complete rollback、after-commit/before-ack、lease expiry/restart；每个恢复路径都核对最终仅一个可见 Artifact、一个 terminal result，以及与不中断基线相同的 bytes/hash/formal identity。篡改 blob、漂移 intent/fence、错误 schema/identity 和缺失对象均在 required-read 或完成事务前失败关闭；active intent、正式引用、宽限期内对象以及数据库判据失败时不执行 orphan 删除。

### 6.3 契约生成与候选范围

在上述代码 commit 上实际运行 `.github/scripts/verify-contract-generation.sh`，exit `0`。两个 fresh generation tree、当前 Rust/Python/TypeScript consumers 与 descriptor 校验通过；生成基线摘要为 `6c805930f201b3d82bbcbee9030b791e48fb08e7`，descriptor SHA-256 为 `01f938418b6d3649a71952051173f56dc635994c5515c9700eec5173f446c428`。Rust consumer 20/20 + 3/3 + 2/2 + 2/2、Python focused consumer 1/1、固定 Node/pnpm TypeScript focused consumer 1/1 均通过。

最终范围审计相对 execution base 统计 117 个变更路径：117 个均命中本节冻结闭集或 §5 的 Human 批准扩展，unexpected `0`；protected paths `0`，其中 `.github/workflows/**`、`cicd.yml`、`deploy/test/**`、Golden/Oracle/expected/容差、C/C++/FFI/native 数值实现和 R7A core manifest 均无差异。`git diff --check` exit `0`，代码候选工作树 clean，generated Python `__pycache__`/`.pyc` 数量 `0`，MANUAL/recovery 临时 worktree、容器和卷残留均为 `0`。

### 6.4 实施期失败证据与 forward-only 修复

实施期真实暴露并修复了六类问题：Rust service image 未包含 first-party domain packs；开发拓扑缺 bootstrap/input 配置且 `dev-down` 依赖未保留的瞬态变量；UI 健康探针使用固定端口而隔离 runner 分配动态端口；一次 Docker Desktop engine 基础设施中断；MANUAL fixture 受 CRLF 影响；clean checkout 没有 `node_modules` 且原入口未定义离线依赖准备。对应修复分别补齐镜像构建上下文、完整本地拓扑参数与 compose 绑定、按实际隔离端口探测、基础设施恢复后在同一候选重跑、让 fixture 对行尾稳定，以及提交 lockfile 并强制离线 frozen install。最终测试没有降低 validator、Golden、Oracle、expected、容差、负向断言或正式 evidence 要求。

## 7. 残余风险

- AC30–AC33 当前仍未点亮；只有本 brief 的技术候选完成、进入公共 `main`，再由 private authority post-merge 精确绑定后才能改变 `26 / 30` 状态。
- R7B 的 recovery 目标是可证明的 crash consistency 与本地灾备协议，不是分布式事务、生产 HA、PITR、跨地域容灾或已运营的备份服务。
- 通用 Python live SDK 测试仍保留环境门控，因此完整检查显示 1 skipped；专用 Phase 2E 真实 server parity 已独立 1/1 通过，不能把前者宣传为常驻在线环境测试。
- 契约 fresh generation 使用 BSR remote plugins，匿名配额曾在实施期短暂返回 `resource_exhausted`；最终双树/consumer/descriptor 已通过，但未来重复取证仍受远端服务与配额可用性影响。
- CICD 版本候选的正式镜像构建、签名、发布和部署身份仍由未来 Human 明确版本号后使用 `$cicd` 处理；本轮未创建 tag、未发布镜像、未部署、未触发远端 CI/CD，也未修改 GitHub 治理设置。
