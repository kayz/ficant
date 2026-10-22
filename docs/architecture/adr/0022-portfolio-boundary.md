# ADR-0022：portfolio 需求边界与 Workbench 接口冻结

- 状态：Accepted
- 日期：2026-10-23
- 决策者：Human
- 范围：相邻 WebApp `kayz/ficant-portfolio` 提出的后台需求；`ficant.portfolio.v1.PortfolioWorkbenchService`

## 背景

ficant-portfolio 是"ficant 后台 + WebApp"场景的验证项目。它本身是衍生品，不影响 ficant 的方向，但会提出能力需求。

它列出 36 条后台需求，其中 25 条需要新建。有几组接近或越过 SPEC §2 明确不做的范围（订单生成与生命周期、清算结算、总账与投资组合核算）：

- `transaction-ledger`：交易台账与成本批次；
- `valuation-results`、`valuation-rule-registry`：日终估值批次与估值规则发布；
- `rebalance-plans`：生成具体交易列表（SPEC 允许的输出是 TargetExposure）；
- `workflow-command`、`alert-*`、`portfolio-restructure`：审批流、告警与带副作用的命令。

R8A 还为它加入了 `PortfolioWorkbenchService.GetPage`。它按 `PortfolioWorkbenchPageId`（D01、P01–P04）返回各页面的投影，把一个 WebApp 的页面划分写进了平台公共契约。

## 决策

**需求准入规则。** portfolio 的需求只有在满足以下之一时才进入 ficant：

1. SPEC 已覆盖；
2. 能表述成与具体页面无关的通用研究能力，例如只读的组合目录与聚合、敞口、估值与风险、收益序列、TargetExposure。

运营类功能留在 portfolio 侧（mock 或独立服务），不进入 ficant：交易台账与成本批次、日终估值批次、把调仓落成交易列表、审批流、告警，以及其他带副作用的命令。若认为某项运营能力应该进入 ficant，先修订 SPEC，再实现。

**Workbench 接口冻结。** `PortfolioWorkbenchService` 保留，但：

- 标为应用专用接口，不承诺稳定性，不作为其他 WebApp 的依赖；
- 不再新增 `PortfolioWorkbenchPageId`、页面投影或其他按页面组织的 RPC；
- portfolio 的新需求以通用服务（如 `PortfolioCatalogService`、`PortfolioAggregationService`、`PortfolioPerformanceService` 的扩展）提出；
- 等 portfolio 有自己的后台时，把页面组装迁过去，再从 ficant 删除该服务。

已有实现、契约与测试不变。冻结只约束以后的新增，不修改 `interface/proto/**`。

## 代价

- portfolio 的部分页面要自己拼装多个通用接口的结果，前端多做一些工作。
- 平台契约里暂时保留一个应用专用服务，需要在评审时记住它是例外。

## 替代方案

- 扩大 SPEC，让 ficant 往组合管理系统发展：拒绝。ficant 是研究平台，portfolio 只是验证场景。
- 立即把 Workbench 移出：暂不。portfolio 目前只有前端，移出意味着先为它建后台，收益不抵成本。
