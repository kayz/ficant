# R5D 迭代 brief — 系统完整性重整

**迭代：** R5D · **承接条目：** S3 / I3 前置（不点亮新 AC） · **execution base：** `58991f5d402691104cd681add2c9d51dc2580915` · **authority base：** `3ff92bb77c29064a57c9ef371b787699519984ac`

本 brief 是 R5D 面向 Human 的唯一设计与最终证据载体。它把 2026-08 系统完整性审计中已经确认的 Rates 输入错位、L1→L2 反向依赖、KRD 同源验证和一方包 inventory 漏项合并为 R6 前的一个完整性前置轮；2026-08-12 Human 已批准 §5 的全部方向。本文当前只冻结规划，不实施代码，不修改 authority、CI/CD、远端 GitHub 设置或现有未跟踪审计报告；§6 中任何实施命令均是待执行计划，不是通过证据。

## 1. 目标

R5D 只交付一个代码结果：`RatesAnalyticsService` 的五个 RPC 都只消费 Application 层从精确、不可变引用中物化并验证的业务输入；调用方不再重复提交 Bond 条款、Calendar 内容、曲线节点、候选券事实、价格、转换因子或 Artifact 已经包含的数值，也不再提交只进入血缘、不参与计算的通用 RulePack / DataSnapshot / Funding / Tax 绑定。实际参与计算的每一项输入都以角色、身份、版本、内容哈希和适用时间进入稳定请求指纹与响应证据。

同一轮补齐四项防复发基础：把 `FixedDecimal` 所有权移至 L0 primitives 并消除 `research/exposure.rs` 对 L2 analytics 的反向引用；用 `cargo metadata` 邻接表和语法感知的 Rust 模块检查建立结构门禁；新增不导入生产 KRD 公式的 Decimal Oracle；把三个已有但遗漏于供应链 policy 的一方 crate 纳入许可证 inventory，使 19 个 Cargo package 加 Python SDK 的 20 个一方包全部受控。只同步 README、产品范围和开发检查说明中的当前事实，不复制 authority 三件套或新增状态文档。

**Acceptance sentence：**

> 给定精确 owner、`knowledge_at`、Subject、算法、单位、RPC 所需 immutable object / snapshot / artifact 引用及明确标记的场景标量，五个 Rates RPC 必须先在 Application 层读取并核验所有实际消费输入，再调用任何数值引擎；缺项、类型错误以及 owner、identity、version、hash、`knowledge_at`、valuation/as-of、visible/effective time 任一漂移都失败关闭且对应 engine 调用计数为零。Bond 条款只来自 exact Bond，交易日信息只来自 exact Calendar，曲线节点只来自 verified CurveSnapshot payload，交割篮子与日期只由 exact FuturesContract 及其 RulePack 派生，价格只来自 verified DataSnapshot/DataSource，套保数值只来自已验证 Artifact payload。成功响应以稳定角色顺序返回全部且仅限实际消费的输入绑定、场景参数摘要和 `request_fingerprint`；任一输入事实或场景参数变化都改变指纹及相应证据，相同输入产生逐位确定的响应。`FixedDecimal` 归 L0、L1→L2 反向引用门禁、独立 KRD Decimal Oracle 和 20 个一方包许可证绑定同时通过；合法且经济含义相同的既有输入保持 R5D 前数值结果，不点亮 AC09、AC30 或任何新 AC。

## 2. 验收

| 条目 | R5D 可执行判据 |
|---|---|
| 公共上下文收敛 | `AnalysisContext` 只保留 owner、`knowledge_at`、Subject、算法和单位。删除 `rule_pack`、`data_snapshot`、`funding_rule_pack`、`tax_rule_pack`，并同时 `reserved` 原字段号 `2 / 3 / 7 / 8` 与字段名；不提供 deprecated 字段、兼容 oneof、server 回填或双读 shim。`knowledge_at` 使用完整 `MarketTime`，不能以系统当前时间代替；Subject 与每个 UnitRef 都必须精确解析并进入输入证据。 |
| 输入证据与指纹 | 新增不可变 `SnapshotBinding`、`ArtifactBinding`、闭枚举 `AnalysisInputRole`、`AnalysisInputBinding` 与 `ParameterDigest`；`ResultMetadata` 增加稳定排序的 `consumed_inputs`、`parameter_digest` 和 `request_fingerprint`。请求 binding 只提交 exact identity/hash，不允许调用方重复声明对象时间；响应 binding 至少承载角色、owner、exact identity/version、content hash 及 Application 从已验证对象取得的 valuation/as-of/visible/effective time。同角色以 exact identity/version/hash 作为次级排序键。身份、版本、哈希、时间或内容任一变化必须改变指纹和对应响应证据；未消费输入不得出现。该证据是 R5D 的输入血缘，不冒充 AC30 的完整代码/镜像血缘。 |
| `AnalyzeBond` 物化 | 请求只给 exact Bond、exact Calendar、verified DataSnapshot、exact TaxRulePack、Subject、`valuation_at`、结算日、Calendar requirement 及明确的价格/YTM 场景 oneof。Application 从 Bond 取完整条款与税收属性，从 Calendar 取 sessions/coverage，从 DataSnapshot 验证价格场景的数据证据，从 TaxRulePack 与 Subject 解析税收口径；禁止内联 `BondTerms`、Calendar 内容和通用 RulePack。价格/YTM 是场景参数，进入 `ParameterDigest`，不得伪装成 snapshot 事实。 |
| `InterpolateYieldCurve` 物化 | 请求只给 verified CurveSnapshot、Subject 和查询日。Application 验证 CurveSnapshot metadata、payload、content hash、owner、as-of/visible time、其 exact Calendar / RulePack 与数据 lineage 后解码节点；禁止内联 `YieldCurveNode`。CurveSnapshot 中记录的 interpolation / curve schema 由已验证 payload 与 metadata 决定，调用方不能覆盖。 |
| `AnalyzeCarryRoll` 物化 | 请求只给 exact Bond、verified CurveSnapshot、Subject、`valuation_at`、初始和 horizon 结算日。Application 从 Bond 取条款，从 CurveSnapshot 取节点并验证其 Calendar / RulePack / 数据 lineage；禁止内联 Bond 条款、Calendar 和曲线节点。 |
| `AnalyzeFuturesDelivery` 物化 | 请求只给 exact FuturesContract、verified DataSnapshot、exact FundingRulePack、Subject、`valuation_at` 和购买日。Application 从 FuturesContract 派生交割 RulePack、产品、交割月与交割日，从已验证 RulePack 派生可交割规则和转换因子，从 verified DataSnapshot / exact DataSource 派生期货及候选券价格和候选券集合，再读取 exact Bond 条款；禁止调用方提交产品、候选券、券条款、价格、转换因子或交割日期副本。R5D 只使用既有单口径交割结果，不增加 AC09 税后 CTD。 |
| `AnalyzeFuturesHedge` 物化 | 请求只给 verified target-risk、delivery、CTD analytics Artifact、exact FuturesContract、Subject 和 `valuation_at`。Application 校验 Artifact kind/media type/owner/hash/lineage 并从 verified payload 解码目标 DV01、CTD Bond、CTD DV01、转换因子、产品与交割身份；请求不再提交这些数值或对象副本。三个 Artifact 对同一估值、合约、CTD 与上游 lineage 不一致时，engine 调用为零。 |
| 失败关闭矩阵 | 每个 RPC 均有缺失对象、错误类型、owner、id/version、content hash、`knowledge_at` 早于 visible time、valuation/as-of 不一致、有效区间不覆盖及 payload 内容漂移的负向用例；每项均在任何 bond/curve/carry/delivery/hedge engine 调用前失败，五类 engine 独立计数。不能把 storage/decoder 失败降格为 warning 或退回请求副本。 |
| 确定性与闭集 | 相同 protobuf 请求与相同已验证存储事实产生逐位相同响应 bytes、输入排序、参数摘要和指纹。每个 RPC 的成功结果都要断言消费角色闭集：缺一个、重复一个、额外一个、角色错配或排序漂移均失败。场景标量内容、单位、模式或摘要算法版本变化必须改变 `ParameterDigest` 与 `request_fingerprint`。 |
| L0 数值所有权 | `FixedDecimal` 的定义和全部算术实现移至 `ficant-domain::primitives`；`analytics` 只保留必要的 façade re-export 以减少同 crate 机械改动。`research/exposure.rs` 直接依赖 primitives，不再引用 `analytics`、`curves`、`futures_delivery` 或 `futures_hedge`；R5D 不拆成三个新 crate。 |
| 结构门禁 | `ficant-contract-tests` 新增 architecture test：用 `cargo metadata --offline --locked --no-deps --format-version 1` 验证精确 workspace crate 邻接表；用 `syn` 解析 L1 `research/**` 的 `use`、路径与 re-export，禁止引用 L2 `analytics` / `curves` / `futures_delivery` / `futures_hedge`。至少有四个隔离反例 fixture 分别证明普通 `use`、绝对路径、嵌套路径和通过 façade 的引用真实失败；合法 primitives 引用通过。该测试进入 `scripts/check-fast.ps1`，未知新 workspace package 或边必须默认失败。 |
| KRD 独立 Oracle | 新 Decimal Oracle 固定一个 Bond 仓位、一个 Futures 仓位和三个 Factor fixture，独立计算每仓位每节点 KRD、逐节点 totals 与组合 totals；同时验证全节点平移所得 DV01、数量线性和反向仓位符号。Python Oracle 不 import Rust、生产 KRD、现有 R4d helper 或生产公式，先从同一输入重算并核验新 expected；独立 Rust integration test 再让生产 KRD 消费同一输入并逐项匹配该 expected。两侧不得共享计算 helper，不得为通过而改变既有容差、expected 或 R4d-a/R4d-b fixture。 |
| 一方包许可证闭合 | 将 `ficant-cgb-futures-pack`、`ficant-funding-pack`、`ficant-tax-pack` 加入 `.github/scripts/supply-chain.lock.json` 的 exact first-party policy，将机械常量从 17 收紧为 20，并用现有工具刷新 `.github/scripts/license-inventory.lock.json` 的 bindings。`cargo metadata` 必须得到 19 个本地 Cargo package；与 Python `ficant-sdk` 合并后恰为 20 个唯一 exact purl/source，三者缺一、重复或 source-integrity 漂移均使 `verify-bindings --require-first-party` 失败。不得改变第三方许可证裁决、例外或漏洞接受。 |
| 文档事实同步 | 根 README、`docs/product/scope.md` 与 `docs/development.md` 只同步 R5D 后的 Rates 输入权威、结构门禁、20 个一方包和实际 v0.1/v0.2 边界；不复制 SPEC / ACCEPTANCE / MANUAL，不新增状态、审计或 checklist 文档，不宣传 AC09、AC30、DMQuant、AI、Policy/Constraint 或完整 DataHealth 扩展已实现。 |
| 回归 | 现有 R4d-a / R4d-b KRD、R5a 来源、R5b coverage、R5c health、Rates API、生产 server 与 Python/TypeScript/Rust contract consumers 全部转绿；现有 AC06–AC08、AC10、AC15、AC16、AC26–AC29、AC35、AC36 的已批准数值与失败关闭不回退。AC09 由 R5E 重新取证，R5D 不用其旧结果作完成证据。 |

R5D 冻结五个 RED-first 子循环，RED 只证明判据有效，不是 checkpoint：

1. **contract RED：** 先收紧 descriptor 判据，证明旧 `AnalysisContext`、五个旧 request shape 和旧 `ResultMetadata` 真实失败；字段号/name reserve、生成确定性和三类 consumer 全绿后形成 contract checkpoint。
2. **application / transport RED：** 先写 `r5d_rates_materialization` 与 Rates API 负向矩阵，证明当前 adapter 会接受重复事实或在未读取 exact input 时调用 engine；五个 materializer、response evidence、完整调用计数和生产 SIT 全绿后形成 materialization checkpoint。
3. **architecture RED：** 先让真实 `research/exposure.rs → analytics::FixedDecimal` 被新门禁报告，并分别运行四个负向 fixture；下沉所有权、crate 邻接闭集和 `check-fast.ps1` 接入全绿后形成 architecture checkpoint。
4. **Oracle RED：** 先以独立 Decimal fixture 对当前候选建立逐仓位/逐节点/totals 比较，并证明至少一个受控扰动会被抓住；精确 KRD、平移 DV01、线性与符号判据全绿后形成 Oracle checkpoint。
5. **supply-chain RED：** 先运行 `verify-bindings --require-first-party` 证明三项 Cargo package 缺失于 17 项 policy；只增加这三项并刷新绑定，使 20 项闭集和负向测试转绿后形成 supply-chain checkpoint。

## 3. 非目标

- AC09 的 Human 批准 TaxRulePack 内容、双税收口径、独立税后 YTM Oracle、市场/主体双 CTD、反转篮子或无税差对照；它们独立进入 R5E。
- AC37 的平台管理员/研究用户分离、数据源白名单和基础数据变更留痕；Definition、Fact、Snapshot、Artifact 四个尚未组合的服务也不在 R5D 补齐。
- 删除 `ficant-web`、清理 dead gRPC-Web 路由或重做 Web/UI；这些拓扑孤儿进入 R6B。
- Python node runtime、DMQuant、Policy / Constraint、完整 DataHealth 扩展、AI / GeneratedNode 沙箱；全部继续顺延 v0.2。
- 三个 domain crate 的完整物理拆分、L2 全部模块重构、数值公式变更、C/C++ ABI 变化或跨 clang / 跨编译器裁决。
- authority 三件套及其私有仓库、AC 点亮、MANUAL、公共根目录本地 authority 副本、现有未跟踪 `docs/review/full-audit-2026-08-07.md`。
- `.github/workflows/**`、GitHub rulesets / CODEOWNERS / approvals / status checks、Dependabot、secret scanning、push protection、Release 对齐、签名策略、版本 tag、镜像、部署、远端 CI/CD 或 branch 清理。
- AC30 的完整代码/镜像/输入血缘，或任何仍待 Human 批准的验收条目。R5D 的 `consumed_inputs` 只完成 Rates 的输入证据前置。

## 4. 公共契约变化

R5D 对 `ficant.rates.v1` 做 v0.1 前获准的破坏性收敛。服务名和五个 RPC 名不变，不新增 v2 service；所有被删除字段同时 reserve number 与 name，不提供兼容 shim。固定 Buf 1.56.0 在两个独立临时树完整生成后，只机械同步 Rust、Python、TypeScript consumer。

通用类型冻结为：

| 类型 | 冻结语义 |
|---|---|
| `ObjectBinding` | exact `VersionRef + content_hash`，只用于有 VersionRef 的 immutable definition / rule pack；不得承载 snapshot 或 artifact。 |
| `SnapshotBinding` | exact snapshot ULID + content hash，只用于 DataSnapshot / CurveSnapshot；`as_of` / `visible_at` 必须由 Application 从 metadata 读取，调用方不能重复提交。 |
| `ArtifactBinding` | exact artifact ULID + content hash；不以伪造 version 把 Artifact 塞进 `ObjectBinding`，kind/media type/owner/lineage 由 Application 从 metadata 与 verified payload 校验。 |
| `AnalysisInputRole` | 闭枚举，至少覆盖 Subject、Unit、Bond、Calendar、CurveSnapshot、DataSnapshot、DataSource、TaxRulePack、FundingRulePack、DeliveryRulePack、FuturesContract、TargetRiskArtifact、DeliveryArtifact、CtdAnalyticsArtifact；`UNSPECIFIED` 不能出现在成功响应。 |
| `AnalysisInputBinding` | `role + owner + oneof(ObjectBinding, SnapshotBinding, ArtifactBinding) + observed/effective evidence`；exact 内容和时间由 Application 从已验证对象构造，transport 不能从请求直接回显。 |
| `ParameterDigest` | 完整 `AlgorithmBinding + canonical parameter bytes hash`；承载 `knowledge_at` 及场景 YTM/price、结算/查询/horizon/购买日等非注册参数，不把它们冒充 snapshot lineage。 |
| `ResultMetadata` | 保留 schema/engine/algorithm/Subject 身份，删除专用 Funding/Tax echo；新增全部且仅限实际消费的 `consumed_inputs`、`parameter_digest` 与 `request_fingerprint`。 |

`AnalysisContext` 的新字段形状冻结为 owner `1`、algorithm `4`、units `5`、subject `6`、`knowledge_at` `9`；原 `rule_pack=2`、`data_snapshot=3`、`funding_rule_pack=7`、`tax_rule_pack=8` 及其名字全部 reserved。

| RPC | 唯一权威输入 | 删除并 reserve 的重复输入 |
|---|---|---|
| `AnalyzeBond` | exact Bond、exact Calendar、verified DataSnapshot、exact TaxRulePack、Subject，以及明确标记的价格/YTM 场景参数 | 内联 `BondTerms`、内联 Calendar 内容、通用 RulePack；旧 Calendar binding 不能继续承载 sessions |
| `InterpolateYieldCurve` | verified CurveSnapshot 及其 Calendar、RulePack、数据 lineage，外加 Subject | 内联 `YieldCurveBinding` / 曲线节点 |
| `AnalyzeCarryRoll` | exact Bond、verified CurveSnapshot 及其 Calendar / RulePack / 数据 lineage、Subject | 内联 Bond 条款、Calendar、曲线节点和通用绑定 |
| `AnalyzeFuturesDelivery` | exact FuturesContract；由合约派生交割 RulePack、产品与交割日；verified DataSnapshot / DataSource 派生期货及券篮价格；exact FundingRulePack、Subject | 调用方产品、候选券、券条款、价格、转换因子、交割月与交割日期副本 |
| `AnalyzeFuturesHedge` | verified risk、delivery、CTD Artifact，exact FuturesContract、Subject；数值从已验证 payload 物化 | CTD Bond、产品、目标 DV01、CTD DV01、转换因子以及通用 RulePack / DataSnapshot 副本 |

Bond 与 Calendar 是条款和交易日事实的唯一权威：RulePack 可决定惯例与规则内容，但不能再夹带第二份 Bond 或 Calendar 事实；CurveSnapshot、DataSnapshot 和 Artifact 必须经 metadata + verified payload 两段核验，不能只凭请求中的 hash 构造“已验证”输入。所有 owner/version/hash/knowledge/valuation/visible/effective-time 漂移在数值引擎前失败关闭。

## 5. 需 Human 决策

以下事项已于 2026-08-12 冻结，本轮没有未决 Human 选择：

- **已裁决——路线位置：** 在 R6 前插入 R5D；它只修 S3 / I3 前置，不点亮新 AC。AC09 独立进入 R5E，不能用 R5D 顺带宣布完成。
- **已裁决——破坏性契约：** v0.1 前允许删除 Rates 重复字段并 reserve 旧 tag/name，不提供兼容 shim、双读期或 v2 service。
- **已裁决——事实权威：** Bond 是债券条款唯一权威，Calendar 是交易日事实唯一权威；RulePack 只提供规则内容，Snapshot / Artifact 只提供已验证事实或结果。
- **已裁决——完整性基础包：** `FixedDecimal` 下沉、L1→L2 结构门禁、独立 Decimal KRD Oracle、20 个一方包许可证闭合和必要事实文档与 Rates 物化同轮交付；R5D 不做三 crate 大拆分。
- **已裁决——远端治理：** GitHub 权限、安全、PR 检查、Release 与签名策略作为独立治理轮，由 Human/CICD 决定，不进入 OPAID 产品迭代。

若实施需要改变上述公共行为、增加非点名业务能力、修改既有 Oracle expected / 容差 / 断言方向、引入新的税制内容、修改 authority、CI/CD、部署或远端设置，必须在首次写入前停止并取得 Human 明确批准。§6 允许写路径之外的路径同样必须先扩权；不得先改后补或用最终结果追认。

## 6. 最终真实测试证据

**规划落盘边界（本次）：** 2026-08-12 在 `C:\git\ficant` 核验 `HEAD == origin/main == 58991f5d402691104cd681add2c9d51dc2580915`；本次只允许新建本文并修改 `docs/iterations/README.md`、`docs/architecture/layering-refactor.md`。`docs/review/full-audit-2026-08-07.md` 保持未跟踪且不改；本地被忽略的 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md` 不作为权威读取，不修改或删除。本段只记录规划文件边界，不是 R5D 代码实施证据。

**R5D 实施允许写路径（精确文件或精确目录闭集；实施开始后本清单不得改写）：**

- `Cargo.toml`、`Cargo.lock`
- `interface/proto/ficant/rates/v1/analytics.proto`
- `crates/ficant-contracts/src/generated/ficant.rates.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.rates.v1.tonic.rs`
- `python/node-contracts/src/ficant_contracts/generated/ficant/rates/v1/analytics_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/rates/v1/analytics_pb2_grpc.py`
- `web-dm/packages/contracts-generated/src/ficant/rates/v1/analytics_pb.ts`
- `crates/ficant-contract-tests/Cargo.toml`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contract-tests/tests/r5d_layer_dependencies.rs`（新建）
- `crates/ficant-contract-tests/tests/fixtures/r5d-layering/**`（新建，仅反例 Rust fixture）
- `crates/ficant-domain/src/lib.rs`
- `crates/ficant-domain/src/primitives/mod.rs`
- `crates/ficant-domain/src/primitives/fixed_decimal.rs`（新建）
- `crates/ficant-domain/src/analytics.rs`
- `crates/ficant-domain/src/curves.rs`
- `crates/ficant-domain/src/futures_delivery.rs`
- `crates/ficant-domain/src/futures_hedge.rs`
- `crates/ficant-domain/src/research/exposure.rs`
- `crates/ficant-domain/tests/bond_analytics_contracts.rs`
- `crates/ficant-domain/tests/yield_curve_contracts.rs`
- `crates/ficant-domain/tests/futures_delivery_contracts.rs`
- `crates/ficant-domain/tests/futures_hedge_contracts.rs`
- `crates/ficant-domain/tests/r4d_a_bond_krd_contracts.rs`
- `crates/ficant-domain/tests/r4d_b_futures_krd_contracts.rs`
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/ports/analytics.rs`
- `crates/ficant-application/src/ports/artifacts.rs`
- `crates/ficant-application/src/ports/curves.rs`
- `crates/ficant-application/src/ports/futures_delivery.rs`
- `crates/ficant-application/src/ports/futures_hedge.rs`
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/use_cases/bond_analytics.rs`
- `crates/ficant-application/src/use_cases/carry_roll.rs`
- `crates/ficant-application/src/use_cases/futures_delivery.rs`
- `crates/ficant-application/src/use_cases/futures_hedge.rs`
- `crates/ficant-application/src/use_cases/rates_materialization.rs`（新建）
- `crates/ficant-application/src/use_cases/mod.rs`
- `crates/ficant-application/tests/r5d_rates_materialization.rs`（新建）
- `crates/ficant-application/tests/r5d_portfolio_krd_oracle.rs`（新建；只消费共享 fixture/expected，不实现 Oracle 公式）
- `crates/ficant-application/tests/r4d_a_bond_krd_contracts.rs`
- `crates/ficant-application/tests/r4d_b_futures_krd_contracts.rs`
- `crates/ficant-api/Cargo.toml`
- `crates/ficant-api/src/rates.rs`
- `crates/ficant-api/tests/rates_service.rs`
- `binaries/ficant-server/Cargo.toml`
- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/rates_sit.rs`（新建）
- `crates/ficant-native-nodes/src/lib.rs`
- `crates/ficant-native-nodes/tests/cgb_bond_analytics.rs`
- `binaries/ficant-worker/tests/phase4_worker_sit.rs`
- `crates/ficant-storage/src/analytics_arrow.rs`
- `crates/ficant-storage/src/carry_arrow.rs`
- `crates/ficant-storage/src/futures_arrow.rs`
- `crates/ficant-storage/src/hedge_arrow.rs`
- `crates/ficant-storage/tests/bond_analytics_arrow.rs`
- `crates/ficant-storage/tests/carry_roll_arrow.rs`
- `crates/ficant-storage/tests/futures_delivery_arrow.rs`
- `crates/ficant-storage/tests/futures_hedge_arrow.rs`
- `python/tests/test_contract_import.py`
- `python/tests/test_rates_sdk_live.py`
- `web-dm/platform-shell/tests/contracts-consumer.test.ts`
- `web-dm/platform-shell/src/phase5a-observer.tsx`
- `web-dm/platform-shell/tests/phase5a-observer.test.tsx`
- `tests/golden-cases/china-rates/r5d-portfolio-krd-oracle-inputs.json`（新建）
- `tests/golden-cases/china-rates/expected/r5d-portfolio-krd-oracle-expected.json`（新建；只由独立 Oracle 首次冻结，不可从生产输出生成）
- `tests/oracle/china-rates/r5d_portfolio_krd_decimal_oracle.py`（新建）
- `tests/oracle/china-rates/test_r5d_portfolio_krd_decimal_oracle.py`（新建）
- `scripts/check-fast.ps1`、`scripts/check.ps1`
- `.github/scripts/supply-chain.lock.json`
- `.github/scripts/license-inventory.lock.json`
- `.github/scripts/verify-supply-chain.sh`
- `.github/scripts/tests/test_license_inventory_bindings.py`
- `README.md`
- `docs/product/scope.md`
- `docs/development.md`
- `docs/iterations/2026-08-r5d-system-integrity.md`（仅填入 §6 最终真实证据与 §7 最终残余风险；不得改写 §1–§5 或 §6 冻结边界）

**禁止写路径：** 所有未逐项列出的路径。特别禁止本文 §1–§5 与 §6 冻结边界、迭代索引和路线图在实施期被用来追认范围；authority 三件套和公共根目录同名副本；所有 ADR；`interface/buf.gen.yaml` 与除五个点名文件外的 generated output；migration；canonical quote schema/hash；DataHealth、Coverage、Position、Factor、Definition/Fact/Snapshot/Artifact service 契约；C/C++、sys/native 数值实现；`domain-packs/**` payload；既有 Phase 2B/2C/2D matrix；`.github/workflows/**`、`cicd.yml`、`deploy/**`、版本与远端设置。若固定生成器确实要求一个未点名 consumer 文件变化，必须先停机并由 Human 在 §5 新增精确扩权，不能直接同步整个 generated tree。

**受保护事实：** 既有 `tests/golden-cases/**` 除两个 R5D 新文件外逐 blob 不变；既有 `tests/oracle/**` 除两个 R5D 新文件外逐 blob 不变；`tests/phase2c/acceptance-matrix.json`、`tests/phase2d/acceptance-matrix.json`、`cpp/**`、`crates/ficant-kernel-sys/**`、`crates/ficant-fixed-income-native/**`、`domain-packs/**`、`migrations/**`、`docs/architecture/adr/**`、`scripts/layering-allowlist.json`、canonical quote v1/schema/hash、现有容差和所有已通过断言方向均保持 execution base 事实。R5D 允许更新供应链一方包 policy 和绑定，但不得改变第三方许可证、例外、工具版本、漏洞结果或风险接受。

**待执行的 RED-first 证据：** 实施者必须按 §2 五个子循环记录每次首个真实非零命令、exit code 与首个错误；RED 不作为 checkpoint。每个 checkpoint 只在对应直接测试全绿后形成，不能回退既有 checkpoint，也不能修改 expected、Oracle、容差或门禁方向制造通过。

**最终针对性命令（全部为计划；截至本 brief 落盘尚未执行，结果栏必须在同一最终代码候选上填真实 exit code 和可得 test count）：**

- 固定 Buf 1.56.0：`buf format --diff --exit-code interface`、`buf lint interface`
- 按 `interface/README.md` 在两个独立临时树完整运行 `buf generate`，比较两棵树及仓库 Rust / Python / TypeScript 生成输出的文件集合与规范化 SHA-256
- `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory`
- `cargo test --offline --locked -p ficant-contract-tests --test r5d_layer_dependencies`
- `uv run --offline --locked --project python python -m pytest python/tests/test_contract_import.py -q`
- `corepack pnpm@10.12.4 --filter @ficant/platform-shell exec vitest run tests/contracts-consumer.test.ts`
- `cargo test --offline --locked -p ficant-application --test r5d_rates_materialization`
- `cargo test --offline --locked -p ficant-api --test rates_service`
- `cargo test --offline --locked -p ficant-server --test rates_sit`
- `cargo test --offline --locked -p ficant-application --test r4d_a_bond_krd_contracts`
- `cargo test --offline --locked -p ficant-application --test r4d_b_futures_krd_contracts`
- `cargo test --offline --locked -p ficant-application --test r5d_portfolio_krd_oracle`
- `uv run --offline --locked --project python python -m pytest tests/oracle/china-rates/test_r5d_portfolio_krd_decimal_oracle.py -q`
- `python .github/scripts/verify-license-inventory.py verify-bindings --inventory .github/scripts/license-inventory.lock.json --cargo-lock Cargo.lock --uv-lock python/uv.lock --pnpm-lock web-dm/pnpm-lock.yaml --supply-lock .github/scripts/supply-chain.lock.json --release-root . --require-first-party`
- `python .github/scripts/tests/test_license_inventory_bindings.py`，必须包含 20 项闭集、三项缺失、重复 purl 与 source-integrity 漂移负例
- `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`
- `pwsh -NoProfile -NonInteractive -File scripts/check.ps1`
- 静默导入既有六个 Windows User 级 `FICANT_TEST_*` 变量且不输出值后，`pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration`
- `git diff --check`
- 从 execution base 到最终候选核对实际 tracked + untracked changed-path 集合完全包含于本节允许路径，并复核全部受保护事实

**最终真实结果（2026-08-12）：** R5D 的技术 acceptance sentence 已在同一最终代码候选上实现并通过本地完整门禁；本轮仍只承接 S3 / I3 前置，不点亮 AC09、AC30 或任何新 AC。

**候选身份与范围：**

- execution base 与最终工作树基点均为 `HEAD == origin/main == 58991f5d402691104cd681add2c9d51dc2580915`，authority base 仍为 `3ff92bb77c29064a57c9ef371b787699519984ac`。未提交、未推送，未修改 authority、远端、CI/CD、部署或版本。
- 排除本 brief 与预存未跟踪审计报告后，实际代码、测试、生成物、派生绑定和已批准规划文档共 `59` 个 changed paths；按 `path<TAB>file_sha256<LF>` 的 UTF-8、路径排序清单计算，候选 manifest SHA-256 为 `6c907d85fcb2bda149287707f7c0ebb0a16d829458e96c07e2d1fd3a1320b4b1`。计入本 brief 后，R5D/规划候选共 `60` 个 changed paths。
- `docs/iterations/README.md` 与 `docs/architecture/layering-refactor.md` 是实施前已批准的规划落盘；删除 legacy constructor 后，Human 又明确授权只迁移 `binaries/ficant-worker/src/tests.rs` 与 `crates/ficant-api/tests/phase2e_sdk_live.rs` 两个历史夹具。把这四项授权与本节冻结 allowlist 合并核对，路径违规为 `0`。
- `docs/review/full-audit-2026-08-07.md` 仍为候选外的未跟踪文件，最终 SHA-256 `a5514e5d3d7c633a81c363de217731503493d8278eeac0f1810a1ff9445da71f`，未修改；被忽略的 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md` 仍未跟踪且未修改或删除。

**实现结果：**

- 五个 Rates RPC 已删除重复业务事实入口，改为由 Application 从 exact Bond、Calendar、FuturesContract、RulePack、verified Snapshot/DataSource 与 verified Artifact 两段物化；Subject、Unit、Algorithm、场景参数和全部实际消费输入进入稳定证据与指纹。生产 server 只组合 R5D materializer；私有 Bond native node 使用 public request + private materialized input 双端口，且 schema、完整 proof、税率标量与指纹均在 engine 前校验。
- 所有 owner、identity/version/hash、knowledge/valuation/as-of/visible/effective time、payload、Curve 数据血缘、FactorDefinition、Delivery RulePack 和纳秒级时间漂移均在数值 handoff 前失败关闭；成功证据按闭集稳定排序。历史 Worker、Rust native node、Python live SDK、TypeScript consumer 和 Phase 2E live fixture 已迁移到同一 exact-ref 契约。
- `FixedDecimal` 与 scale 所有权已下沉至 L0 primitives，analytics 保留 façade re-export；L1 research 不再反向依赖 L2。`cargo metadata` 精确 19-crate 邻接表与 `syn` 语法门禁已进入 `check-fast.ps1`，五个隔离 fixture 覆盖四种违规路径及合法 primitives 路径。
- 独立 Decimal Oracle 固定 1 Bond + 1 Futures + 3 Factors，独立见证逐仓位、逐节点、totals、平移 DV01、数量线性和反向仓位符号；Rust 只消费同一 fixture/expected 与生产端比较，不导入 Oracle 公式。既有 R4d-a/R4d-b expected、容差与断言方向未改。
- Delivery Arrow schema 补齐 `market_timezone` 与 `valuation_local_date` 后与 42 列声明一致；Bond/Delivery codec 对完整 facts、跨行共享事实和篡改失败关闭。该补强没有改变数值公式或受保护 canonical quote。
- 供应链 policy 现在恰含 19 个 Cargo workspace package + Python `ficant-sdk` 的 20 个唯一一方 purl；新增三个 pack 的缺失、重复 purl 和 source-integrity 漂移均有负例。最终 license inventory digest 为 `be0203d397f29e5d6eb21c90b30868e46d3f0a16139266d13728f1cb952c2f23`；第三方许可证、例外、工具版本、漏洞快照与风险接受未变。

**RED-first 留痕裁决：**

- application/materialization 留存的首个生产语义 RED 为 `cargo test -p ficant-application --test r5d_rates_materialization --locked --no-fail-fast`，exit `1`，`5 passed / 2 failed`；首个失败 `hedge_rejects_future_valuation_and_drifted_delivery_rule_authority` 报告漂移输入到达 numerical handoff，另一失败证明小于 1 微秒的 knowledge 漂移产生相同参数 hash。扩充 Curve 权威检查后曾为 `5 passed / 3 failed`，新增虚构 lineage 到达 handoff；最终同套件扩展为 10 项并全绿。
- transport 留存的业务 RED 为 `cargo test --offline --locked -p ficant-api --test rates_service`，exit `1`，`4 passed / 1 failed`；`all_five_rpcs_return_stable_complete_consumed_input_evidence` 首先因 Curve `LineageIncomplete` 失败，暴露夹具仍使用占位 DataSource hash，随后改为 exact canonical hash。
- contract、独立 architecture 语义、Oracle 以及最初“17 项 policy 缺三个 pack”的首轮非零输出没有完整留存，不能事后伪造为 checkpoint。实施期间另有两次可核实但不冒充上述原始 RED 的门禁触发：裸 descriptor test 因 Linux-only 默认 Buf 路径 exit `1`，修为 PATH 上解析且继续严格校验 1.56.0；最终源码变化后 `verify-bindings` 以 `pkg:cargo/ficant-contract-tests@0.1.0` binding mismatch exit `2`，随后仅刷新派生绑定。结构 fixture、KRD 扰动性质与 11 个供应链正负测试在最终候选上证明门禁方向，但首轮 RED 审计链缺口保留到 §7。

**最终聚焦证据（同一候选，均 exit `0`）：**

- Buf `1.56.0` 的 format/lint 通过；两个独立完整 `buf generate` 树各 `73` 文件，逐路径 SHA-256 mismatch `0`。仓库生成物与临时树一致：Rust `10/10`、Python 当前模板 `36/36`、TypeScript `27/27`。
- `descriptor_inventory`：`20 passed`；`r5d_layer_dependencies`：`3 passed`；Python contract import：`1 passed`；Node `22.17.0` + pnpm `10.12.4` TypeScript contract consumer：`1 passed`。
- `r5d_rates_materialization`：`10 passed`；Rates API：`5 passed`；生产 server SIT：`1 passed`。所有漂移用例均通过 engine/repository handoff 计数证明失败关闭。
- R4d-a：`4 passed`；R4d-b：`6 passed`；Rust R5D KRD 对照：`7 passed`；Python Decimal Oracle：`3 passed`。
- `verify-bindings --require-first-party` 返回上述 inventory digest；许可证绑定正负套件 `11 passed`。

**最终全量门禁（同一候选）：**

- `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`：exit `0`，`58.6 s`。
- prepend 固定 Node `22.17.0` 后，`pwsh -NoProfile -NonInteractive -File scripts/check.ps1`：exit `0`，`211.5 s`；包含 strict Clippy、workspace build/test、C++/Oracle、Python live SDK、许可证和 Web `35/35`。
- 静默导入既有六个 User 级 `FICANT_TEST_*` 变量后，`pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration`：exit `0`，`322.1 s`；数据库迁移 `4/4`、lease `1/1`、执行闭环 `3/3`、Worker `1/1`、业务闭环 `1/1`、negative invariants `13/13`，以及 Phase 2B/2C/2D、Phase 3A/3B SIT 全绿。
- `git diff --check`：exit `0`。从 execution base 复核 `tests/phase2c/acceptance-matrix.json`、`tests/phase2d/acceptance-matrix.json`、`cpp/**`、`crates/ficant-kernel-sys/**`、`crates/ficant-fixed-income-native/**`、`domain-packs/**`、`migrations/**`、`docs/architecture/adr/**`、`scripts/layering-allowlist.json` 与 canonical quote 相关路径，差异均为 `0`。

本轮无 Human 选择的版本号，因此没有运行 `scripts/check-release-candidate.ps1`，没有创建 tag、镜像或部署，也没有触发或修改远端 CI/CD。

## 7. 残余风险

- Rates contract 是有意的破坏性收敛；仓库内登记 consumer 已迁移并通过，但仓库外未登记 consumer 会被直接破坏。v0.1 前不提供 shim 是 Human 已接受的迁移风险。
- contract、architecture、Oracle 与最初 supply-chain 循环的“首个语义 RED”原始输出没有完整保留；最终负向门禁和完整候选是可复现的，但这四项历史时序证据不能补造。该缺口是过程审计风险，不是已知代码失败。
- 当前模板生成的 Python `36` 个文件与仓库逐哈希一致，但仓库另跟踪一个 R3B 于 `2026-07-30` 引入、仅含无服务 stub 的 `ficant/market/v1/definition_pb2_grpc.py`；R5D 未修改它。它是历史生成物孤儿候选，删除或重构需独立授权。
- R5D 的 `consumed_inputs` 只覆盖 Rates 实际输入，不包含源码、构建镜像、运行镜像和恢复链，因此不构成 AC30–AC33 的完整血缘或灾备证据。
- KRD Oracle 只独立见证固定 Bond+Futures / 三 Factor 的数值与代数性质；它不见证税后 YTM、双 CTD、曲线全口径或跨编译器一致性。AC09 由 R5E 承担，跨 clang 裁决由 R7A 承担。
- 语法感知门禁能抓住 Rust 源码路径引用和 workspace crate 边，不等同于完成三 crate 的物理隔离；宏展开、build script 或运行时动态耦合仍需后续审计。完整拆分只在 R7A 的 AC04 实证要求时决定。
- 20 个一方包 inventory 闭合只修当前供应链分类遗漏，不代表完成远端 secret scanning、push protection、签名、镜像或发布治理。
- Definition、Fact、Snapshot、Artifact 服务仍存在“声明但未生产组合”的拓扑债务，dead gRPC-Web 与 `ficant-web` 逻辑孤儿仍保留到 R6A/R6B；R5D 不能宣传服务拓扑已经闭合。
- pnpm 在 contract/Web 门禁中仍提示 `pnpm.onlyBuiltDependencies` 位于 `web-dm/package.json`、不会在 workspace root 生效；当前测试不受影响，但配置归位属于后续开发工具清理，不在 R5D 允许路径。
- 远端 GitHub 安全与分支治理保持原状；独立治理轮何时执行仍由 Human/CICD 决定，不影响 R5D 本地候选的技术完成，但继续构成仓库治理风险。
- 未跟踪审计报告不进入 R5D 候选，也不是第二份 brief；若以后要纳入版本控制，必须由独立任务明确授权和审查。
