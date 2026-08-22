# R8A 迭代 brief — 只读组合纵向切片

**面向 Human 的产品名：** 金证FICC合同管理系统 · **平台名：** FICANT · **内部迭代：** R8A · **execution base：** `11015f41b3f58e82017e85a834f2ba227b702ca2` · **base tree：** `b5afe57f443cae36b47216ffd9e4ba518650aa12` · **状态：** 本地自测候选已形成，待 Human 验收

本 brief 是 R8A 面向 Human 的唯一设计、权限边界与最终证据载体。它只在 `C:\git\ficant` 建设只读组合后台纵向切片；`C:\git\ficant-portfolio`、COGA、Web UI、发布、部署和远端治理均不在本轮写入范围。Human 已于 2026-08-21 批准本 brief。§6 以下证据来自同一最终代码候选，不把计划命令写成通过。

## 1. 目标

R8A 为金证FICC合同管理系统的 D01、P01、P02、P03、P04 五页提供真实、只读、可审计的中国国债组合后端。FICANT 新增最小不可变 Portfolio / Book / PortfolioGroup 目录、exact PositionSnapshot 绑定、最小 BenchmarkRef 与版本化 PortfolioMetricConvention；Book / Portfolio 只定义目录和聚合范围，不复制 Position、交易、成本、现金、NAV 或会计账簿。

新增 `PortfolioCatalogService.ListBooksAndPortfolios`、`PortfolioAggregationService.GetPortfolioOverview` 和最小 `PortfolioWorkbenchService.GetDefaultContext/GetPage`。Catalog 是非正式 CRUD 读取；Aggregation 是使用现有 PositionViews、Portfolio KRD、Rates AnalyzeBond、MarketFact 与 MarketDefinition 的正式分析组合结果；Workbench 只做六维 context 解析、授权裁剪、编排和领域投影，不承载页面布局或金融不变量。

WebApp 以 Hybrid 方式消费：后台可达时这五页只能使用真实 DTO；其他十九页由 WebApp 自己明确标记 demo / partial。FICANT BFF 永不返回 `demo`，真实请求失败必须返回 typed error，浏览器不解析底层 exact refs、不做金融聚合，也不能用 mock 回退冒充 backend success。

**Acceptance sentence：**

> 给定有权 Researcher、六维 Portfolio context 和 D01/P01/P02/P03/P04 任一页，FICANT 必须先把 scope、估值/知识时点、币种、层级模式、Benchmark 与 period 解析为 owner 一致且可 required-read 的 exact Portfolio/Book/Group、PositionSnapshot、Definition、Data/Curve snapshot、Calendar、RulePack、Benchmark 和 PortfolioMetricConvention，再复用既有 PositionViews、CalculateKeyRateDv01、AnalyzeBond 与 MarketFact/Definition 路径产生 `real`、`partial`、`stale` 或 typed `error` 的只读领域投影；P02/D01 的新增聚合只用 Decimal 与批准的版本化 convention，能被独立 Oracle 逐项证明并携完整 formal evidence，任何 owner/version/hash/knowledge/as-of/benchmark/convention 漂移均在底层金融调用前失败关闭。最终 descriptor 的 17 个公共 service 与唯一生产 route set 完全相等，固定 Rust/Python/TypeScript consumers、确定性本地 TypeScript 包、真实 server gRPC/gRPC-Web SIT 和三个统一本地检查入口在同一候选上通过；既有十四个服务、R5D AnalyzeBond exact-input 合同、R7B identity、Oracle、expected 和容差均不回退。

## 2. 验收

| 条目 | R8A 可执行判据 |
|---|---|
| 最小领域与目录 | `Book`、`PortfolioGroup`、`Portfolio`、`PortfolioSnapshotBinding`、`BenchmarkRef`、`PortfolioMetricConvention` 全部不可变、owner/Subject scoped、双时间可判、content-addressed。Portfolio 精确绑定已有 PositionSnapshot；不存在第二套 position、交易、NAV 或账簿余额表。 |
| Catalog | `ListBooksAndPortfolios` 按可信 principal 的 tenant/allowed owner 裁剪，支持 exact owner/Subject、as-of、knowledge-at、状态、规范化搜索和 scope-bound AEAD cursor。排序固定为 `(book_code, group_path, portfolio_code, version)`；翻页不混入较晚可见版本。响应返回完整树、Portfolio 列表和 exact PositionSnapshot 绑定，并明确标记为非正式读取证据。 |
| Aggregation | `GetPortfolioOverview` 对一个 normalized book/group/portfolio scope 逐个 required-read 其 exact PositionSnapshot，调用既有 PositionViews、PortfolioRisk 与逐券 AnalyzeBond，查询既有 Definition/Fact/Data/Curve authority；只聚合既有结果，不复制债券定价、KRD、曲线或事实选择算法。结果包含规模、外部 economic P&L、批准的点时债券组合指标、DV01/KRD、Benchmark 对照、成员明细、coverage、request fingerprint 与 `FormalOutputEvidence`。 |
| Workbench | `GetDefaultContext` 在 caller 指定 owner/Subject/knowledge-at 内稳定选择首个可见 active Portfolio，并返回完整 normalized context；`GetPage` 只接受 D01/P01/P02/P03/P04，返回 `portfolio-workbench.v1` PageEnvelope。P01 投影目录；P03 投影既有 PositionViews/KRD；P04 投影 Definition、MarketFacts 与 AnalyzeBond formal result；P02/D01 投影 PortfolioOverview。没有 ListWorkspaces、ListPages 或 Execute。 |
| 六维 context | `portfolioScope` 改变 exact member set；`asOf`/`knowledgeAt` 改变双时间选择；`currency` 改变 Unit/折算判据或返回 unsupported；`lookThrough` 改变目录成员展开；`benchmark` 改变 exact benchmark snapshot 与对照；`period` 改变 MarketFact 查询窗和 resolved period。每一维都有单变量正负测试；不得只改 provenance 文本。 |
| Fail-closed | principal owner、Portfolio/Book/Group version/hash、PositionSnapshot hash/time、Definition/Data/Curve/Calendar/RulePack、Benchmark、MetricConvention、Unit 或 context 任一漂移，AtomicUsize/等价 spy 证明 PositionViews/KRD/AnalyzeBond 数值 handoff 均为 `0`。缺失会使声明范围结果错误时返回 ERROR；只影响可明确分母和 participating set 的缺失可返回 PARTIAL，但必须列 coverage，不得注入零值。 |
| 数据模式 | `REAL` 仅用于声明范围完整且 freshness 合格；`PARTIAL` 必须有非空缺失原因且数值只陈述 participating set；`STALE` 必须仍通过所有 hash/双时间校验但超过 convention freshness；`ERROR` 不得携 success projection，并带七类 closed typed error。BFF 没有 `DEMO` 枚举。 |
| 授权 | 三个新 service 统一要求 Researcher 与 `portfolio:read`，并继续按 tenant/allowed owner 裁剪；BFF 不能借内部组合扩大 caller 对 Position、Fact、Definition、Artifact 或 Subject 的可见范围。错误、aggregate 和分页均不得泄露无权对象。 |
| 正式与非正式证据 | PortfolioOverview 使用既有 R7B canonical v1 identity 与 FormalOutputPublisher；新增 typed Portfolio/Book/Group/Benchmark/Convention input kinds，但不改变 identity 算法。Catalog、Definition/Fact 读取使用 `NonFormalReadEvidence`，不能伪装为正式分析；Page provenance 稳定排序承载两类证据。 |
| Decimal Oracle | 独立 Python Decimal Oracle 固定至少两个 Portfolio、三只中国国债和三个 KRD factor，独立计算 market value/economic P&L sum、加权 YTM、修正久期、凸性、票息、剩余期限、DV01/KRD 与 Benchmark 差；Rust 测试只解析 fixture/expected、调用生产聚合并逐字段比对，不从生产实现导入公式。不得调整既有 Oracle、expected 或容差。 |
| 契约与拓扑 | 新增一个 proto package、三个 service、四个 RPC；14 个既有 service/RPC/tag 不变。descriptor service inventory 精确从 `14` 扩为 `17`。coverage inventory 精确变为 `68` 个 reachable success arms：`6` 个 composition carrier、`62` 个具名非组合 arm；descriptor-extra 与 route-missing 两个真实反例继续失败。 |
| TypeScript 包 | `interface/` 是唯一 proto 源；tracked TS generated tree 形成 `@ficant/contracts-generated` 本地 package，固定 `0.0.0` 仅作未发布本地契约身份。两次 fresh package bytes 必须相同；最终在 ignored `web-dm/packages/contracts-generated/dist/` 产生 `.tgz`，记录 descriptor SHA-256、source-tree digest 与 package SHA-256。不得发布 registry，也不得修改 Portfolio WebApp。 |
| Fixture 与端口 | fixture 至少有 1 Book、1 Group、2 Portfolio、每个 Portfolio 的 exact PositionSnapshot、中国国债/曲线/市场事实、Benchmark snapshot 和 Convention v1。bootstrap 可重复且不制造重复可见对象。Docker 开发 gRPC-Web 固定为 `http://127.0.0.1:18080`，允许 origin 精确包含 `http://127.0.0.1:5173`；fixture Researcher/scopes 在文档和 SIT 完全一致。 |
| 回归 | fixed Buf format/lint、双 fresh tree、descriptor、三语言 consumer、contract package、domain/application/storage/API/server focused tests、P01/P03/P04 native gRPC SIT、session→GetPage gRPC-Web、Oracle、原 R4–R7B 回归和三个统一入口全部通过。 |

RED-first 子循环在 Human 批准后按依赖图执行，并保留每条轨首次真实非零命令、exit code、首个失败测试与首错；计划命令不写成通过：

1. **W1 · Contract / Domain：** 先让 descriptor、三语言 consumer、17-service inventory、68/6/62 coverage 与 domain invariants 因缺少 Portfolio contract 而 RED；再只做 proto/generated/domain 加法。W1 独占 `interface/`、三棵 generated tree、contract tests、domain Portfolio 路径。
2. **W2 · Catalog / Storage：** 依赖 W1；先以 as-of/knowledge、cursor scope、hash/owner/time tamper 和 migration round-trip RED，再实现只读 catalog repository、migration 与 fixture seed。W2 独占 application catalog port/use case、storage/migration/catalog tests。
3. **W3 · Aggregation / Oracle：** 依赖 W1，可与 W2 并行使用内存 repository；先建立独立 Decimal Oracle、reuse spies 和漂移零调用 RED，再实现 PortfolioOverview/formal evidence。W3 独占 aggregation/workbench application modules与新 Oracle，不修改既有数值实现。
4. **W4 · API / Server / Package：** 依赖 W2+W3；先让三个新 service 的 API/SIT、17-route topology、gRPC-Web session→GetPage 和双 package digest RED，再组合生产 adapter、fixture bootstrap、CORS/scopes 与本地 package。W4 独占 API/server/dev/package 路径。
5. **Root integration：** Root 只整合完成的 direct sibling Worker 结果、核对实际 diff 和 protected facts，运行 focused→fast→full→integration。Worker 不得创建下级 Agent；任何公共字段、公式、允许路径或受保护事实变化都返回 Root/Human 决策。

## 3. 非目标

- 不修改、接入或构建 `C:\git\ficant-portfolio`；不在 FICANT 增加 React、PageModel、PageLayout、11 类 UI module、InsightRail、Playwright 页面或 App Registry 项。
- 不实现其余十九页、ListWorkspaces、ListPages、Execute、导出、保存视图、告警确认或工作流推进。
- 不承担 OMS/EMS、报单、交易写入、成本批次、投资组合会计、总账、NAV、清算、结算、估值锁定或监管报表。
- 不实现年化收益、波动率、Sharpe、Calmar、最大回撤、Campisi、多因子归因、VaR、完整情景、OAS、基金/委外穿透、非标、信用债、美国国债、多币种 FX 或完整 Benchmark 发布工作流。
- 不新建 Position 表、组合 PositionSnapshot、债券/曲线/KRD 算法或平行 REST；不把 existing PositionSnapshot 的 economic P&L 猜成日收益或 NAV。
- 不改变 R5D `AnalyzeBondRequest` exact Bond/Calendar/DataSnapshot/TaxRulePack 物化合同，不恢复 inline BondTerms、曲线节点、价格副本或通用 RulePack；公共金融数值不使用 f64。
- 不改变 R7B FormalOutputEvidence canonical identity、Code/Runtime 语义、现有正式输出、恢复协议或既有 Golden/Oracle/expected/容差。
- 不接入 COGA，不让 COGA、Portfolio WebApp 或新 package 成为 FICANT 运行时依赖。
- 不读取、修改或删除本地 ignored `SPEC.md`/`ACCEPTANCE.md`/`MANUAL.md`，也不修改两份未跟踪审计报告。
- 不创建版本号、tag、镜像发布、部署、远端 CI/CD、GitHub workflow/权限/安全设置或 branch protection 变化。

## 4. 公共契约变化

公共变化只做加法。新建 `interface/proto/ficant/portfolio/v1/portfolio.proto`，package 固定为 `ficant.portfolio.v1`；现有 `analytics.proto`、Position、Exposure、Definition、Fact 与十四个 service 不改 tag、方法或语义。

`FormalInputKind` 在既有 `0..15` 后追加且不重排：`PORTFOLIO = 16`、`BOOK = 17`、`PORTFOLIO_GROUP = 18`、`BENCHMARK = 19`、`PORTFOLIO_METRIC_CONVENTION = 20`。Human 于 2026-08-21 补充批准在其后追加 `FACT = 21`，用于精确标识 P04 与 PortfolioOverview 实际消费的 `MarketFact`；不得把 Fact 伪装成 Definition 或 Snapshot。以上 kind 复用既有 `FormalInputBinding.object_ref` 与 canonical v1 identity；不得新增平行 evidence、重排旧值或改写 R7B hash 算法。

Human 于 2026-08-21 另行批准以追加式 `ValuationValueRole` 修复 R8A 与既有 D-019 的语义冲突：`PRICE = 1`、`YIELD = 2`、`REMAINING_YEARS = 3`，`Valuation.value_roles = 10`。旧事实省略该字段时继续规范化为全部 `PRICE`，其既有 canonical bytes、摘要与存储编码保持不变；新事实若显式携带角色，则角色数必须与 `values` 完全相等且不得出现 `UNSPECIFIED`。R8A authority 必须验证场景值角色与 `PRICE_IN/YIELD_IN` 一致，并验证剩余期限角色为 `REMAINING_YEARS`；禁止按 ordinal 猜测单位语义。

### 4.1 不可变 Portfolio 领域消息

- `PortfolioStatus`：`UNSPECIFIED=0`、`ACTIVE=1`、`SUSPENDED=2`、`CLOSED=3`。
- `PortfolioSnapshotBinding`：`snapshot_id=1`、`content_hash=2`、`observed_at=3`、`visible_at=4`。
- `BenchmarkRef`：`benchmark=1`（VersionRef）、`content_hash=2`。
- `PortfolioMetricConventionRef`：`convention=1`（VersionRef）、`content_hash=2`。
- `Book`：`book=1`（VersionRef）、`owner=2`、`subject_ref=3`、`code=4`、`display_name=5`、`status=6`、`effective_from=7`、`effective_to=8`、`content_hash=9`。
- `PortfolioGroup`：`group=1`、`owner=2`、`subject_ref=3`、`book=4`（exact LineageRef）、`parent_group=5`（optional exact LineageRef）、`code=6`、`display_name=7`、`status=8`、`effective_from=9`、`effective_to=10`、`content_hash=11`。
- `Portfolio`：`portfolio=1`、`owner=2`、`subject_ref=3`、`book=4`、`group=5`（二者为 exact LineageRef）、`code=6`、`display_name=7`、`status=8`、`position_snapshot=9`、`benchmark=10`、`metric_convention=11`、`effective_from=12`、`effective_to=13`、`content_hash=14`。

MetricConvention 只冻结本轮点时指标，不伪装成完整绩效口径：

- `PortfolioMetricWeighting`：`UNSPECIFIED=0`、`MARKET_VALUE=1`、`MARKET_VALUE_TIMES_MODIFIED_DURATION=2`、`NOTIONAL=3`。
- `PortfolioDecimalRounding`：`UNSPECIFIED=0`、`TIES_TO_EVEN=1`。
- `PortfolioMetricConvention`：`convention=1`、`owner=2`、`schema_id=3`（固定 `ficant.portfolio-metric-convention.v1`）、`ytm_weighting=4`、`duration_weighting=5`、`convexity_weighting=6`、`coupon_weighting=7`、`remaining_life_weighting=8`、`rounding=9`、`freshness_limit_seconds=10`、`effective_from=11`、`effective_to=12`、`content_hash=13`。

### 4.2 Catalog 与 Aggregation

`ListBooksAndPortfoliosRequest` 固定为 `owner=1`、`subject_ref=2`、`as_of=3`、`knowledge_at=4`、`statuses=5`、`search=6`、`page=7`。`PortfolioCatalogPage` 固定为 `books=1`、`groups=2`、`portfolios=3`、`page=4`、`read_evidence=5`。`ListBooksAndPortfoliosResponse` 以 oneof `catalog=1 | error=2` 返回。`PortfolioCatalogService` 只含一个 unary RPC `ListBooksAndPortfolios`，没有写方法。

Context 与 scope 使用 typed selector，浏览器不提交 exact hash：

- `PortfolioScopeSelector` oneof：`book_id=1 | group_id=2 | portfolio_id=3`；`ExactPortfolioScope` oneof：`book=1 | group=2 | portfolio=3`（均为 LineageRef），并以 `member_portfolios=4` 返回稳定排序的实际成员。
- `PortfolioCurrencyMode`：`UNSPECIFIED=0`、`ORIGINAL=1`、`CNY=2`；R8A 不声明 USD/FX。
- `PortfolioLookThroughMode`：`UNSPECIFIED=0`、`NONE=1`、`CONSOLIDATED=2`、`SEPARATE=3`。它只展开 Book/Group/Portfolio 目录层级，不表示基金或委外穿透。
- `PortfolioPeriodPreset`：`UNSPECIFIED=0`、`ONE_DAY=1`、`SEVEN_DAYS=2`、`THIRTY_DAYS=3`、`YEAR_TO_DATE=4`、`ONE_YEAR=5`。
- `PortfolioContextInput`：`scope=1`、`valuation_at=2`、`knowledge_at=3`、`currency=4`、`look_through=5`、`benchmark_id=6`、`period=7`。
- `NormalizedPortfolioContext`：`scope=1`、`valuation_at=2`、`knowledge_at=3`、`currency=4`、`currency_unit=5`、`look_through=6`、`benchmark=7`、`period=8`、`period_from=9`、`period_to=10`、`metric_convention=11`。Server 必须重新解析并精确比较任何 caller-supplied normalized context。

`PortfolioBasicMetrics` 只含 DecimalValue：`market_value=1`、`economic_pnl=2`、`weighted_ytm=3`、`modified_duration=4`、`convexity=5`、`weighted_coupon_rate=6`、`weighted_remaining_years=7`、`dv01=8`。字段不可计算时保持 absent 并由 coverage 解释，禁止零值兜底。`PortfolioKrdSummary` 为 `totals=1`（复用 FactorDv01）、`parallel_dv01=2`。`PortfolioMemberOverview` 为 `portfolio=1`、`position_snapshot=2`、`basic_metrics=3`、`krd_summary=4`。

Human 于 2026-08-21 补充冻结 `PortfolioCoverage`：`participation=1`（复用 `ficant.research.v1.CoverageDeclaration`）、`missing_reasons=2`（稳定排序的 repeated string）。`PortfolioOverview.coverage=8`、`P03Projection.coverage=3` 与 `PortfolioPageEnvelope.coverage=10` 均使用该消息；PARTIAL 时 `missing_reasons` 必须非空，REAL 时必须为空。该补充只完成原 brief 已批准但未指定类型的 coverage tag，不修改既有 `CoverageDeclaration`。

`PortfolioOverview` 固定为 `scope=1`、`position_snapshots=2`、`basic_metrics=3`、`krd_summary=4`、`benchmark_metrics=5`、`benchmark=6`、`metric_convention=7`、`coverage=8`、`members=9`、`request_fingerprint=10`、`formal_evidence=11`。`GetPortfolioOverviewRequest.context=1`；`GetPortfolioOverviewResponse` oneof `overview=1 | error=2`。`PortfolioAggregationService` 只含 `GetPortfolioOverview`。

### 4.3 最小 Workbench BFF

- `PortfolioWorkbenchPageId`：`UNSPECIFIED=0`、`D01=1`、`P01=2`、`P02=3`、`P03=4`、`P04=5`。
- `PortfolioPageDataMode`：`UNSPECIFIED=0`、`REAL=1`、`PARTIAL=2`、`STALE=3`、`ERROR=4`；没有 DEMO。
- `PortfolioPageState`：`UNSPECIFIED=0`、`READY=1`、`EMPTY=2`、`BLOCKED=3`。
- `PortfolioWorkbenchErrorCode`：`UNSPECIFIED=0`、`UNAUTHENTICATED=1`、`FORBIDDEN=2`、`NOT_FOUND=3`、`CONFLICT=4`、`STALE=5`、`INTEGRITY=6`、`UNAVAILABLE=7`。`PortfolioWorkbenchTypedError` 为 `code=1`、`safe_message=2`、`trace_id=3`、`retryable=4`；输入解码/缺字段仍使用标准 gRPC INVALID_ARGUMENT 与现有安全 ErrorDetail，不把 SQL、凭据或隐藏对象写入消息。
- `NonFormalReadEvidence`：`schema_id=1`、`consumed_inputs=2`（FormalInputBinding）、`request_fingerprint=3`。`PortfolioPageProvenance`：`owner=1`、`subject_ref=2`、`request_fingerprint=3`、`formal_evidence=4`、`non_formal_reads=5`；两组都稳定排序且不能互相冒充。
- `PortfolioPageSelection.instrument=1` 只用于 P04；server 必须证明该 instrument exact version 存在于 resolved PositionSnapshot。其他页面携 selection 失败。
- `GetDefaultContextRequest`：`owner=1`、`subject_ref=2`、`knowledge_at=3`；response oneof `context=1 | error=2`。默认值来自当前知识时点下稳定排序的首个有权 active Portfolio，不用 server-clock 或 UI hardcode。
- `GetPortfolioPageRequest`：`page_id=1`、`context=2`、`selection=3`。
- `PortfolioPageEnvelope`：`schema_version=1`（固定 `portfolio-workbench.v1`）、`page_id=2`、`request_id=3`、`generated_at=4`（Timestamp）、`data_mode=5`、`normalized_context=6`、`page_state=7`、`permissions=8`、`provenance=9`、`coverage=10`，oneof projection 为 `d01=11 | p01=12 | p02=13 | p03=14 | p04=15`，`typed_error=16`。ERROR 时 projection 必须为空；非 ERROR 时 typed_error 必须为空。
- `D01Projection`/`P02Projection` 只携 PortfolioOverview；`P01Projection` 携 PortfolioCatalogPage 与只由目录计数产生的 StructureMetrics；`P03Projection` 直接携既有 PositionViews、PortfolioKeyRateExposure 与 coverage；`P04Projection` 直接携既有 MarketDefinition、InstrumentFacts、AnalyzeBondResult。没有 layout、module、chart、action 或前端 display string。
- `PortfolioWorkbenchService` 只含 `GetDefaultContext` 与 `GetPage`；`GetPage` 直接返回 PageEnvelope，使业务错误能以 `data_mode=ERROR + typed_error` 安全呈现。畸形 wire、超限或无法认证的 transport failure仍可使用 gRPC status，但不得回退 mock。

`AnalyzeBondRequest`、`DecimalValue` 与既有 RPC 保持当前 R5D/R7B 形状；R8A 只消费它们，不改 tag、不加 inline 条款或 caller price 副本。

## 5. 需 Human 决策

Human 于 2026-08-21 对本 brief 的“批准”同时冻结下表的 R8A 选择、§4 公共 shape、§6 写路径和 execution base。任何变更必须在首次相关代码写入前重新取得明确授权。

| 决策 | 建议冻结选择 | 排除边界 |
|---|---|---|
| D1 · 产品/服务边界 | FICANT 平台名不变；产品文案使用“金证FICC合同管理系统”；R8A 新增 Catalog、Aggregation、Workbench 三个只读 service，五页真实纵切。 | 不接 WebApp/COGA，不把 UI module 或二十四页清单变成领域对象。 |
| D2 · “收益”含义 | R8A 只返回已导入 `economic_pnl`、AnalyzeBond YTM 与批准的点时加权指标；不把 PositionSnapshot P&L 推导为日收益/NAV。 | 年化算术/几何收益、波动、Sharpe、Calmar、最大回撤和 benchmark excess return 需要新的 NAV/return-series authority，明确顺延，不以 PDF 公式越权实现。 |
| D3 · 点时指标 | market value/economic P&L 逐仓位同 Unit 精确求和；YTM 用 `market_value × modified_duration` 加权；修正久期/凸性用 market value 加权；票息/剩余期限用正 notional 加权；DV01/KRD 直接复用 PortfolioRisk。 | 不实现 OAS；不重算 KRD；不接受零分母、混 Unit、缺 duration 或未通过 AnalyzeBond 的仓位。 |
| D4 · Decimal/舍入 | 中间值只用 Decimal/FixedDecimal；先按公共单位归一，再在最终输出 Unit scale 做 ties-to-even；overflow、零分母或无法表达失败关闭。Oracle 使用相同公开 convention 但独立公式。 | 不用 f64，不用 epsilon，不为通过测试修改既有容差或 expected。 |
| D5 · short/不完整仓位 | market value/economic P&L 与 KRD 保留有符号事实；加权平均仅对正 long bond participating set计算。存在 short、非债、缺字段时相关平均字段 absent并返回 PARTIAL/coverage；若 caller 声明要求完整范围则 ERROR。 | 不把 short 绝对值化、不静默排除后仍标 REAL、不注入零指标。 |
| D6 · Benchmark | BenchmarkRef 指向只读、版本化 benchmark catalog 记录及其 exact PositionSnapshot；用相同 convention 计算点时对照。 | 不实现 Benchmark 写/发布、指数成分时序、收益内连接或插值。 |
| D7 · 六维 P0 | currency 只支持 ORIGINAL/CNY；非 CNY 的 ORIGINAL 必须同 Unit，否则 ERROR。lookThrough 只展开 Book/Group/Portfolio 目录：NONE=直接成员、CONSOLIDATED=所有后代合计、SEPARATE=后代逐项。period 只控制 fact/观测查询窗，不宣称绩效序列。 | USD/FX、基金/委外穿透、跨币种汇总和 period 收益指标顺延；unsupported 返回 typed UNAVAILABLE。 |
| D8 · Freshness | fixture 的 Convention v1 固定 freshness limit `86400` 秒；所有时间比较保留 seconds+nanos/timezone/local date。超过限制但 hash/双时间合法为 STALE；未来可见、时区或 local-date 漂移为 ERROR。 | 不用 server 当前日期猜交易日，不把 stale 冒充 partial/real。 |
| D9 · 正式证据 | PortfolioOverview 是正式分析并经 FormalOutputPublisher 持久化；Catalog、Definition、Fact 是显式非正式读取；PageEnvelope 只组合证据，不建立第二 identity。 | 不修改 R7B canonical algorithm，不给 CRUD 读取伪造 result hash/output identity。 |
| D10 · 本地契约包 | 在 FICANT 内新增 `@ficant/contracts-generated@0.0.0` 可重复 `.tgz`，只作本地未发布 artifact；记录 descriptor/tree/package 三摘要。 | 不发布 npm、GitHub Package 或版本 tag；不修改 Portfolio WebApp alias，本轮只交付可消费路径。 |
| D11 · 开发接线 | Docker gRPC-Web 为 `http://127.0.0.1:18080`，开发 origin 精确加入 `http://127.0.0.1:5173`；fixture identity 为 Researcher，scopes 固定 `portfolio:read,positions:read,rates:analyze,facts:read,definitions:read,artifacts:read`。 | `127.0.0.1:50051` 只在直接本地 server 绑定时使用，WebApp 不猜 native/gRPC-Web 端口；不放宽 `*` CORS。 |
| D12 · 交付 | OPAID 只形成本地 self-tested 候选和 Human handoff；Human 后续另行决定提交、PR、合并或发布。 | 本 brief 不授权 push、远端 CI/CD、tag、镜像或部署。 |
| D13 · Fact 正式证据 | Human 于 2026-08-21 补充批准追加 `FormalInputKind::Fact = 21`，P04/Overview 对 exact Valuation/MarketFact 的正式或非正式证据必须使用该 kind。 | 不重排 `0..20`，不改 canonical identity，不把 Fact 错标成 Definition/DataSnapshot。 |
| D14 · KRD 币种 Unit | Human 于 2026-08-21 补充批准 KRD 仅在既有校验处同时接受 `currency` 与 `currency_amount` dimension；Bond 与 Curve 仍必须引用同一个 exact UnitRef。 | 不改 KRD 公式、输入选择、算法身份、容差、Oracle 或 expected。 |
| D15 · Valuation measure roles | Human 于 2026-08-21 补充批准 `Valuation.value_roles=10` 的追加式角色数组；旧省略值保持全 `PRICE` 与旧摘要，新 typed fact 的角色、Unit dimension、内容摘要和存储 bytes 全部相互绑定。 | 不把第二个 value 固定解释为 YEARS，不通过 method 字符串或调用方序号猜语义，不回写平台生成分析结果为外部事实。 |
| D16 · 一方包供应链 | Human 于 2026-08-21 补充批准供应链 verifier 的精确一方包数量由 19 更新为 20，并绑定最终 supply lock SHA；新增项是本轮本地 `@ficant/contracts-generated@0.0.0`。 | 不发布 npm，不放宽精确计数、license 或摘要校验。 |
| D17 · 机械 consumer 闭合 | Human 于 2026-08-21 补充批准仅修改 `crates/ficant-application/tests/unit_semantic_proof.rs`、`crates/ficant-api/tests/market_fact_service.rs` 与 `binaries/ficant-worker/src/production.rs`，分别接纳 typed Valuation Unit roles、追加字段的 legacy/negative transport fixture，以及 `FormalInputKind` 16..21 的精确 worker 映射。 | 不扩大生产语义，不改变旧 Valuation、金融公式、摘要、Oracle、expected 或 tolerance。 |
| D18 · 债券仓位 KRD 最终舍入 | Human 于 2026-08-21 补充批准新增单一 Domain helper，把冻结的方向价差、bump、signed quantity 与 registered face 作为同一有理式约分，只在最终仓位 DV01 的 12 位固定尺度执行 ties-to-even；Application 债券仓位路径改为调用该 helper。 | 不改变既有 `key_rate_dv01`、期货 KRD、方向公式、输入选择、算法身份、Unit/owner/ref 校验、容差、任何既有 Oracle/expected，也不以调票息或持仓数量换取测试通过。 |
| D19 · R7A 哈希闭合 | Human 于 2026-08-21 批准只更新 `crates/ficant-contract-tests/tests/fixtures/r7a-core-extension/core-source-sha256.tsv` 中 D18 已授权的两行：`research/exposure.rs` → `5164c871bfbb1428e47bbfc4f62e499ba809f2fba72e8d7801a4905845c9eb83`，`research/mod.rs` → `7cc50ece586e1ae0311dd3d990414aefb950bfe82823a1640f098b61524e45ff`。47 文件集合不变。原 §6 清单不改。 | 不增删 47 文件，不改 C++/native/其余 45 个源，不改变 R7A「不拆 crate」裁决，不把其他哈希或 Oracle/expected 一并改写。 |
| D20 · 共享库 SIT reset | IncludeIntegration 真实失败：`phase4_worker_sit` 在已应用 0026 的库上 reset 时未 DROP `portfolio`，随后 `CREATE SCHEMA portfolio` 报 `42P06`。已机械补齐与已批准 `ficant-storage/tests/support` 相同的 `DROP SCHEMA IF EXISTS portfolio CASCADE`，仅限这些共享库 reset 副本：`binaries/ficant-worker/tests/support/mod.rs`、`binaries/ficant-server/tests/r6a_governed_input_sit.rs`、`binaries/ficant-server/tests/r6b_artifact_service_sit.rs`、`crates/ficant-acceptance/tests/phase1_business_loop.rs`、`crates/ficant-acceptance/tests/negative_invariants.rs`、`crates/ficant-data/tests/snapshot_publication_sit.rs`、`crates/ficant-data/tests/dual_source_sit.rs`。原 §6 清单不改。 | 不改 migration 0026、生产 schema 语义、Oracle/expected，也不把其他 SIT 逻辑或 fixture 一并改写。 |

先前资料建议的“252 交易日、对外几何/并列算术、样本标准差、无风险利率 0、共有日期内连接、峰谷回撤”不在 R8A 冻结公式中：当前 FICANT 没有可证明的 Portfolio NAV/return series、现金流调整或 Benchmark return authority。若未来 Human 选择新增该输入面，必须新开迭代并用独立数值样例重新批准，不能回填到本轮 convention。

## 6. 最终真实测试证据

**R8A execution base（已批准并冻结）：** public commit `11015f41b3f58e82017e85a834f2ba227b702ca2`，tree `b5afe57f443cae36b47216ffd9e4ba518650aa12`；批准时 `main == origin/main`。实施从该精确 base 新建本地分支；不得把随后移动的 main 当作浮动 base。本 brief 规划写入前工作树只含必须保留的未跟踪 `docs/review/full-audit-2026-08-07.md` 与 `docs/review/full-audit-2026-08-19.md`。

**实施允许写路径（Human 已批准并冻结的闭集）：**

- `docs/iterations/2026-08-r8a-portfolio-p0.md`（本文件；实施后只在本节追加同一最终候选的真实证据，并更新第 7 节风险）
- `docs/iterations/README.md`
- `README.md`
- `docs/development.md`
- `docs/product/scope.md`
- `docs/quality/evidence.md`
- `docs/interface/ui-reference.md`
- `docs/architecture/layering-refactor.md`
- `docs/architecture/data-dictionary.md`
- `Cargo.lock`
- `interface/proto/ficant/core/v1/evidence.proto`（Human 补充批准只追加 16..21 enum values）
- `interface/proto/ficant/market/v1/fact.proto`（Human 补充批准只追加 `ValuationValueRole` 与 `Valuation.value_roles=10`；旧 tag/语义不变）
- `interface/proto/ficant/portfolio/v1/portfolio.proto`（新建）
- `crates/ficant-contracts/src/generated/**`
- `crates/ficant-contracts/src/lib.rs`（Human 于 2026-08-21 补充批准；只导出新 `ficant.portfolio.v1` generated package）
- `python/node-contracts/src/ficant_contracts/generated/**`
- `web-dm/packages/contracts-generated/src/**`
- `crates/ficant-contract-tests/Cargo.toml`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contract-tests/tests/r7b_formal_evidence.rs`（只允许接纳新增 typed input kinds/PortfolioOverview carrier，不改既有 0..15 或 identity 断言）
- `crates/ficant-contract-tests/tests/r8a_portfolio_contract.rs`（新建）
- `python/tests/test_contract_import.py`
- `web-dm/platform-shell/tests/contracts-consumer.test.ts`
- `.github/scripts/verify-contract-generation.sh`（只更新最终 descriptor/generated 摘要）
- `.github/scripts/verify-supply-chain.sh`（Human 补充批准只把精确一方包数量 19 更新为 20，并绑定最终 supply lock SHA）
- `.github/scripts/license-inventory.lock.json`（只允许既有工具机械 refresh bindings）
- `.github/scripts/supply-chain.lock.json`（只允许既有工具机械 refresh 新 package/lock 事实）
- `web-dm/.gitignore`
- `web-dm/package.json`
- `web-dm/pnpm-workspace.yaml`
- `web-dm/pnpm-lock.yaml`
- `web-dm/packages/contracts-generated/package.json`（新建）
- `web-dm/packages/contracts-generated/README.md`（新建，只说明生成来源与本地安装）
- `scripts/package-contracts.ps1`（新建）
- `scripts/test-contract-package.ps1`（新建）
- `crates/ficant-domain/Cargo.toml`
- `crates/ficant-domain/src/lib.rs`
- `crates/ficant-domain/src/market/mod.rs`（Human 补充批准只导出 typed Valuation role）
- `crates/ficant-domain/src/market/valuation.rs`（Human 补充批准追加 role，不改变旧省略角色事实的 canonical 语义）
- `crates/ficant-domain/src/portfolio/mod.rs`（新建）
- `crates/ficant-domain/src/research/exposure.rs`（Human 于 2026-08-21 补充批准；只新增 D18 债券仓位 KRD 单次最终 ties-to-even helper，既有函数语义不变）
- `crates/ficant-domain/src/research/mod.rs`（Human 于 2026-08-21 补充批准；只导出 D18 helper）
- `crates/ficant-domain/tests/r8a_portfolio_contracts.rs`（新建）
- `crates/ficant-domain/tests/r8a_bond_position_krd_rounding.rs`（Human 于 2026-08-21 补充批准；只验证 D18 精确、half-even、符号与溢出/非法分母边界）
- `crates/ficant-runtime/src/native_execution.rs`（Human 于 2026-08-21 补充批准；只追加 FormalInputKind 16..21，不改变 canonical identity 算法）
- `crates/ficant-runtime/tests/native_execution.rs`（Human 于 2026-08-21 补充批准；只验证新增 kind code 与既有 identity 不回退）
- `crates/ficant-application/Cargo.toml`
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/ports/unit_resolution.rs`（Human 补充批准只解析显式 Valuation role 并验证 exact Unit dimension）
- `crates/ficant-application/src/ports/fingerprint.rs`（Human 补充批准只让新 typed Valuation role 进入 v2 摘要，旧全 PRICE 事实保持 v1）
- `crates/ficant-application/src/ports/portfolio.rs`（新建）
- `crates/ficant-application/src/use_cases/mod.rs`
- `crates/ficant-application/src/use_cases/portfolio_catalog.rs`（新建）
- `crates/ficant-application/src/use_cases/portfolio_aggregation.rs`（新建）
- `crates/ficant-application/src/use_cases/portfolio_workbench.rs`（新建）
- `crates/ficant-application/src/use_cases/portfolio_risk.rs`（Human 补充批准在既有 Unit 校验处兼容 `currency`/`currency_amount`；不改算法、输入选择或容差）
- `crates/ficant-application/src/use_cases/position_views.rs`（同上）
- `crates/ficant-application/src/use_cases/rates_materialization.rs`（同上，不改 R5D materialization）
- `crates/ficant-application/src/use_cases/formal_outputs.rs`（只允许接入 PortfolioOverview schema）
- `crates/ficant-application/tests/r8a_portfolio_catalog.rs`（新建）
- `crates/ficant-application/tests/r8a_portfolio_aggregation.rs`（新建）
- `crates/ficant-application/tests/r8a_portfolio_workbench.rs`（新建）
- `crates/ficant-application/tests/unit_semantic_proof.rs`（Human 补充批准只补 `Rate`/`Years` 的 exact dimension consumer）
- `migrations/postgresql/0026_r8a_portfolio_p0.sql`（新建）
- `crates/ficant-storage/Cargo.toml`
- `crates/ficant-storage/src/postgres/mod.rs`
- `crates/ficant-storage/src/postgres/codec.rs`（Human 补充批准只为 typed Valuation role 增加新编码；旧 Valuation bytes 不变）
- `crates/ficant-storage/src/postgres/formal_outputs.rs`（Human 于 2026-08-21 补充批准；只补新增 FormalInputKind 的双向持久化映射）
- `crates/ficant-storage/src/postgres/portfolio.rs`（新建）
- `crates/ficant-storage/tests/support/mod.rs`（只增加 R8A schema/table reset）
- `crates/ficant-storage/tests/migration_acceptance.rs`
- `crates/ficant-storage/tests/r8a_portfolio_postgres.rs`（新建）
- `crates/ficant-api/Cargo.toml`
- `crates/ficant-api/src/lib.rs`
- `crates/ficant-api/src/grpc_web.rs`
- `crates/ficant-api/src/formal_evidence.rs`（只映射新增 input kinds）
- `crates/ficant-api/src/market_fact.rs`（Human 补充批准只解析/投影 typed Valuation role，旧 Fact RPC 行为兼容）
- `crates/ficant-api/src/portfolio_catalog.rs`（新建）
- `crates/ficant-api/src/portfolio_aggregation.rs`（新建）
- `crates/ficant-api/src/portfolio_workbench.rs`（新建）
- `crates/ficant-api/tests/portfolio_catalog_service.rs`（新建）
- `crates/ficant-api/tests/portfolio_aggregation_service.rs`（新建）
- `crates/ficant-api/tests/portfolio_workbench_service.rs`（新建）
- `crates/ficant-api/tests/market_fact_service.rs`（Human 补充批准只适配追加 `value_roles` 的 legacy fixture，并验证 typed/非法角色 transport）
- `binaries/ficant-worker/src/production.rs`（Human 补充批准只补 `FormalInputKind` 16..21 的 domain→proto 精确映射）
- `binaries/ficant-server/Cargo.toml`
- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/composition.rs`
- `binaries/ficant-server/tests/service_topology.rs`
- `binaries/ficant-server/tests/r8a_portfolio_sit.rs`（新建）
- `binaries/ficant-server/examples/r8a_portfolio_bootstrap.rs`（新建，fixture-only）
- `deploy/dev/docker-compose.yml`（只增加 exact Portfolio scopes/origin 与 fixture接线，不改变生产默认拓扑）
- `scripts/dev-up.ps1`（只增加 exact Portfolio scopes/origin 与 bootstrap提示）
- `scripts/bootstrap-portfolio-p0.ps1`（新建）
- `scripts/check-coverage.ps1`（只更新冻结 68/6/62 inventory 文案/判据）
- `scripts/test-coverage-check.ps1`（只增加新 carrier 的真实负例）
- `scripts/check-fast.ps1`（只加入 R8A contract/package focused gate）
- `scripts/check.ps1`（只加入 R8A Oracle/package/integration gate）
- `tests/fixtures/portfolio/**`（仅新建 R8A bootstrap 数据，不复用或修改既有 Golden/Oracle）
- `tests/oracle/portfolio/r8a_portfolio_metric_inputs.json`（新建）
- `tests/oracle/portfolio/r8a_portfolio_metric_expected.json`（新建）
- `tests/oracle/portfolio/r8a_portfolio_metric_decimal_oracle.py`（新建）
- `tests/oracle/portfolio/test_r8a_portfolio_metric_decimal_oracle.py`（新建）

**受保护事实：** `C:\git\ficant-portfolio/**`、`C:\git\cogawork/**`、`SPEC.md`/`ACCEPTANCE.md`/`MANUAL.md`、两份未跟踪审计报告、R5D AnalyzeBond proto/物化、R7A 47-file core manifest、R7B identity/recovery、十四个既有 service 与生产 route、所有既有 Golden/Oracle/expected/容差、C/C++/FFI/native 数值实现、RulePack 内容、`.github/workflows/**`、`cicd.yml`、`deploy/test/**`、远端 GitHub 设置、版本/tag/镜像/部署均不修改。新增 migration/fixture 不得改写已有 `0001..0025` 或已有 fixture 事实。

本节以下证据全部来自同一个最终代码候选，不把计划命令、早期脏工作树或文档提交写成代码已通过。**最终代码候选：** commit `ff0af1575eae6042315955e0ec992877dbffaa8a`，tree `563d144731bda596dd4c32ddf49e7c0c172ab251`；父提交为冻结 execution base `11015f41b3f58e82017e85a834f2ba227b702ca2` / tree `b5afe57f443cae36b47216ffd9e4ba518650aa12`。后续若只修改本文件，该文档提交不冒充重新执行完整检查的代码候选。取证时 Node 为 `v22.17.0`。两份未跟踪审计报告在 recovery 期间暂时移出工作树，取证结束后已放回，未纳入候选。

### 6.1 本地检查与测试计数

| 实际入口/证据 | 结果 |
|---|---|
| `.\scripts\check.ps1 -IncludeIntegration`（在干净 `ff0af1575eae6042315955e0ec992877dbffaa8a` 上） | exit `0`，约 19.7 分钟，输出 `FICANT complete local checks passed.` Coverage 68 reachable arms / 6 composition carriers / 62 具名非组合 arm。R8A Oracle 11/11；license digest `a9e3c6923d3b73ce8894e5a075db661ce5b35b60547fdb19f0ca8094aa3a09e4`；Python generated consumer 1 passed / 1 environment-gated skipped；本地契约包 tests 6，`@ficant/contracts-generated@0.0.0`，descriptor SHA-256 `3c97ce22d4ced6e9f082e4f684a5a507096a968bf8ef02a6da651f120bb2cc68`，source-tree SHA-256 `2c24c4e06b4c732887284497dd919a3dd2bee9d8278003c726528603354f1f06`，source_file_count 30，package SHA-256 `4d6b12bd212d97040ea0f99049cc48d004bc69cbc15f1def95fd9b4da68c2917`；Web tests 35/35。PostgreSQL migrations 7/7、lease 1/1、execution 3/3、Worker 1/1、Phase 1 1/1、negative invariants 13/13；R8A Postgres 5/5；R8A production gRPC/gRPC-Web SIT 1/1（P01/P03/P04 native 与 session→GetPage）；R7B recovery proof passed，manifest SHA-256 `7EB8D0A9CE1C6808F13BE9FAD7448ECBF0ED3CE6E14B36EE74B77BAA43BDEABA`。 |
| `git diff --check`（相对该代码候选 HEAD） | exit `0` |

Windows 本机 `check.ps1` 不运行 Linux/BSR 双 fresh Buf generation；descriptor SHA 由契约包门禁记录，不能代替 `.github/scripts/verify-contract-generation.sh` 的双树证明。

### 6.2 实施期失败与就地修复

同一候选上真实失败并修复、未降低 Oracle/expected/容差：Docker Desktop 停机导致 `PoolTimedOut`；共享库 SIT reset 未 DROP `portfolio`（`42P06`，D20）；license first-party binding 因测试 reset 源变化失配后用既有 `refresh-bindings` 刷新；生产 Aggregation 把 R5D AnalyzeBond Subject hash 与 `subject_record_content_hash` 当成同一摘要而误报完整性错误，改为只核对同一 `VersionRef`。

## 7. 残余风险

- 现有 PositionSnapshot 没有 NAV、外部现金流调整或日收益权威，因此本轮诚实边界是点时持仓/收益率/风险研究，不是绩效、会计或完整组合投研产品。
- fixture 只证明 CNY、中国国债、正 long bond 与最小目录层级；short/非债/多币种会显式 partial/error，不代表完整资产覆盖。
- Workbench 是最小读 BFF，不是 WebApp PageModel；`C:\git\ficant-portfolio` 仍需后续独立任务安装锁定 `.tgz`、移除源码 alias 并完成 Hybrid UI 接线，本轮不触碰该仓库。
- 本地 `.tgz` 没有 registry、长期版本或远端分发保证；它只为相邻 WebApp 的下一次独立迭代提供可校验输入。
- fresh contract generation 仍依赖 BSR remote plugins；本候选未在 Windows 上取得双 fresh tree 命令证据，不能用旧生成树代替。
- Docker gRPC-Web origin 固定为开发 POC 值，不是生产 CORS、身份、UAT 或部署批准。
- 本轮未 push、未创建 tag、未发布镜像、未部署、未触发远端 CI/CD。
