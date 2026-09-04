# ficant 架构与数据字典

**状态（2026-09-04）：** 已同步公共 `main` 上 Phase 0–4、R5D/R5E、R6A/R6B、R7A/R7B 与 R8A/R8B 的架构字典；后续规划能力不视为已实现

**权威边界：** private authority manifest 绑定的 `SPEC.md` 定义规范；`README.md` 只提供非权威技术基线与背景，`interface/` 定义跨边界字段，Rust Domain/Application 定义业务不变量，Migration 定义持久映射；架构选择与依据记录在 `docs/architecture/adr/`

## 模块与依赖方向

```text
web-dm/* / Python consumer / Agent Tools
                    ↓ 统一 interface/ Protobuf + gRPC-Web/gRPC
Rust API ─────────→ Application ─────────→ Domain
   │                    │                    ↑
   └→ Contracts         └→ Runtime ──────────┘
                            ↑
Storage adapters/codecs ─→ narrow Application ports ─→ Domain

External file / PostgreSQL
          ↓ fixed raw quote contract
ficant-data adapters → point-in-time + mapping + quality → Canonical Arrow RecordBatch
       → deterministic Parquet + canonical Manifest
       → Application dual-blob publication → PostgreSQL + Ceph RGW DataSnapshot
       → required read + three-way verification → Canonical Arrow RecordBatch

2026-07 iteration-3 目标边界（现已落地，保留为依赖方向说明）：
Application → BondAnalyticsEngine port ← ficant-fixed-income-native adapter
                                              ↓
                                       ficant-kernel-sys（唯一 unsafe）
                                              ↓ C ABI
                                       C++20 fixed-income-kernel
ficant-worker composition root ──显式注入──→ native adapter
Application → BondAnalyticsArtifactCodec port ← Storage Arrow codec
```

- Rust `domain` 不依赖数据库、网络、文件系统或 Web 框架。
- Application 持有授权、opaque proof、幂等 fingerprint 和事务意图，并调用纯 Rust Runtime policy；Runtime 只依赖 Domain。
- Storage 实现 Application ports，并使用 Domain 类型完成持久映射；Application/Domain 不反向依赖 Storage。
- API 依赖 Application 与机械生成的 Contracts；跨边界字段仍只由 `interface/` 定义。
- Python 不进入平台主进程，也不直接访问数据库、密钥、对象存储或 RunJournal。
- WebApp 代码和设计位于 `web-dm/webapps/<app-id>/`，共享宿主位于 `web-dm/platform-shell/`；后台合同只在根 `interface/` 定义。
- 多语言目录所有权见 [ADR-0001](adr/0001-polyglot-monorepo-source-ownership.md)；数值内核、唯一 unsafe crate 和派生结果语义见 [ADR-0002](adr/0002-fixed-income-kernel-and-ffi-safety-boundary.md)。
- 所有内部模块按 [ADR-0003](adr/0003-deep-modules-and-explicit-internal-boundaries.md) 声明职责、数据/错误所有权、允许/禁止依赖和 composition root；复杂性必须封装在模块内部。

## Phase 1 核心对象

| 对象组 | 已实现对象 | 关键不变量 |
|---|---|---|
| Definition | `Instrument`、`Bond`、`FuturesContract`、`Calendar`、`Unit`、`MarketRulePack` | ID + 正整数版本；修改追加新版本；支持精确版本、as-of 和稳定 cursor 分页 |
| Market Fact | `Cashflow`、`Quote`、`Trade`、`Valuation`、`CurveSnapshot` | 绑定精确 Instrument/Unit/来源 revision；更正追加新 revision，不覆盖旧事实 |
| Snapshot | `DataSnapshot`、`UniverseSnapshot` | 内容寻址、所有者和非空血缘；发布后不可变 |
| Run/Evidence | `ExperimentRun`、`RunJournal` | 固定 Snapshot、RulePack、镜像、参数与 seed；Journal 追加且可重放 |
| Publication | `Artifact`、`SignalSet` | 不同根 ID；内容哈希、大小、类型、所有者与完整血缘一致 |

以上共 17 个 README Phase 1 对象；`ficant.app.v1` 的 Platform Shell 会话/Registry 合同不计入该数字。

## Snapshot、Artifact 与 SignalSet

| 对象 | 正式对象存储角色 | 语义 |
|---|---|---|
| `DataSnapshot` | `data_parquet` + `data_manifest` | `blob_content_hash` 指向数据内容，`manifest_hash` 指向 Manifest；二者必须分别有 verified durable ref；`schema_hash` 只是摘要 |
| `UniverseSnapshot` | `universe_members_manifest` | `content_hash` 指向成员 Manifest；Instrument version 必须非空、排序且唯一 |
| `Artifact` | `artifact_payload` | 记录 kind、media type、hash、非零 size 和完整 lineage |
| `SignalSet` | `signal_payload` | 使用独立 ID，通过 content-addressed lineage ref 指向 kind=`SignalSet` 的 Artifact；二者共享同一已验证 payload，但不是同一对象 |

SignalSet 除承载 Artifact 自身引用外的 lineage 集合必须与持久化 Artifact 的完整 lineage 一致；Snapshot、Run、RulePack、tenant、owner、hash 或 size 任一漂移都 fail closed。

## 正式输出证据与身份

R7B 的 `FormalOutputEvidence` 是同步 Analytics 与异步 ResearchGraph 共用的正式输出信封，不是任意 CRUD 响应 metadata。当前范围固定为五个 Rates 结果、Portfolio KRD、PositionViews、成功 CapitalUse、DataHealthReport、Artifact、SignalSet、R8A `PortfolioOverview`，以及 R8B `PortfolioPerformanceSeries`。Catalog、Definition 与 Fact 读取继续使用非正式 read evidence，不得伪装为正式分析。R8A 追加 `FormalInputKind` 16..21（Portfolio、Book、PortfolioGroup、Benchmark、PortfolioMetricConvention、Fact）；R8B 追加 22..24（PortfolioValuationSnapshot、BenchmarkLevelSnapshot、PortfolioPerformanceConvention）。0..21 不重排，canonical identity 算法不变。

| 组成 | 不变量 |
|---|---|
| `subject` | versioned Subject object ref；exact owner/version/content hash |
| `consumed_inputs` | 具有稳定 role 的封闭 kind；object 或 named content ref 二选一；严格排序且 role 唯一；适用时携 observed/visible/effective time |
| `code` | 40 位小写 Git commit/tree 与二者的 domain-separated digest；Server/Worker 编译值必须等于部署设置 |
| `runtime` | 实际 image config SHA-256 与 canonical environment digest |
| `implementations` | 按 role 稳定排序的实现摘要 |
| 参数与结果 | parameters hash、optional seed、result hash；result hash 必须等于 canonical payload bytes |
| `output_identity` | 对以上全部字段使用长度分隔、domain-separated canonical v1 bytes 重算；request/run/attempt/lease/clock 不进入 identity |

同步正式输出在成功响应前写入 `analytics.formal_outputs`，同 identity 的 bytes/evidence 漂移失败关闭。Graph Artifact/SignalSet 由发布命令单独携带同一 evidence，并以 `research.artifact_formal_evidence` 的规范化关联记录交叉核对受保护的 legacy domain payload；完整 trace 只展开正式 evidence，legacy Artifact 普通读取兼容但 full-formal trace 返回 `LineageIncomplete`。

Graph output 在任何 blob stage 前先写 `research.output_publication_intents`。intent 精确绑定 task/fence、Artifact、payload hash/size、manifest 与 formal evidence；complete 在同一 PostgreSQL 事务消费 intent。Ceph promote 与 PostgreSQL 不是分布式事务，超龄无 intent/无正式引用对象由有界 orphan maintenance 清理，active intent 与已引用对象不得删除。

## 单位与 Decimal

协议中的 Decimal 唯一表示为 `coefficient(string) + scale + UnitRef`，禁止隐式 float。Application 先读取同租户精确 Unit version，形成不可伪造的 resolved proof；Storage 在任何写入前用持久 Definition 再复核 dimension、scale 和有效 precision。

| Market Fact 字段 | 必须使用的 Unit dimension |
|---|---|
| Cashflow amount | `currency` |
| Quote bid / ask | `price` |
| Trade price | `price` |
| Trade quantity | `notional` |
| 省略 `value_roles` 的既有 Valuation | 全部 `price` |
| 显式 `PRICE` | `price` |
| 显式 `YIELD` | `rate` |
| 显式 `REMAINING_YEARS` | `years` |

省略角色的既有 Valuation 继续规范化为全部 `PRICE`，canonical bytes 与存储编码保持不变。新事实若显式携带角色，角色数必须与 `values` 完全相等且不得出现 `UNSPECIFIED`。本轮不引入换算、汇率或价格归一化。

## 组合目录与只读聚合

`Book`、`PortfolioGroup` 与 `Portfolio` 是 owner/Subject scoped 的不可变目录对象，不是第二套 Position 或会计账簿。`Portfolio` 通过 exact `PortfolioSnapshotBinding` 指向已有 `PositionSnapshot`。`BenchmarkRef` 与 `PortfolioMetricConvention` 都是版本化引用；R8A convention 只冻结点时加权口径。金额、比例、bp、久期和权重继续只走 `DecimalValue` + exact Unit。

## 组合日度计量与收益序列

| 对象 | 身份与时间 | 关键不变量 |
|---|---|---|
| `PortfolioPerformanceConvention` | owner scoped 的正整数 version + content hash；带 visible/effective time | exact Calendar；`DAILY_TIME_WEIGHTED`、`END_OF_DAY`、`CALENDAR_SESSION_CLOSE`、`TIES_TO_EVEN` |
| `PortfolioValuationSnapshot` | content-addressed snapshot；owner/Subject；valuation/visible time | exact Portfolio、PositionSnapshot、PerformanceConvention、currency Unit；`NAV = gross assets - liabilities`；只追加 |
| `BenchmarkLevelSnapshot` | content-addressed snapshot；owner/Subject；valuation/visible time | exact Benchmark、dimensionless Unit、正 level；只追加 |
| `PortfolioPerformanceSeries` | exact normalized scope、period、request fingerprint 与正式 output identity | Calendar 全 session/full-member required-read；先聚合 NAV/Flow 再计算；coverage expected=observed 且 missing 为空 |

相邻 session 的研究口径固定为 `P&L_t = NAV_t - Flow_t - NAV_{t-1}` 与 `R_t = P&L_t / NAV_{t-1}`；Flow 为期末外部现金流，累计收益逐步执行 `Π(1+R_t)-1`。金额、level 和比例均只用 scale-12 `FixedDecimal`/`DecimalValue` ties-to-even，不允许 float。它不代表正式估值关账、会计总账或交易流水生产链。

## RulePack 生效语义

`MarketRulePack` 采用显式精确版本和半开区间：

```text
effective_from <= subject_time < effective_to
```

- Valuation 的 subject time 是自身 `valuation_at`。
- ExperimentRun/Phase1 的 run market time 是所绑定 `DataSnapshot.as_of`。
- 不使用执行时钟、Journal 时间、Snapshot `visible_at`、Signal `valid_from` 或某笔 Trade 时间代替。

`MarketRulePack` 可携带一个带 `type_url` 的不透明内容载荷；只要载荷存在，`content_hash` 必须是其确定性 Protobuf bytes 的 SHA-256。Application 在任何可变 I/O 前解析 exact version 并形成 opaque proof；Storage 在事务第一步复核真实持久 RulePack 与区间。coverage miss 返回不可重试的 `ValidationFailed`，身份/版本/tenant/proof 漂移返回不可重试的 `LineageIncomplete`。

一般 RulePack 绑定仍不在 core 中执行内容。Phase 2C 的 `AnalyzeFuturesDelivery` 是明确例外：它在进入数值引擎前，从精确、授权且处于半开生效区间内的 `cgb-futures` RulePack 读取内容，复核 hash、market、rule type 和 type URL，并由 L3 parser 解析完整交割规则。缺失项以不可重试的 `ValidationFailed` 失败关闭；规则数值不留在 domain 或 C++ 默认表中。

## Run 与 Journal

- `ExperimentRun` 初始为 `Created/revision=1`，合法成功路径为 `Created(1) → Running(2) → Succeeded(3)`；`expected_revision` 指变更前 revision。
- Phase1 首次 Snapshot→Run 使用 candidate proof，把尚未提交的 DataSnapshot 精确绑定到 Run；独立 CreateRun 则只读取已持久 Snapshot。
- RunJournal 的 sequence 从 1 连续增长。sequence 1 的 `prev_hash` 必须 absent/`NULL`；后续事件必须引用前一事件的真实 hash，不能用零哈希代替 absent。
- 当前完整成功链包含 `RunCreated`、`RunStarted`、`ArtifactPublished`、`SignalSetPublished`、`RunSucceeded` 五个规范事件，可在重启后分页读取并确定性重放。

## Phase 1 事务与跨存储职责

Application 先完成 scope、owner、Unit、RulePack、Snapshot、Artifact/Signal 与 lineage 校验，再验证并 promote Ceph RGW staging 内容；随后提交一个 storage-owned PostgreSQL 事务：

```text
Market Fact
→ DataSnapshot + UniverseSnapshot metadata/durable refs
→ ExperimentRun + 两次状态转换
→ Artifact + SignalSet
→ 五条 RunJournal 事件
```

PostgreSQL 负责 tenant-scoped metadata、版本/revision、血缘、durable blob ref、幂等键、并发约束和这一业务单元的原子提交。Ceph RGW 负责按 SHA-256 内容寻址的不可变 bytes、staging/verify/promote 与 orphan 清理。不能把两种存储描述成一个分布式事务：PG 失败后的已 promote 但未引用对象由 orphan 机制回收，正式 metadata/Run/Journal 不产生半状态。

## 已发布内容读取

metadata/resource 不存在返回 `NotFound`。metadata 和 durable ref 已存在时，正式业务读取必须使用 non-optional required read：

| 读取 | 必须复核 |
|---|---|
| Artifact | tenant、owner、对象角色、ID、payload hash/size、lineage |
| SignalSet | 独立 Signal/Artifact 身份、Artifact kind、同一 payload、完整 lineage |
| DataSnapshot | data Parquet 与 Manifest 两个角色都成功 |
| UniverseSnapshot | members Manifest 成功 |

对象缺失、hash mismatch 或 size mismatch 都返回 `HashMismatch/retryable=false`，并恰好发出一次 `storage.published_content_integrity_failure` 结构化事件。事件只包含安全的 tenant、resource kind/id、blob role、expected hash/size、reason 和 trace context；不输出 bucket/key、credential、raw bytes、SQL 或 stack。无法判断完整性的 Ceph RGW/网络故障才是 `StorageUnavailable`。

`probe_verified` 仍可用于 orphan/reconciliation 和未发布探测，但不得进入已发布内容的正式业务读。

## 错误与授权

- scope 同时绑定 tenant、actor 和可访问 owners；所有 Definition、Fact、Snapshot、Run 和 Publication 必须在相同授权边界内。
- Application 稳定错误映射到 `ficant.core.v1.ErrorDetail`、gRPC code、retryable 与安全 trace；`StateConflict` 对外使用既有 `ImmutableViolation/FailedPrecondition`，不新增平行枚举。
- 浏览器 Shell 的 `ficant.app.v1.SafeError` 是 Platform Service 的安全错误信封；它与 Phase1 core business error mapper 各自服务不同接口边界，不应混写。

## Phase 2 与后续边界

iteration-3 的小范围 Phase 2A 已实现固定利率和贴现国债的现金流、应计利息、净价、全价、到期收益率（YTM）、麦考利久期、修正久期、凸性和 DV01，并贯通 C++20 内核、稳定 C ABI、安全 Rust adapter、确定性 Arrow 编码及 PostgreSQL/Ceph RGW 发布与重放的内部 Artifact 链。

该切片不改变外部事实语义：平台生成的现金流与估值风险结果不得写成现有 `Cashflow` 或 `Valuation`。它们使用内部 `BondAnalyticsResult` 并作为内容寻址 Artifact 发布，绑定 Bond、MarketRulePack、估值时点、输入快照、算法版本和 ABI 版本；详见 [ADR-0002](adr/0002-fixed-income-kernel-and-ffi-safety-boundary.md)。

2026-07 Phase 2B 新增内部 `YieldCurveBinding` / `CarryRollInput` / `CarryRollResult` 语义：曲线绑定独立 `CurveSnapshot`，按实际日数在冻结 YTM 节点间线性插值且不外推；持有期结果绑定 owner、Bond、CurveSnapshot、MarketRulePack、DataSnapshot、估值日、起止日、算法/约定/ABI 版本，并作为确定性 Arrow Artifact 发布。输入、血缘、hash 或 size 漂移均 fail closed。

2026-07 Phase 2C 新增内部 `CgbFuturesProduct` / `FuturesDeliveryRule` / `FuturesDeliverableInput` / `FuturesDeliveryMeasures` / `FuturesDeliveryBasket` 语义：输入绑定 owner、FuturesContract、Bond、MarketRulePack、DataSnapshot、估值/购入/交割日期、算法/约定/ABI 版本。交割专用 L3 parser 从精确 `cgb-futures` RulePack 解析期限资格、交割月份、标准票息、百元面值基准、舍入与年化日基准；这些规则进入输入 fingerprint，并由安全 Rust adapter 显式传递给 C ABI。生产内核从债券日程推导转换因子、应计利息和持有期票息，并输出交割发票价、基差、融资成本、净基差、IRR 与 CTD。结果使用独立确定性 Arrow schema 作为内部 Artifact 发布，输入、血缘、hash 或 size 漂移均 fail closed。该类型不是外部行情事实，也不包含期现套保比例、保证金或交易所交割流程。

Storage adapter 通过 Apache `object_store 0.14.1` 访问 S3，并以 Ceph RGW 20.2.2 作为受支持的服务端实现；bucket、endpoint、access key 和 secret key 仍由运行环境注入。`minio` 与 `async-std` 已从锁文件和可达依赖图移除，旧 D-026 限时接受不再是活动风险处置。开发与 CI 的单节点 Ceph 只验证 S3 兼容性、内容完整性、重启和业务闭环，不代表生产高可用拓扑；选择依据和升级条件见 [ADR-0010](adr/0010-ceph-rgw-and-apache-object-store.md)。

## Phase 3A 数据接入语义

`DataSource` 是带 tenant/owner 的版本化定义，当前只开放 `FileNdjson` 与 `Postgres`。领域对象只保存非敏感 connection binding 和逻辑 dataset；真实路径、数据库 URL 与凭据由 composition 注入，不进入对象、日志或错误文本。Storage 使用 `data.source_identities` 与 `data.sources` 保存 append-only identity/version，并复核授权、幂等 fingerprint 和连续版本。

`ficant-data` 独占 raw source row、Instrument 映射、市场会话、点时选择、质量规则与 Canonical Arrow 编码。点时读取同时要求 `observed_at <= as_of`、`visible_at <= visible_at_cutoff` 和 `as_of <= visible_at_cutoff`；Instrument mapping 使用 source identity、外部 key 和半开有效区间解析到 exact `VersionRef`，Calendar 与 Unit 同样绑定精确版本。

Canonical Quote Schema v1 固定为 16 列，schema ID 为 `ficant.market.quote.canonical.v1`，schema SHA-256 为 `e804a0becec18e51dde1be4250384ffe667cf4149c34dc3d2cfc82a206d71502`。行按 `(observed_at, instrument_id, source_record_id)` 稳定排序；任一 raw row 不满足唯一 ID、规范时间、可见性、映射、交易会话、双边存在性、bid/ask 顺序或 Decimal/Unit 规则时整批失败。进程内 RecordBatch 只有经过下述 Phase 3B 发布与验证边界后才是正式研究快照输入。

## Phase 3B 不可变快照语义

`ficant-data` 使用固定 Apache Arrow/Parquet Rust `59.1.0` writer，把 Canonical Quote batch 编码为一个未压缩、无 dictionary、writer version 2.0/data page v2、单 row-group Parquet 文件。writer 的 `created_by`、batch/page row limit、统计信息和 offset index 均冻结；相同输入产生完全相同 bytes、大小和 SHA-256。任何 schema metadata、列、nullable、行序或值漂移都失败关闭。

`ficant.data.snapshot-manifest.v1` 是字段顺序固定、无空白且以换行结束的 UTF-8 canonical JSON。它绑定 `DataSnapshot`/tenant/owner、Canonical schema ID/hash、Parquet hash/size/row count、as-of/visible cutoff/market timezone、DataSource exact version、Instrument mapping digest、Calendar/Unit exact version、实际 Instrument exact versions、质量计数和所有 writer 参数；不包含运行时当前时间或绝对路径。

Application 的 `PublishDataSnapshot` 在 I/O 前复核授权、非空 payload 和两个 domain hash，再复用 Phase 1 staging/promote、`VerifiedSnapshotProof::data` 与 `SnapshotRepository` 发布 metadata。正式消费必须先用 `VerifiedReadFacade::read_verified_snapshot` 取得两个 required payload，再由 `CanonicalSnapshotCodec::decode_verified` 校验 Snapshot/Manifest/Parquet 三方绑定、Parquet footer/schema/row group 和血缘；任一缺失、篡改或非 canonical Manifest 均不返回部分 batch。DataSource 已加入 PostgreSQL 血缘目标解析，因此 Snapshot 绑定的 source exact version 与 Calendar、Unit、实际 Instrument 一样由 storage 复核。

## Validity

Valid: long-term until superseded
