# ficant 架构与数据字典

**状态：** Phase 0 / Phase 1 已实现架构字典

**权威边界：** `README.md` 定义系统约束，`interface/` 定义跨边界字段，Rust Domain/Application 定义业务不变量，Migration 定义持久映射；架构选择与依据记录在 `docs/architecture/adr/`

## 模块与依赖方向

```text
web-dm/* / Python consumer / Agent Tools
                    ↓ 统一 interface/ Protobuf + gRPC-Web/gRPC
Rust API ─────────→ Application ─────────→ Domain
   │                    │                    ↑
   └→ Contracts         └→ Runtime ──────────┘
                            ↑
Storage adapters/codecs ─→ narrow Application ports ─→ Domain

iteration-3 目标边界：
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

## 单位与 Decimal

协议中的 Decimal 唯一表示为 `coefficient(string) + scale + UnitRef`，禁止隐式 float。Application 先读取同租户精确 Unit version，形成不可伪造的 resolved proof；Storage 在任何写入前用持久 Definition 再复核 dimension、scale 和有效 precision。

| Market Fact 字段 | 必须使用的 Unit dimension |
|---|---|
| Cashflow amount | `currency` |
| Quote bid / ask | `price` |
| Trade price | `price` |
| Trade quantity | `notional` |
| iteration-2 Valuation values | `price` |

其他 Valuation measure 在后续 Domain Pack 明确前拒绝；本轮不引入换算、汇率或价格归一化。

## RulePack 生效语义

`MarketRulePack` 采用显式精确版本和半开区间：

```text
effective_from <= subject_time < effective_to
```

- Valuation 的 subject time 是自身 `valuation_at`。
- ExperimentRun/Phase1 的 run market time 是所绑定 `DataSnapshot.as_of`。
- 不使用执行时钟、Journal 时间、Snapshot `visible_at`、Signal `valid_from` 或某笔 Trade 时间代替。

Application 在任何可变 I/O 前解析 exact version 并形成 opaque proof；Storage 在事务第一步复核真实持久 RulePack 与区间。coverage miss 返回不可重试的 `ValidationFailed`，身份/版本/tenant/proof 漂移返回不可重试的 `LineageIncomplete`。这里只验证绑定与生效区间，不执行规则内容，也不是 Phase 2 定价引擎。

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

2026-07 Phase 2B 新增内部 `YieldCurveBinding` / `CarryRollInput` / `CarryRollResult` 语义：曲线绑定独立 `CurveSnapshot`，按实际日数在冻结 YTM 节点间线性插值且不外推；持有期结果绑定 owner、Bond、CurveSnapshot、MarketRulePack、DataSnapshot、估值日、起止日、算法/约定/ABI 版本，并作为确定性 Arrow Artifact 发布。输入、血缘、hash 或 size 漂移均 fail closed。国债期货数值、可交割券、转换因子（CF）、基差、净基差、隐含回购利率（IRR）、最便宜可交割券（CTD）和套保算法仍未实现。

Storage adapter 通过 Apache `object_store 0.14.1` 访问 S3，并以 Ceph RGW 20.2.2 作为受支持的服务端实现；bucket、endpoint、access key 和 secret key 仍由运行环境注入。`minio` 与 `async-std` 已从锁文件和可达依赖图移除，旧 D-026 限时接受不再是活动风险处置。开发与 CI 的单节点 Ceph 只验证 S3 兼容性、内容完整性、重启和业务闭环，不代表生产高可用拓扑；选择依据和升级条件见 [ADR-0010](adr/0010-ceph-rgw-and-apache-object-store.md)。

## Validity

Valid: long-term until superseded
