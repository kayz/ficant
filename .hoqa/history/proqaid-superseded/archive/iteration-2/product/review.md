# Product Review：iteration-2 round-1

## Review Outcome

**结论：Product 当前记忆已与共享快照一致，可进入 worker 计划/提示词合并；没有未解决的 Product pre-dispatch defect。** iteration-2 的用户价值继续由契约/API 驱动的真实纵向业务切片证明，而不是把 Phase 0/1 清单拆成互不相连的目录、类型和 CRUD。`Valuation`/`CurveSnapshot` 的事实存储边界及 Phase 1 最小 `SignalSet` 与 Phase 9 发布能力的区别，已经进入当前 `docs/product/scope.md`，现作为冻结 worker 约束执行，不再作为待解决 blocking finding。

## Evidence Reviewed

- `README.md`
- `iteration-2-checklist.md`
- `docs/product/scope.md`
- `.codex/AGENTS.md`
- `.proqaid/orchestrator/current-iteration.md`
- `.proqaid/product/charter.md`
- `.proqaid/product/inbox.md`
- `.proqaid/product/inbox.iteration-2.round-1.md`
- iteration-1 Product 的已归档 `context.md`、`review.md` 与 round-1 outbox

## 方案取舍

1. **推荐：契约/API 驱动的最小纵向闭环。** 用一组确定性的中国国债/国债期货 fixture 串起市场事实、快照、运行、日志、Artifact、最小 SignalSet、查询与重放。优点是直接证明用户价值和业务不变量；代价是 worker 与 Quality 必须围绕一条共享验收链路协调。
2. **不推荐：按对象逐一完成 CRUD。** 易于切分 worker，但只能证明孤立对象存在，不能证明快照冻结、跨对象血缘、不可变性和重放共同工作。
3. **本轮排除：以 DMQuant/Platform Shell 为主线。** 可形成可见界面，但会提前引入完整策略、回测或 AI 体验，并可能用静态交互掩盖后端闭环缺失。

人类已确认的 checklist 与 inbox 明确要求真实 Phase 0+1 业务闭环，因此 round-1 采用方案 1，不新增产品范围。

## Confirmed Product Findings

- 主要用户是固定收益量化研究员/研究工程师；本轮核心工作不是生成策略，而是把市场事实冻结为可审计研究输入并证明结果血缘。
- 一组业务形态真实、来源明确的 golden fixture 足以验证本轮领域闭环；它可以是确定性测试数据，但不能被描述为实时行情或供应商数据。
- `Valuation` 与 `CurveSnapshot` 在 Phase 1 可以作为外部/fixture 市场事实被验证、持久化和冻结；生成这些值的定价、收益率、风险和曲线算法属于 Phase 2。
- Phase 1 已在 README 的优先对象中包含 `SignalSet`，但 README Phase 9 才交付 Registry、正式发布治理与 `TargetExposure`。iteration-2 只能验证最小 SignalSet 领域记录、不可变持久化和完整血缘约束。
- 本轮业务关闭必须证明真实 PostgreSQL/MinIO、点时快照、追加日志、内容寻址、按 ID/版本/时间/血缘查询、幂等性、重放和负向不变量共同成立。
- Platform Shell 启动和共享契约调用属于 Phase 0 集成证据，但不能单独代表 Product 业务闭环。

## Frozen Worker Constraints

1. `Valuation`/`CurveSnapshot` 是“带来源的输入事实，仅验证、存储和冻结”；任何 Phase 2 定价、收益率、风险或曲线算法及其伪成功实现均不在授权范围。
2. Phase 1 最小 `SignalSet` 只验证领域记录、不可变持久化和完整血缘；Phase 9 Registry、审批、下游发布及 `TargetExposure` 不在授权范围。任何扩大必须先由 Orchestrator 按 checklist 变更流程请求人类决策。

这两条是已批准产品范围的执行约束，而非尚待修正文档或待决策缺陷。Orchestrator 应把它们逐字或等义写入相关 worker prompt、验收矩阵和集成检查。

## Resolved Current-Memory Findings

1. **产品范围已合并：** `docs/product/scope.md` 已包含 iteration-2 用户价值、最小纵向闭环、P-01 至 P-07、真实 PostgreSQL/MinIO 要求和 Phase 2/9 边界。
2. **仓库结构已合并：** README 已以根 `interface/` 替代平行 `proto/` 入口，并明确 `web-dm/` 的 WebApp 共置结构与生成契约来源。
3. **角色 charter 已清理：** `.proqaid/product/charter.md` 已移除 iteration-1 专属 Write Boundary，改为长期迭代中立表述。

## Remaining Evidence Gate

- README 当前“尚无生产源码、可运行系统或已验证产品行为”的声明仍与共享快照一致，不是陈旧 finding。
- worker 完成前不得改写该状态为已实现。iteration-2 最终 Product 复核必须根据 P-01 至 P-07 和 Quality/Delivery 证据，逐项列出真实通过的 Phase 0/1 能力，并继续把 Phase 2–9 与完整 DMQuant 标为后续范围。

## Product Acceptance Position

- Product 接受当前 `docs/product/scope.md` 与 `.proqaid/product/context.md` 中的 P-01 至 P-07 作为 iteration-2 产品验收基线；Quality 证据矩阵和 worker 验收必须保持同一语义。
- Product 不接受 mock repository、内存数据库、硬编码成功结果、静态页面、单纯连通性或覆盖率数字作为闭环证据。
- Product 不接受把存储 fixture `Valuation`/`CurveSnapshot` 描述为已具备定价/曲线能力，也不接受把最小 SignalSet 记录描述为 Phase 9 发布已完成。
- Product 最终确认必须在 worker 完成后重新检查 README、`docs/product/scope.md` 与真实运行证据；round-1 不认证任何已实现行为。

## Runtime Policy

- 目标运行时：GPT-5.6 Terra，reasoning high。
- 实际模型应用状态：**unverified/fallback status unverified**。当前运行时未提供可核验的模型或 reasoning 证明，也无法证明 fallback 是否发生。

## Validity

Valid: iteration-2 only
