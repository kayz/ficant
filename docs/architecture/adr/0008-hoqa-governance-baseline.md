# ADR-0008：采用 HOQA 治理基线（历史）

- 状态：Superseded by ADR-0009
- 日期：2026-07-15
- 范围：项目治理、Human/Model 分工、Worker 编排、测试、环境、交付与关闭

> 本文只保留 HOQA 时期的决策与审计事实。自 ADR-0009 生效后，OPAID 管理本地开发与确定性自测，中央 CI/CD 管理发布；本文以及 `docs/history/hoqa/**` 均不再是活动执行入口。

## 背景

项目已形成可复用的产品目标、架构边界、业务实现、测试资产、环境能力、runner 和历史证据，但原 PROQAID 基线把七个责任视角、阶段门、频繁 Review 与角色交接同时暴露给执行过程。这增加了切换和恢复成本，也使业务判断、自动化测试、执行器权限与交付操作容易相互混淆。

本决策当时只迁移治理机制，不否定已接受的产品事实、技术 ADR、实现、测试数据、缺陷、风险或证据。原完整承接关系和可恢复状态现分别归档于 `docs/history/hoqa/governance/migration-map.md` 与 `docs/history/hoqa/governance/state.toml`。

## 决策

在本 ADR 有效期间，HOQA 是项目唯一活动软件工程治理方法，治理包位于 `.hoqa/`；原 `.proqaid` 和项目本地 PROQAID skill 当时移入 `.hoqa/history/proqaid-superseded/`。上述完整治理包现已整体归档到 `docs/history/hoqa/governance/`，只能用于历史审计，不得驱动当前工作。

### 参与者与责任

- Human 决定意图、业务含义、业务接受、风险接受，以及凭证、GUI、管理员权限和其他低可观测操作。
- Orchestrator 负责计划、Product/Architecture/Interface 三个决策 lens、Development Worker、集成、确定性验证、交付工作和关闭准备。
- Quality 在存在非平凡自动化测试策略或需要协调 Test Worker 时独立工作，负责测试计划、自动化清单、Test Worker、批次证据、测试报告和缺陷列表。Quality 不批准业务含义，不监督 Development Worker，不承担最终迭代 verdict。
- Audit 仅在最终文档和候选证据准备完毕后，以只读方式检查 Human 意图、文档、代码/配置和证据的一致性。Audit 不重设计、不修改、不路由修复。

Product、Architecture、Interface 和 Delivery 仍是必须履行的项目责任，但不再作为独立 HOQA 参与者或过程门。前三者由 Orchestrator 使用对应 lens 完成；Delivery 是 Orchestrator 的交付工作，特权和低可观测操作归 Human。

### Worker 与 runner

Worker 是临时执行资源，不是角色。只有两个以上相互独立、有界的任务能够并行且收益高于协调成本时才使用 Worker。Development Worker 由 Orchestrator 管理，Test Worker 由 Quality 管理；单一顺序任务默认由对应参与者直接完成。

模型、权限和执行环境分别路由。Spark 可在有界机械任务中使用 read-only 或 isolated workspace-write；高风险、未知根因、跨模块、数值、FFI/内存、安全、事务/恢复或业务语义问题直接使用强模型。任何 Worker 请求改变冻结边界、expected、Oracle 或容差时必须停止并返回 Orchestrator/Human 决策面。

runner 只封装执行复杂性：精确 base、隔离 worktree、路径边界、冻结合同、命令、退出码、测试计数、实际模型、权限、环境、恢复预算、证据和清理。角色、业务模块和文档不得复制这些平台细节。Worker 完成声明只是候选结果，参与者判断必须绑定确定性证据。

### 测试、缺陷和接受

Human 冻结业务含义与接受边界；Quality 设计或确认测试策略、fixture/Oracle 自动化和容差表达，组织 Test Worker 执行，并提交测试报告和结构化 bug list。Orchestrator 根据 bug list 路由 Development 修复；Quality 只重测受影响清单。Workers 不得通过弱化断言或修改 expected/Oracle/容差取得绿灯。

测试工具的退出码、测试清单、数量和证据决定自动化事实；Quality 判断这些事实是否覆盖既定测试合同；Human 决定业务接受和剩余风险。最终 Audit 只判断文档到意图、代码和证据是否一致。

### 环境、SIT、Delivery 与 UAT

普通开发和测试使用 Windows 11、PowerShell 7、Windows Git/worktree 和 task-local capability preflight。旧 WSL runner 只作兼容历史，不再是普通入口。仅当验收需要集成持久化、服务拓扑或目标式行为时进入 SIT。

SIT 中，Orchestrator 的 delivery work 通过 Docker Desktop 管理隔离 PostgreSQL/MinIO、测试数据、健康与清理；Human 负责保持 Docker Desktop 可用，以及 GUI、管理员或 WSL integration 操作。Orchestrator 准备打包、迁移/回滚和发布合同；Human 授权并执行凭证相关或低可观测操作。

VPS `47.100.66.40`、`greatquant.com`、应用名 `dm` 仍是 UAT/发布目标。UAT 位于迭代关闭之后，结果不影响迭代关闭。`C:\git\key` 不得被普通 Worker 或治理迁移读取、枚举、复制、输出或写入项目。

### 阶段与关闭

本 ADR 有效期间的迭代按 HOQA 的 Align/Decide、Execute、Test、Operate、Close 组织。进入实现前必须具有清晰目标、范围、非目标、Human/Model 分工、技术边界和可验证接受；进入 Test 前必须有集成候选；进入 Operate 前必须确认该迭代确需 SIT 或发布准备。

关闭要求：冻结接受满足；集成确定性验证通过；Quality 报告无阻断缺陷；适用的 SIT/发布准备证据与候选绑定；最终文档与 Human 意图、代码和证据一致；Audit 通过；Human 接受业务结果和剩余风险。UAT 不属于关闭条件。

## 选择依据

HOQA 将复杂性封装在参与者职责、Worker 合同和 runner 内部，而不让治理状态机、权限细节和执行平台弥漫到所有业务模块和文档。它保留独立 Quality、最终一致性 Audit、可审计证据和 Human Operator 边界，同时减少无收益的角色切换、串行交接和重复恢复。

## 隔离边界与风险

- 治理事实与历史证据隔离：在本 ADR 有效期间，`.hoqa/` 是活动基线、`.hoqa/history/**` 只读且无权威性；现在二者均位于 `docs/history/hoqa/governance/`，只作历史证据。
- 判断与执行隔离：参与者制定和接受合同，Worker 只执行有界任务。
- 测试与开发隔离：Quality/Test Worker 不监督或修改开发；Orchestrator/Development Worker 不修改冻结测试语义。
- 环境与凭证隔离：普通 Worker 无 UAT/root key 权限；Human 承担低可观测操作。
- 风险：角色名减少可能让责任被误认为消失。当时的缓解方式是 `.hoqa/state.toml`、checklist 和 runner profile 明确映射；这些材料现已归档且无活动权威性。
- 风险：历史 PROQAID 文本可能被误读为活动规则。缓解方式是移动到 superseded 目录、历史文档加标记并由仓库策略禁止重新创建 `.proqaid`。

## 替代方案

- 保留 PROQAID 并只改名：拒绝，因为无法消除竞争权威和旧角色门。
- 同时保留 `.proqaid` 与 `.hoqa`：拒绝，因为恢复时无法确定活动基线。
- 删除全部历史治理材料：拒绝，因为会丢失关闭事实、SHA、风险和证据来源。
- 所有工作都交给 Worker：拒绝，因为不确定性和协调成本会扩散，且弱化 Human/Model 责任边界。

## 后续迁移条件

本条件已由 Human 采用 OPAID 并以中央 CI/CD 管理发布的决定触发。后续活动边界以 ADR-0009 为准，不再通过修改本历史 ADR 恢复 HOQA 或旧 runner。

## 影响

ADR-0008 当时取代所有活动 PROQAID 治理机制和旧七角色阶段门，但不取代 ADR-0001 至 ADR-0007 的有效技术决策。ADR-0009 现已取代 ADR-0008 的活动治理地位；iteration-1/2 的 PROQAID verdict 与 HOQA 时期记录仍是当时的历史事实，不转换为当前 OPAID 状态。
