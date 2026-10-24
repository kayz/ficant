# ADR-0029：回放输出与血缘

- 状态：Accepted
- 日期：2026-10-24
- 决策者：Human（2026-10-24 要求完成二期收尾）；本 ADR 的具体做法由 agent 提出，随 PR 交 Human 审阅
- 范围：AC44
- 关联：ADR-0014、ADR-0024 至 ADR-0028；SPEC §2「ficant 不做订单」

## 背景

AC44：回放结果以 SimulationResult、SignalSet 或 TargetExposure 输出并可展开完整血缘；输出中不存在任何订单对象。

回放结果已是 `SimulationResult`，每个决策的输出是 `TargetExposure`，仿真成交是 `SimulatedFill`（建模记录，不是订单）。结果标识已绑定全部输入，但部分输入只以哈希出现，没有对象 ID，无法逐项展开。

## 决策

### 1. 血缘

`SimulationResult::lineage()` 按固定顺序列出全部输入对象，每项是「角色 + 精确引用（`LineageRef`）」：

| 角色 | 引用 |
|---|---|
| `Policy` | PolicyArtifact ID + 内容哈希 |
| `DataSnapshot` | Bar 快照 ID + 内容哈希 |
| `Subject` | Subject 版本 |
| `SubjectState` | 状态快照 ID + 内容哈希（`subject_state_hash`：ID、Subject、净资本、按代码排序的上限、时间） |
| `ConstraintSet` | 约束集规则包版本（仅当 Subject 绑定了约束集） |
| `FundingRulePack` | 资金规则包版本 |
| `CouponData` | 票息参考数据版本 + 数据内容哈希 |

- 为此结果标识新增 PolicyArtifact ID、状态快照内容哈希、票息参考数据版本；`BondCarry` 端口新增 `source()`。
- 代码与运行环境摘要、种子、成本模型参数不是对象，留在结果标识里，不进入血缘列表。
- 已标定的 PolicyArtifact 通过标定记录引用每个候选的回放结果 ID（ADR-0028），可继续展开到上表。

### 2. 不产生订单的门禁

测试扫描回放与标定的全部源码（`ficant_domain::replay` 模块、`bar_replay` 与 `policy_calibration` 用例，去掉注释）：任何以 `order` / `orders` 为一个词的标识符（类型、字段、变体、函数）都使门禁失败。`ordered` 这类词不算。门禁本身有正反样例测试。

## 代价

- 门禁按标识符拼写判断，换个名字（如 `Instruction`）就能绕过；它防的是无意中引入订单概念，不是恶意规避。评审仍需把关。
- 门禁范围限于回放与标定，不扫描整个仓库：因子模型里的 `SecondOrderPolicy`（二阶项）不是交易订单。
