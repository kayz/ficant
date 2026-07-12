# ficant 架构与数据字典

**状态：** Phase 0 / Phase 1 已实现架构字典

**权威边界：** `README.md` 定义系统约束，`interface/` 定义跨边界字段，Rust Domain/Application 定义业务不变量，Migration 定义持久映射

## 模块与依赖方向

```text
web-dm/* / Python consumer / Agent Tools
                    ↓ 统一 Protobuf + gRPC-Web/gRPC
Rust API → Application → Domain
             ↓ ports       ↑ 无基础设施依赖
PostgreSQL repositories + MinIO content-addressed store
                    ↓
C++20 stable C ABI（Phase 2 数值算法边界，本轮只有构建/ABI基线）
```

- Rust `domain` 不依赖数据库、网络、文件系统或 Web 框架。
- Application 持有授权、opaque proof、幂等 fingerprint 和事务意图；Storage 复核 proof 并执行持久化。
- Python 不进入平台主进程，也不直接访问数据库、密钥、对象存储或 RunJournal。
- WebApp 代码和设计位于 `web-dm/webapps/<app-id>/`，共享宿主位于 `web-dm/platform-shell/`；后台合同只在根 `interface/` 定义。

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

Application 先完成 scope、owner、Unit、RulePack、Snapshot、Artifact/Signal 与 lineage 校验，再验证并 promote MinIO staging 内容；随后提交一个 storage-owned PostgreSQL 事务：

```text
Market Fact
→ DataSnapshot + UniverseSnapshot metadata/durable refs
→ ExperimentRun + 两次状态转换
→ Artifact + SignalSet
→ 五条 RunJournal 事件
```

PostgreSQL 负责 tenant-scoped metadata、版本/revision、血缘、durable blob ref、幂等键、并发约束和这一业务单元的原子提交。MinIO 负责按 SHA-256 内容寻址的不可变 bytes、staging/verify/promote 与 orphan 清理。不能把两种存储描述成一个分布式事务：PG 失败后的已 promote 但未引用对象由 orphan 机制回收，正式 metadata/Run/Journal 不产生半状态。

## 已发布内容读取

metadata/resource 不存在返回 `NotFound`。metadata 和 durable ref 已存在时，正式业务读取必须使用 non-optional required read：

| 读取 | 必须复核 |
|---|---|
| Artifact | tenant、owner、对象角色、ID、payload hash/size、lineage |
| SignalSet | 独立 Signal/Artifact 身份、Artifact kind、同一 payload、完整 lineage |
| DataSnapshot | data Parquet 与 Manifest 两个角色都成功 |
| UniverseSnapshot | members Manifest 成功 |

对象缺失、hash mismatch 或 size mismatch 都返回 `HashMismatch/retryable=false`，并恰好发出一次 `storage.published_content_integrity_failure` 结构化事件。事件只包含安全的 tenant、resource kind/id、blob role、expected hash/size、reason 和 trace context；不输出 bucket/key、credential、raw bytes、SQL 或 stack。无法判断完整性的 MinIO/网络故障才是 `StorageUnavailable`。

`probe_verified` 仍可用于 orphan/reconciliation 和未发布探测，但不得进入已发布内容的正式业务读。

## 错误与授权

- scope 同时绑定 tenant、actor 和可访问 owners；所有 Definition、Fact、Snapshot、Run 和 Publication 必须在相同授权边界内。
- Application 稳定错误映射到 `ficant.core.v1.ErrorDetail`、gRPC code、retryable 与安全 trace；`StateConflict` 对外使用既有 `ImmutableViolation/FailedPrecondition`，不新增平行枚举。
- 浏览器 Shell 的 `ficant.app.v1.SafeError` 是 Platform Service 的安全错误信封；它与 Phase1 core business error mapper 各自服务不同接口边界，不应混写。

## Phase 2 与后续边界

本轮没有实现现金流生成、定价、收益率、久期/DV01、曲线插值、基差/IRR/CTD 或套保算法。现有 C++ 工程只证明固定 Clang/CMake/Ninja 构建和稳定 C ABI 基线。

Storage 的发布 Workspace/生产 adapter 当前经 `minio 0.4.0` 可达 `async-std 1.13.2`，并在 put 请求签名/内容处理路径实际使用其 blocking runtime；当前 `ficant-server`/`ficant-worker` 组合根尚未直接装配该 adapter。2026-07-13 复核确认 RustSec 项为 `INFO / unmaintained`、无 patched version；`minio 0.4.0` 是 crates.io 最新版且上游 `master` 仍依赖 `async-std 1.13`，因此没有安全的小版本升级路径。该项按 D-026 作为“当前安全风险低、维护风险中等”的 `accepted-unfixed` 收束，不是架构长期背书。Architecture/Delivery 必须在 iteration-3 Entry Gate、首次外部发布或 2026-10-13 前（最早者）验证上游移除、受控 fork 或受维护 S3 SDK 迁移；禁止自动继承到其他版本、依赖链、调用边界或发布范围。

## Validity

Valid: long-term until superseded
