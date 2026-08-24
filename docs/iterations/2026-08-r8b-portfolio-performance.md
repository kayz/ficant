# R8B 迭代 brief — 组合日度计量与收益序列

**面向 Human 的产品名：** 金证FICC合同管理系统 · **平台名：** FICANT · **内部迭代：** R8B · **execution base：** `a66f780c949614ab050a625667b93b129653588f` · **base tree：** `01d41c2cc2a14065bce91d555d43e9c6d0d7c1ee` · **状态：** Human 已于 2026-08-25 批准按建议实施；契约、口径、测试和写入边界已冻结

本 brief 是 R8B 面向 Human 的唯一设计、权限边界和最终证据载体。本轮只在 `C:\git\ficant` 建设后端组合日度计量与收益序列；不接入 `C:\git\ficant-portfolio`，不创建版本、tag、镜像或部署，也不触发远端 CI/CD。

## 1. 目标

R8B 在 R8A 的不可变 Book/Group/Portfolio 目录与 exact scope 之上，新增一条可验证的研究口径日度计量链：不可变 PortfolioValuationSnapshot、不可变 BenchmarkLevelSnapshot、版本化 PortfolioPerformanceConvention，以及 `PortfolioPerformanceService.GetPortfolioPerformance`。

服务按 exact owner、Subject、scope、knowledge-at、Calendar、Unit、PerformanceConvention、Portfolio、PositionSnapshot、Benchmark 与全部快照 required-read，在任何收益计算前核对版本、hash、双时间、币种、成员集合与交易日覆盖。成功输出使用 FixedDecimal、稳定请求指纹和 R7B `FormalOutputEvidence`，并在响应前持久化正式输出。

**Acceptance sentence：**

> 给定有权 Researcher 和一个已由后台精确规范化的 Portfolio context，FICANT 必须解析同一 owner/Subject 下稳定排序的 exact 成员集合，required-read 每个 Calendar 开市日每个成员唯一的收盘 PortfolioValuationSnapshot、同日唯一 BenchmarkLevelSnapshot、exact PerformanceConvention/Calendar/Unit/Portfolio/PositionSnapshot/Benchmark，并按“期末外部现金流、日度时间加权、逐步 ties-to-even、几何累计”计算收益序列；缺失、重复、owner/hash/version/time/unit/session/scope 漂移必须在首次数值运算与正式发布前失败关闭。最终 descriptor 与生产路由由 17 个精确扩为 18 个 service，独立 Decimal Oracle、PostgreSQL、API、native gRPC/gRPC-Web SIT、三语言 consumer、双 fresh generation、结构/供应链门禁及三个统一入口在同一最终候选上通过，R4–R8A 既有 Oracle/expected/容差和 R7B identity 不回退。

## 2. 验收

| 条目 | R8B 可执行判据 |
|---|---|
| 不可变输入 | `PortfolioValuationSnapshot`、`BenchmarkLevelSnapshot` 和 `PortfolioPerformanceConvention` 均 owner/Subject scoped、content-addressed、双时间可判；快照只追加，不提供公共写 RPC。NAV 必须精确等于 gross assets 减 liabilities；快照精确绑定 Portfolio、PositionSnapshot、PerformanceConvention 与 Unit。 |
| 日历覆盖 | PerformanceConvention 精确绑定一个 Calendar。区间按 Calendar 本地交易日筛选开市 session，至少含两个开市日；每个成员每个 session 恰好一个估值快照，每个 session 恰好一个 Benchmark level。缺失、重复、闭市日混入或时区/交易日不符一律 ERROR，不做 inner join、不补零、不降为 partial。 |
| 冻结公式 | 对相邻 session `t-1,t`：`P&L_t = NAV_t - Flow_t - NAV_{t-1}`，正 Flow 表示投资者申购/注资；`R_t = P&L_t / NAV_{t-1}`。Flow 固定期末发生；期初 NAV 必须为正。Benchmark 日收益为同日 level 变化除期初 level。累计收益按 `Π(1+R_t)-1` 逐步执行 scale-12 ties-to-even；active 日/累计收益分别为 Portfolio 与 Benchmark 对应值之差。 |
| Scope 聚合 | Portfolio scope 使用该 Portfolio；Group/Book 使用 R8A normalized context 中的全部 exact member portfolios。每个 session 先按成员稳定顺序求和 NAV/Flow，再计算组合收益；不得先算成员收益后简单平均。成员集合、顺序或任一快照变化都改变指纹和正式血缘。 |
| Decimal/Unit | 所有金额、指数与比例只走 `FixedDecimal`/`DecimalValue`；不使用 f32/f64。所有成员金额 Unit 必须与 normalized currency Unit 完全一致且权威 Unit dimension 为 `currency` 或 `currency_amount`；Benchmark level Unit 必须为 exact `dimensionless`。 |
| 双时间 | `valuation_at` 必须落在 Calendar session 的本地交易日；`visible_at >= valuation_at` 且 `visible_at <= knowledge_at`。Convention/Calendar/Portfolio/Benchmark 在对应时点必须有效且可见。任何漂移在 `ArithmeticWitness`/等价 spy 计数仍为 `0` 时失败。 |
| 正式证据 | 新增 FormalInputKind 22..24：PortfolioValuationSnapshot、BenchmarkLevelSnapshot、PortfolioPerformanceConvention。成功证据还必须包含实际消费的 Subject、Book/Group/Portfolio、PositionSnapshot、Benchmark、Calendar、Unit 与全部快照，按 canonical 排序进入 R7B identity；响应前写入现有 immutable formal-output repository。 |
| Coverage | `PortfolioPerformanceCoverage` 明确 expected/observed session、portfolio-observation 与 benchmark-observation 数量。成功时必须全等且 missing sessions 为空；它是新的 composition carrier，coverage inventory 从 `68/6/62` 精确变为 `69/7/62`。 |
| 授权与错误 | 新 service 只允许 Researcher + `portfolio:read`，继续按 trusted tenant/allowed owner 裁剪；错误使用既有 Core ErrorDetail，不泄露无权对象。请求不得提交快照 payload、NAV、Flow、Benchmark level、Calendar 内容或公式参数副本。 |
| PostgreSQL | migration 0027 新增三类不可变表、精确 FK/唯一约束/查询索引与 UPDATE/DELETE 拒绝触发器。查询只返回 knowledge-at 可见版本；重启读回、hash/time/unit tamper、重复 session 和跨 tenant 均有真实 PostgreSQL 测试。 |
| 服务闭合 | 新增 `PortfolioPerformanceService` 一个 unary RPC；descriptor 与生产 route set 均精确为 18。native gRPC 与 gRPC-Web 必须命中真实 production composition，验证成功、无权、缺日和漂移负例。 |
| Oracle | 独立 Python Decimal Oracle 至少覆盖 2 Portfolio、3 个交易日、期末申购/赎回、负 P&L、Benchmark 与 Group 聚合；Rust 只读取 fixture/expected、调用生产公式并逐字段比对，不导入生产公式。 |
| 回归 | Buf format/lint、双 fresh tree、descriptor、Rust/Python/TypeScript consumer、contract package、focused domain/application/storage/API/server tests、R8B Oracle、R4–R8A 回归、license/supply-chain、`check-fast`、`check`、`check -IncludeIntegration` 与 `git diff --check` 全部通过。 |

RED-first 子循环冻结如下：

1. Contract/Domain：先让新 service、18-service topology、69/7/62 coverage、FormalInputKind 22..24、不可变快照和公式测试因缺失实现而 RED，再只做追加式契约/领域实现。
2. Application/Oracle：先让完整 session matrix、单变量漂移零运算、scope 聚合和 Decimal Oracle RED，再实现 exact materialization 与纯 Decimal 计算。
3. Storage/API：先让 migration、双时间读回、正式发布、transport 与 production route RED，再组合 PostgreSQL、API 与 server。
4. SIT/Supply chain：最后建立 native/gRPC-Web 成功和负例、刷新既有 lock binding、运行 focused→fast→full→integration；任何 Oracle/expected/容差不得为通过而修改。

## 3. 非目标

- 不接入、构建或修改 `C:\git\ficant-portfolio`，不增加页面、React、Playwright 或 App Registry 项。
- 不提供正式投资组合会计、估值锁定、总账、会计分录、OMS/EMS、报单、清算、结算或监管报表；本轮 NAV 是带完整来源证据的研究计量输入。
- 不实现交易/批次成本、应计现金、负债引擎、外部流水导入或 NAV 生产工作流；快照由受信 bootstrap/后续输入迭代产生，本轮没有公共写 RPC。
- 不实现年化收益、年化波动率、Sharpe/Calmar、最大回撤、Campisi/多因子归因、VaR、跟踪误差、动态基准、FX、多币种、基金/委外穿透或模拟组合。
- 不把 R8A PositionSnapshot 的 economic P&L 猜成 NAV/Flow，不重算 Bond/Rates/KRD，不修改任何既有金融 Oracle、expected 或 tolerance。
- 不改变 R5D exact Rates 输入合同、R7B canonical identity/恢复协议、R8A 三个 service/RPC/tag/语义或现有 17 条生产 route。
- 不读取或修改 ignored `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`，不处理审计报告。
- 不创建版本号、tag、镜像、部署，不推送、不触发远端 CI/CD，不修改 GitHub workflow/权限/安全设置。

## 4. 公共契约变化

公共变化仅追加到 `ficant.portfolio.v1` 和 `FormalInputKind`：

- `FormalInputKind` 追加 `PORTFOLIO_VALUATION_SNAPSHOT=22`、`BENCHMARK_LEVEL_SNAPSHOT=23`、`PORTFOLIO_PERFORMANCE_CONVENTION=24`；0..21 不重排。
- 新增 `PortfolioPerformanceConventionRef`；Convention 固定 schema `ficant.portfolio-performance-convention.v1`、exact Calendar、`DAILY_TIME_WEIGHTED`、`END_OF_DAY`、`CALENDAR_SESSION_CLOSE`、`TIES_TO_EVEN`。
- 新增不可变 `PortfolioValuationSnapshot`：identity、owner、Subject、exact Portfolio/PositionSnapshot/Convention、valuation/visible time、currency Unit、gross assets、liabilities、NAV、net external flow、content hash。
- 新增不可变 `BenchmarkLevelSnapshot`：identity、owner、Subject、exact Benchmark、valuation/visible time、dimensionless Unit、positive level、content hash。
- 新增 `PortfolioDailyPerformancePoint`、`PortfolioPerformanceCoverage`、`PortfolioPerformanceSeries`、`GetPortfolioPerformanceRequest/Response`。
- 新增 `PortfolioPerformanceService.GetPortfolioPerformance`。请求只携 `NormalizedPortfolioContext`；server 必须重新解析 scope authority 并逐字段核对 normalized context，不接受任何数值或快照副本。
- `PortfolioPerformanceSeries.formal_evidence` 复用既有 R7B 信封，不新增平行 metadata 或 identity 算法。

## 5. 需 Human 决策

Human 已批准以下冻结项；实施中若要改变，必须先停下并取得新的明确授权：

| 决策 | 已冻结选择 | 边界 |
|---|---|---|
| D1 产品边界 | 后端研究计量与收益序列，不接 WebApp，不是正式会计 NAV。 | 不宣传为 PMS/会计/清算能力。 |
| D2 现金流时点 | 期末 Flow；正数为投资者注资，负数为赎回。 | 不支持期初、日内或 Dietz 猜测。 |
| D3 收益口径 | 日度 TWR + 几何累计；active 为 Portfolio 减 Benchmark。 | 年化、波动、回撤、Sharpe、归因和 VaR 后续迭代。 |
| D4 缺失策略 | 全成员、全开市日、全 Benchmark 完整才成功。 | 不 inner join、不零填、不返回 partial 数值。 |
| D5 数值 | scale-12 FixedDecimal，division/multiplication 均 ties-to-even。 | 禁止 float，不调整既有 Oracle。 |
| D6 服务拓扑 | 独立 `PortfolioPerformanceService`，生产服务 17→18。 | 不把时序逻辑塞入 R8A Aggregation/Workbench。 |
| D7 发布 | 本轮只形成本地自测候选。 | 无 tag、镜像、部署、push 或远端 CI/CD。 |

## 6. 最终真实测试证据

**R8B execution base（已批准并冻结）：** public commit `a66f780c949614ab050a625667b93b129653588f`，tree `01d41c2cc2a14065bce91d555d43e9c6d0d7c1ee`；开始时 `main == origin/main` 且 tracked worktree clean。R8A 主线重取证的文档回写与本 brief 是执行前规划写入，不改变代码 execution base。

**实施允许写路径（Human 已批准建议后冻结的闭集）：**

- `docs/iterations/2026-08-r8a-portfolio-p0.md`
- `docs/iterations/2026-08-r8b-portfolio-performance.md`（本文件；实施开始后只在本节追加真实证据并更新第 7 节）
- `docs/iterations/README.md`
- `README.md`
- `docs/development.md`
- `docs/product/scope.md`
- `docs/quality/evidence.md`
- `docs/architecture/layering-refactor.md`
- `docs/architecture/data-dictionary.md`
- `Cargo.lock`
- `interface/proto/ficant/core/v1/evidence.proto`
- `interface/proto/ficant/portfolio/v1/portfolio.proto`
- `interface/ficant.pb`
- `crates/ficant-contracts/src/generated/**`
- `python/node-contracts/src/ficant_contracts/generated/**`
- `web-dm/packages/contracts-generated/src/**`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contract-tests/tests/r7b_formal_evidence.rs`
- `crates/ficant-contract-tests/tests/r8b_portfolio_performance_contract.rs`（新建）
- `python/tests/test_contract_import.py`
- `web-dm/platform-shell/tests/contracts-consumer.test.ts`
- `.github/scripts/verify-contract-generation.sh`
- `.github/scripts/verify-supply-chain.sh`
- `.github/scripts/license-inventory.lock.json`
- `.github/scripts/supply-chain.lock.json`
- `crates/ficant-domain/src/primitives/fixed_decimal.rs`
- `crates/ficant-domain/src/portfolio/mod.rs`
- `crates/ficant-domain/src/portfolio/performance.rs`（新建）
- `crates/ficant-domain/tests/r8b_portfolio_performance.rs`（新建）
- `crates/ficant-contract-tests/tests/fixtures/r7a-core-extension/core-source-sha256.tsv`（仅更新 fixed_decimal.rs 一行）
- `crates/ficant-runtime/src/native_execution.rs`
- `crates/ficant-runtime/tests/native_execution.rs`
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/ports/portfolio_performance.rs`（新建）
- `crates/ficant-application/src/use_cases/mod.rs`
- `crates/ficant-application/src/use_cases/portfolio_performance.rs`（新建）
- `crates/ficant-application/tests/r8b_portfolio_performance.rs`（新建）
- `migrations/postgresql/0027_r8b_portfolio_performance.sql`（新建）
- `crates/ficant-storage/src/postgres/mod.rs`
- `crates/ficant-storage/src/postgres/portfolio_performance.rs`（新建）
- `crates/ficant-storage/src/postgres/formal_outputs.rs`
- `crates/ficant-storage/tests/migration_acceptance.rs`
- `crates/ficant-storage/tests/r8b_portfolio_performance_postgres.rs`（新建）
- `crates/ficant-api/src/lib.rs`
- `crates/ficant-api/src/grpc_web.rs`
- `crates/ficant-api/src/formal_evidence.rs`
- `crates/ficant-api/src/portfolio_performance.rs`（新建）
- `crates/ficant-api/tests/portfolio_performance_service.rs`（新建）
- `binaries/ficant-worker/src/production.rs`
- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/composition.rs`
- `binaries/ficant-server/tests/service_topology.rs`
- `binaries/ficant-server/tests/r8b_portfolio_performance_sit.rs`（新建）
- `binaries/ficant-server/examples/r8b_portfolio_performance_bootstrap.rs`（新建）
- `scripts/bootstrap-portfolio-performance.ps1`（新建）
- `scripts/check-coverage.ps1`
- `scripts/test-coverage-check.ps1`
- `scripts/check-fast.ps1`
- `scripts/check.ps1`
- `tests/fixtures/portfolio/performance/**`（新建）
- `tests/oracle/portfolio/r8b_portfolio_performance_inputs.json`（新建）
- `tests/oracle/portfolio/r8b_portfolio_performance_expected.json`（新建）
- `tests/oracle/portfolio/r8b_portfolio_performance_decimal_oracle.py`（新建）
- `tests/oracle/portfolio/test_r8b_portfolio_performance_decimal_oracle.py`（新建）

**受保护事实：** `C:\git\ficant-portfolio/**`、COGA、ignored 本地权威文件、审计报告、R5D Rates exact-input 契约、R7B identity/recovery 算法、R8A 既有字段/tag/service 语义、既有 17 service/route、所有既有 Golden/Oracle/expected/容差、C/C++/FFI/native 数值实现、RulePack、`.github/workflows/**`、`cicd.yml`、`deploy/test/**`、远端 GitHub、版本/tag/镜像/部署均不得修改。0027 不改写 0001..0026。

本节以下只允许记录同一最终候选上的真实命令、exit code 与可得 test count；计划命令不得写成通过。

## 7. 残余风险

- R8B 的估值/NAV/Flow 是不可变研究快照，不含交易、批次成本、应计、正式会计关账或估值锁定流程。
- 首个切片只承诺单币种中国国债组合和静态 Benchmark level；多币种、动态基准和跨资产需另开迭代。
- 严格完整覆盖会在数据缺日时返回错误，优先保证结果可信，不承诺“尽量出数”。
- 本地契约包仍是未发布 `0.0.0`；不构成 WebApp 已接入或可部署产品。
- 无版本发布授权，因此本轮不运行版本候选镜像入口、不创建 tag、不部署、不触发远端 CI/CD。
