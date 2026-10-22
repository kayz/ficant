# ADR-0022：WebApp 是平台的客户端；portfolio 需求边界与 Workbench 冻结

- 状态：Accepted
- 日期：2026-10-23
- 决策者：Human
- 范围：ficant 与 WebApp 的关系；相邻 WebApp `kayz/ficant-portfolio` 提出的后台需求；`ficant.portfolio.v1.PortfolioWorkbenchService`

## 背景

ficant-portfolio 是"ficant 后台 + WebApp"场景的验证项目。它本身是衍生品，不影响 ficant 的方向，但会提出能力需求。

它列出 36 条后台需求，其中 25 条需要新建。有几组接近或越过 SPEC §2 明确不做的范围（订单生成与生命周期、清算结算、总账与投资组合核算）：

- `transaction-ledger`：交易台账与成本批次；
- `valuation-results`、`valuation-rule-registry`：日终估值批次与估值规则发布；
- `rebalance-plans`：生成具体交易列表（SPEC 允许的输出是 TargetExposure）；
- `workflow-command`、`alert-*`、`portfolio-restructure`：审批流、告警与带副作用的命令。

R8A 还为它加入了 `PortfolioWorkbenchService.GetPage`。它按 `PortfolioWorkbenchPageId`（D01、P01–P04）返回各页面的投影，把一个 WebApp 的页面划分写进了平台公共契约。

## 决策

**总原则：ficant 是平台，WebApp 是平台上的客户端。**

- 可以同时存在多个 WebApp，彼此独立；任何一个都可以公开或私有开发，放在各自的仓库里。
- ficant 仓库只放平台本身：后台服务、公共契约、契约包，以及作为宿主的 Platform Shell。WebApp 的页面设计、源码与测试不进 ficant 仓库。
- 公共契约与具体 WebApp 无关：不按某个应用的页面组织接口，不在契约里出现应用名或页面编号。
- 新增或下线一个 WebApp 不应要求修改 ficant 源码：注册、授权范围与入口地址属于部署配置。
- WebApp 只通过已发布、带版本的契约包与平台交互，不引用 ficant 的源码目录。

据此，下面两条是对 portfolio 的具体适用。

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

## 现状差距与后续

对照总原则，现在有三处不满足，按顺序处理：

1. **应用注册写死在代码里。** 生产 `ficant-server` 构造平台时传入空的应用列表（`build_platform_application` 中的 `Vec::new()`），新增 WebApp 只能改源码。改为从部署配置读取应用注册（`app_id`、显示名、入口、允许的 origin、capabilities），沿用 `AppRegistration::try_new` 的校验。
2. **契约包不能独立消费。** `@ficant/contracts-generated` 是 `private: true`、版本 `0.0.0`，portfolio 通过指向 `../ficant` 源码的路径别名使用。改为随 ficant 版本 tag 发布带版本的包（私有 WebApp 可用 GitHub Packages）；Python SDK 同理。契约兼容性已有 `buf breaking`（FILE 级）把关。
3. **WebApp 设计稿放在 ficant 仓库里。** `web-dm/webapps/dmquant/design.md` 是 ficant 研究 WebApp 的目标设计，应迁到该 WebApp 自己的仓库；仓库建立前先存放在私有研究资料库。迁出需要同时修改仓库策略脚本里的必需路径（受保护的 `.github/**`）。

## 代价

- portfolio 的部分页面要自己拼装多个通用接口的结果，前端多做一些工作。
- 平台契约里暂时保留一个应用专用服务，需要在评审时记住它是例外。

## 替代方案

- 扩大 SPEC，让 ficant 往组合管理系统发展：拒绝。ficant 是研究平台，portfolio 只是验证场景。
- 立即把 Workbench 移出：暂不。portfolio 目前只有前端，移出意味着先为它建后台，收益不抵成本。
