# R1 迭代 brief — 分层门禁与主体契约

> 本文是本迭代面向 Human 的唯一文档。Agent 交流、Worker 证据、失败诊断与中间候选保留在编排工具与实际命令输出中，不进入本文。

**迭代** R1 · **状态** 待 Human 冻结 · **依据** SPEC v1.1、ACCEPTANCE v0.1（30 条）、[`../architecture/layering-refactor.md`](../architecture/layering-refactor.md)

**Base commit SHA：** `403ff701610b1494ed2b88073832d3f8a56304d1`

**依据的 ADR（全部 Accepted，2026-07-26）：** [0011](../architecture/adr/0011-position-as-snapshot-not-state.md) · [0012](../architecture/adr/0012-research-subject-identity-and-state.md) · [0013](../architecture/adr/0013-layering-law-shape-in-core-content-in-rulepack.md) · [0014](../architecture/adr/0014-policy-artifact-and-shared-evaluator.md) · [0015](../architecture/adr/0015-global-factor-identity.md) · [0016](../architecture/adr/0016-analytics-service-as-first-class-execution.md) · [0017](../architecture/adr/0017-data-health-and-coverage-declaration.md) · [0018](../architecture/adr/0018-platform-admin-and-researcher-separation.md)

---

## 1. 目标

**建立分层门禁，并冻结主体契约。**

R1 是唯一一轮以建立判据为主的迭代。它交付的主要不是能力，而是**此后每一轮都要通过的闸门**，以及主体这一个 L1 对象的最小可运行形态。

**Acceptance sentence（一句可验证）：**

> 在干净环境起完整开发拓扑后，可通过真实 gRPC-Web 注册一个 `Subject` 与一份 `SubjectStateSnapshot`，按 ID 查询回来得到逐字段相同的内容，且该主体的版本标识出现在一次 `AnalyzeBond` 调用的血缘中；同时 `check.ps1` 包含分层检查，其 allowlist 恰有一条 `futures_delivery` 条目。

**本轮主体只被携带，不参与计算。** 融资利率仍由调用方传入——那是 R3 的语义工作。本轮只证明管道通。

---

## 2. 验收

**本轮点亮：**

| 条目 | 内容 | 判定方式 |
|---|---|---|
| **AC03** | 全仓库不存在 `market == "CN"` 或等价市场分支 | 分层检查脚本纳入 `check.ps1`，命中即失败 |

**本轮建立的可执行判据但不点亮**（红灯是预期状态，由后续轮次转绿）：

| 条目 | 判据形态 | 转绿轮次 |
|---|---|---|
| AC01 | `ficant-domain` 内规则数值检索，`futures_delivery` 暂列 allowlist | R2 |

**后续轮次建立判据，R1 不宣称已建立：**

| 条目 | 所需判据 | 负责轮次 |
|---|---|---|
| AC02 | 换 RulePack 换结果的对照用例，以及缺失规则项的精确失败断言 | R2 |
| AC04 | 虚构市场 RulePack 与 Subject 的端到端计算、L0/L1/L2 改动行数统计 | R7 |

**本轮专属闸门（不进 ACCEPTANCE）：**

1. `subject.proto` 与 `subject_state.proto` 均能生成 Rust / Python / TypeScript 三侧类型，契约测试通过。
2. 每个新契约单文件不超过一页；超页即视为抽象过粗，退回重切。
3. `ficant-domain` 对 `ficant-storage`、网络、文件系统、模型服务的依赖仍为零。
4. 分层检查的 allowlist **恰有一条**条目，且其格式包含移除轮次（`R2`）。

---

## 3. 非目标

本轮**不得**做的事，做了即为越界：

- **不定义** `position.proto`、`factor.proto`、`health.proto`、`constraint.proto`、`policy.proto`——它们在 R4 / R5 / v0.2 的实现轮次定义。**提前定义是本轮最可能发生的越界。**
- 不实现主体的资金档、税收待遇解析**语义**——只冻结字段形状（R3）
- 不改动 `futures_delivery.rs` 的规则数值（R2）
- 不拆 `Bond.issue_date`（R3）
- 不改 canonical quote schema（R5）
- 不实现角色分离与白名单（R6）
- 不新增定价原语、不动 C++ 数值库
- 不建业务界面
- 不触碰 `.github/**`、`cicd.yml`、`deploy/**` 的发布边界

**不得修改的业务语义、Oracle、expected 与容差：** Phase 2A–2E 的全部 Golden Case 与独立 Oracle、Phase 3A/3B 的 canonical schema 哈希、Phase 4 的执行语义。本轮任何改动若导致它们变化，即为失败。

---

## 4. 公共契约变化

新增 `interface/proto/ficant/core/v1/` 下两个文件：

| 文件 | 消息 | 要点 |
|---|---|---|
| `subject.proto` | `Subject`、`SubjectVersion`、`SubjectRecord`、`AccessSet`、`FundingTier`、`TaxTreatment`、`ConstraintSetRef` | **只装身份**：准入集、资金档**可得性**、税收待遇、考核机制、约束集引用。可版本化。**引用而非内嵌任何数值** |
| `subject_state.proto` | `SubjectStateSnapshot`、`LimitCeiling` | **只装状态**：净资本、各项额度**上限**。走双时间通道，**不是版本**。额度**占用**由持仓算出，不在此存储（ADR-0012） |

新增服务（加法式）：`ficant.core.v1.RegistryService` 提供 `RegisterSubject` / `GetSubject` / `RegisterSubjectState` / `GetSubjectState`，需 `registry:read` / `registry:write` scope。

`AnalyzeBondRequest` 增加可选 `subject_ref` 字段；提供时进入结果血缘，**不改变任何计算数值**。这是本轮唯一触及既有契约的改动，且为加法式。

> **破坏性变更授权：** Human 已批准本轮及后续轮次可做破坏性契约变更（pre-1.0、无外部消费者）。本轮改动本身为加法式；R2–R5 将产生破坏性变更，届时须重跑既有取证。

---

## 5. 需 Human 决策

### 已裁决（2026-07-26）

- **✅ FactorId 命名 = `<市场>.<类别>.<经济量>.<期限>`**，全小写点分（如 `cn.gov.yield.10y`）。见 ADR-0015。契约本身推迟至 R4，**但命名规范现在就锁死**。
- **✅ Subject 版本粒度 = 身份版本化、状态快照化。** 授信与套保额度变化**不触发新版本**。见 ADR-0012。
- **✅ 未导入数据 = 不猜测。** 只按已有数据评估 + 覆盖度显式 + 健康度预警；不引入用户申报的外部占用字段。见 ADR-0017。
- **✅ 基础数据写入权 = 平台管理员。** 见 ADR-0018。
- **✅ 历史 brief 处置 = 移入 `docs/history/iterations/`。** 归档说明已就位，物理移动由 Human 以 `git mv` 执行以保留历史。
- **✅ SPEC diff 已批准并写入：** 新增不变量 I10（覆盖度显式）、§5 补入角色分离。
- **✅ 范围裁决：** Constraint / ShadowPrice / Policy 移至 v0.2，ACCEPTANCE 降至 30 条。

- **✅ ADR 0011–0018 状态 = Accepted。**
- **✅ base commit SHA 由 Root Orchestrator 自行冻结。** 开工第一步：确认工作区干净且与 `origin/main` 精确一致，取 `git rev-parse HEAD` 记为本轮 base，此后不得变更；若期间 `main` 前进，**不得移动 base**，另起一轮。

### 本轮开工前无待决事项

Root Orchestrator 冻结 base 后即可开工。

---

## 6. 最终真实测试证据

以下均在本候选上实际执行；所列 exit code 均为 `0`。

- `.\scripts\check-fast.ps1`：快速门禁通过。分层门禁报告 `AC03=0`、`AC01=12`、allowlist `=1`；门禁 fixture 为按实际调用计数的 18 个断言，覆盖 Rust、C++、tests、migrations 与字符串拼接绕过。可得的新增针对性测试包括 Registry API 1、主体领域 2、主体血缘映射 1、server composition 3。
- `.\scripts\check.ps1`：完整本地检查通过。包含严格 Clippy、Rust/C++/Python/Web 构建与测试；生成契约 14 项、C++ CTest 8 项、Python 生成契约 1 通过/1 按设计跳过及 live parity 1 通过、Web 35 项通过。验收矩阵报告 `Q-001..Q-036` 36 项映射完整且冻结资产未变。
- `cargo test --offline --locked -p ficant-server`：server 生产组合的 3 项、health probe 的 5 项、integrity sink 的 3 项测试通过。
- `.\scripts\dev-up.ps1`：本地 PostgreSQL、Ceph RGW、migration、Server、Worker、Web、UI 拓扑就绪；除一次性 migration 正常退出外，其余预期服务均健康。
- 实际执行的一次性 Python gRPC-Web 垂直切片（经 UI `/ficant-api`）：`RegisterSubject`、`GetSubject`、`RegisterSubjectState`、`GetSubjectState` 逐字段往返相同；带 `subject_ref` 的 `AnalyzeBond` 与不带该字段的调用数值逐字段相同，且结果元数据带回相同主体版本引用。另以早于 `visible_at` 一秒的 `knowledge_at` 查询，得到结构化 `NotFound`。两项切片检查均通过。
- `.\scripts\dev-down.ps1`：开发容器停止成功，脚本默认保留 PostgreSQL 与 Ceph 命名卷。
- `.\scripts\check.ps1 -IncludeIntegration`：风险回归通过。可得环境测试共 31 项：migration 4、lease queue 1、执行闭包 3、生产 worker 1、Phase 1 1、负向不变量 13、Phase 2B/2C/2D 各 1、Phase 3A 2、Phase 3B 3。

规定的本轮自测命令：

```powershell
.\scripts\check-fast.ps1
.\scripts\check.ps1
.\scripts\dev-up.ps1
# 垂直切片：经 UI /ficant-api 调用 RegisterSubject / GetSubject /
#           RegisterSubjectState / GetSubjectState，再调一次带 subject_ref 的 AnalyzeBond 并展开血缘
.\scripts\dev-down.ps1
```

风险相关回归命令：

```powershell
.\scripts\check.ps1 -IncludeIntegration
```

**允许写路径：** `interface/proto/ficant/core/v1/subject.proto`、`interface/proto/ficant/core/v1/subject_state.proto`、`interface/proto/ficant/rates/v1/analytics.proto`（仅新增 `subject_ref`）、`crates/ficant-domain/src/**`、`crates/ficant-api/src/**`、`crates/ficant-application/src/**`、`crates/ficant-storage/src/postgres/**`、`migrations/postgresql/**`、`scripts/**`、`docs/architecture/adr/**`、`README.md`、`MANUAL.md`、`binaries/ficant-server/src/lib.rs`（仅 RegistryService 的生产组合与路由；Human 于 2026-07-27 授权）

**禁止写路径：** `cpp/**`、`crates/ficant-domain/src/futures_delivery.rs`、`crates/ficant-data/src/canonical.rs`、`crates/ficant-domain/src/market/bond.rs`、`tests/golden-cases/**`、`.github/**`、`deploy/**`、`cicd.yml`、`SPEC.md`、`ACCEPTANCE.md`

> SPEC 与 ACCEPTANCE 的写入权在 Human。Agent 发现需要修改时提 diff，不直接改。

---

## 7. 残余风险

- **主体仍只有形状与血缘。** `FundingTier`、税收待遇、净资本和额度上限尚不影响融资成本、税收或约束判断；调用方继续提供融资利率。这是 R1 的明确非目标，后续语义轮必须保持数值回归证据。
- **主体版本引用具有不可逆性。** 它已可进入历史 `AnalyzeBond` 结果元数据；版本字段或版本演进策略的后续变更必须保持既有血缘可读。
- **唯一遗留 allowlist 是受控债券期货交割规则。** 分层门禁实测为恰好一条 `futures_delivery`（移除轮次 R2）；后续轮只能移除，不能增加。
- **RegistryService 当前是服务契约，不是业务界面。** 已验证 gRPC-Web 路由和认证 scope；Platform Shell 仍未提供主体搜索、编辑、资金/税收或额度业务工作流。
- **AC02 与 AC04 尚无可执行判据。** R1 不再把它们表述为已建立；R2 必须以 RulePack 语义对照用例建立 AC02，R7 必须以虚构市场端到端场景和行数统计建立 AC04。
