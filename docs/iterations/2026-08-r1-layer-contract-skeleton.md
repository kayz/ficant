# R1 迭代 brief — 分层门禁与主体契约

> 本文是本迭代面向 Human 的唯一文档。Agent 交流、Worker 证据、失败诊断与中间候选保留在编排工具与实际命令输出中，不进入本文。

**迭代** R1 · **状态** 待 Human 冻结 · **依据** SPEC v1.1、ACCEPTANCE v0.1（30 条）、[`../architecture/layering-refactor.md`](../architecture/layering-refactor.md)

**Base commit SHA：** `<Root Orchestrator 开工第一步自行冻结，见 §3>`

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

**本轮建立判据但不点亮**（红灯是预期状态，由后续轮次转绿）：

| 条目 | 判据形态 | 转绿轮次 |
|---|---|---|
| AC01 | `ficant-domain` 内规则数值检索，`futures_delivery` 暂列 allowlist | R2 |
| AC02 | 换 RulePack 换结果的对照用例（当前必然失败） | R2 |
| AC04 | 虚构市场的 L0/L1/L2 改动行数统计脚本 | R7 |

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
| `subject.proto` | `Subject`、`SubjectVersion`、`AccessSet`、`FundingTier`、`TaxTreatment`、`ConstraintSetRef` | **只装身份**：准入集、资金档**可得性**、税收待遇、考核机制、约束集引用。可版本化。**引用而非内嵌任何数值** |
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

`<本节由执行本轮的 Root 在完成后填写，须记录实际命令、exit code 与可得的 test count。计划命令、-ListOnly 输出与 Worker 文字声明都不能冒充测试通过。>`

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

**允许写路径：** `interface/proto/ficant/core/v1/subject.proto`、`interface/proto/ficant/core/v1/subject_state.proto`、`interface/proto/ficant/rates/v1/analytics.proto`（仅新增 `subject_ref`）、`crates/ficant-domain/src/**`、`crates/ficant-api/src/**`、`crates/ficant-application/src/**`、`crates/ficant-storage/src/postgres/**`、`migrations/postgresql/**`、`scripts/**`、`docs/architecture/adr/**`、`README.md`、`MANUAL.md`

**禁止写路径：** `cpp/**`、`crates/ficant-domain/src/futures_delivery.rs`、`crates/ficant-data/src/canonical.rs`、`crates/ficant-domain/src/market/bond.rs`、`tests/golden-cases/**`、`.github/**`、`deploy/**`、`cicd.yml`、`SPEC.md`、`ACCEPTANCE.md`

> SPEC 与 ACCEPTANCE 的写入权在 Human。Agent 发现需要修改时提 diff，不直接改。

---

## 7. 残余风险

`<完成后补充实测残余风险。以下为开工前的预判。>`

- **本轮最可能的越界是"顺手把其他契约也定义了"。** 非目标里已显式列出五个 proto 文件，且允许写路径按文件名精确限定，不给目录通配。Root 检查真实 diff 时应首先核对是否出现未授权的 `.proto`。
- **Constraint 的"只有形状"边界容易被侵蚀。** agent 会不知疲倦地优化它摸得到的一切，很可能把 `500%` 写进 domain 当默认值。分层检查必须在本轮就位——这正是它排在最前的理由。
- **allowlist 会被当成逃生舱。** 规格上它只能移除不能新增；若 Root 在 diff 中发现 allowlist 增加了条目，即为失败候选，不论测试是否通过。
- **主体版本标识进入历史血缘后不可更改**，与 FactorId 同属高不可逆项。建议 Human 直接审阅 `subject.proto` 的版本字段设计，而非依赖验收清单。
- 新增 2 个 proto 文件与 1 个 service 会增加契约测试与三侧类型生成的构建时长，需观察是否触及熵预算。
