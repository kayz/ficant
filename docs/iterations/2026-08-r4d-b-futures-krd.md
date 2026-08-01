# R4d-b 迭代 brief — 具体期货 KRD 与全组合聚合

**迭代：** R4d-b · **承接条目：** AC16 · **execution base：** `cc19182ee1a5d857f5da1c25d29c27b0f3a9de7e` · **authority base：** `0fd4073f8513317f99c220c5d1a98c5ee8d79b51`

本 brief 是 R4d-b 面向 Human 的唯一设计。R4d-a 已在公共提交 `cc19182ee1a5d857f5da1c25d29c27b0f3a9de7e` 建成 verified PositionSnapshot / CurveSnapshot、完整 registered Bond、Factor convention 驱动的逐债券 KRD 与债券子组合 totals，但明确没有期货 KRD，authority 也未点亮 AC16。R4d-b 只补齐 exact FuturesContract 的 base CTD / conversion factor、固定 CTD 冲击重定价、逐期货仓位 KRD 与 Bond + Futures 全组合机械聚合；不改 R4a 的交割 RPC 或 R4d-a 的债券算法。本文冻结设计、测试和逐文件写路径；Human 已于 2026-08-01 批准 §5 五项新增语义，设计自此冻结。该批准不自动授权开始实现、commit、push、Pull Request、merge、authority binding 或发布。

## 1. 目标

扩展既有 `PortfolioRiskService.CalculateKeyRateDv01`，使同一请求能够从一个精确 PositionSnapshot、一个 verified CurveSnapshot 和（仅在含期货仓位时）一个 verified DataSnapshot 内部生成完整的 Bond + Futures 逐仓位 KRD 与全组合 totals。

服务端先完成全仓位类型、owner、version、Unit、Factor topology 与时点预检。Bond 沿用 R4d-a。每个 Futures 仓位必须解析为注册的 exact `FuturesContract`，由其注册 `product_code`、RulePack ref、结算时间与报价 Unit 选择精确 L3 product rule；合约规模只从该 RulePack 的 `contract_size_in_quote_units` 解析。服务再结合 verified DataSnapshot 的同日双边报价和 R4a 的候选券边界，调用既有 RulePack parser 与 delivery engine 一次取得 base CTD 和 conversion factor。冲击阶段固定该 CTD 与 conversion factor，只按 R4d-a 的同一完整 curve axis / convention 重定价 CTD Bond；不得在 up/down shock 内重选 CTD。

调用方不能提交 product、交割日期、候选券、价格、CTD、conversion factor、Bond terms、DV01、KRD、权重或 totals。任何纳入敞口的仓位不是完整 registered Bond 或 risk-ready exact FuturesContract，或任何必需事实缺失 / 漂移，整次请求在返回 partial exposure 前失败关闭。

**Acceptance sentence：**

> 注册至少一只持仓 Bond、一个有符号整数手数的具体 FuturesContract、该期货的完整可交割 Bond 集合、三项稳定 CurveNode / FactorDefinition 及 Bond / Futures 双向 binding；发布 `as_of == valuation_at`、`visible_at <= knowledge_at` 的 verified CurveSnapshot、同一估值时点的 verified DataSnapshot 与同 owner / Subject 的精确 PositionSnapshot。一次 `CalculateKeyRateDv01` 仅凭这些 ID 与 UnitRef，返回稳定排序的每个 Bond / Futures 仓位三因子 KRD 和三项全组合 totals；每个 total 与响应中同 FactorId 的全部逐仓位 exact Decimal / UnitRef 之和完全相等。期货 KRD 等于 base CTD 在同一 Factor shock 下的内部重定价差，经 frozen conversion factor、精确 RulePack 的合约规模和有符号手数缩放；TS 的 20000 个百元报价单位与 TF/T/TL 的 10000 个单位形成可手算的 2:1 对照。构造 shock 后本应切换 CTD 的对照时，结果仍固定使用 base CTD。删除或漂移任一 snapshot、quote、FuturesContract risk selector、RulePack item、CTD Bond pricing term、Factor / binding、Unit、owner / version / hash / 时点，提供单边报价、非整数手数、连续 / 拼接 / 无 subtype 合约，或加入任一不支持仓位，均在返回任何 partial exposure 前失败关闭。既有 Rates / R4a / R4d-a 结果、C++ / C ABI、Golden、Oracle、canonical Arrow schema/hash、Phase 2C/2D matrix、v1 cgb RulePack 与 allowlist 均不变。

## 2. 验收

| 条目 | R4d-b 可执行判据 |
|---|---|
| AC16 | 一个 PositionSnapshot 同时含至少一个 Bond 和一个 exact FuturesContract，完整返回 position × factor 向量与全组合 totals。测试必须在响应外按同 FactorId / hash / UnitRef 对全部 Bond + Futures 仓位机械求和并 exact 相等；少任一仓位或 Factor、改变返回顺序、返回 bond-only subtotal 均失败。 |
| 期货 KRD | 对至少三个 Factor 用 R4d-a direction 公式重定价固定 base CTD；按 §4 公式缩放到有符号合约仓位。测试须含 long / short 符号、非零相邻期限、exact zero 轴、TS / T 合约规模 2:1、conversion factor 对照，以及“若重选会切 CTD、实际仍不切”的反证。 |
| 服务端 CTD | product、purchase / delivery dates、RulePack、候选券、双边 midpoint 价格、base CTD 与 conversion factor 全部由 exact FuturesContract、verified DataSnapshot 和注册定义派生。请求 wire 中不存在这些可伪造数值；delivery engine 只在 base materialization 调用，shock 重定价期间调用数为 0。 |
| 全量失败关闭 | 先预检全部 included positions。普通 Instrument、无 subtype、连续 / 拼接序列、legacy FuturesContract、缺 quote / RulePack / pricing / topology / Unit 或跨 owner/version/hash/time 的任一项，使 curve、bond 和 delivery 数值引擎调用数均为 0；base materialization 后才可发现的 CTD/RulePack 错误仍须保证 shock bond engine 为 0、且不返回 partial exposure。 |

R4d-b 闸门：

1. RED-first 分三次取得：domain / protobuf 公式与完整聚合 contract；application 的 server-derived CTD / fixed-CTD repricing contract；transport / production composition contract。每次先只加判据并取得非零 exit code；RED 不是 checkpoint。domain、application、storage、transport 分别在对应直接测试通过后才能成为 forward-only checkpoint。
2. `FuturesContract` 字段 1–6 保持原号；只新增 §4 的 7 / 8。legacy definition 仍可读取并继续服务既有 Rates 路径，但缺任一新增字段时不能进入 R4d-b。不得从 `Instrument.symbol`、市场分支、正则或调用方 enum 猜 product。
3. 含 Futures 时 `futures_data_snapshot_id` 必填；纯 Bond 请求必须不填，填了未消费的 snapshot 也失败。DataSnapshot 必须 `as_of == valuation_at` 完整 `MarketTime` 精确相等、`visible_at <= knowledge_at` 且 `visible_at >= as_of`，verified Parquet / Manifest、owner / hash / size / canonical decode 全部沿用 R4a 失败关闭边界。
4. 每个 CTD 输入只接受同一 DataSnapshot 中、与 FuturesContract 注册 `price_unit` 精确相等的当日最新 quote；候选 Bond 集合沿用 authority 已批准的 R4a coverage 边界。每个 Futures / Bond 都必须同时有正 bid 与正 ask、`bid <= ask`，服务端 exact midpoint=`(bid + ask) / 2`；不能精确表示时失败，不舍入、不接受单边 fallback。
5. purchase date 固定为 `valuation_at.local_trading_date`；delivery date 固定为 exact FuturesContract `settlement_time.local_trading_date`；delivery-month-first 是该日所在月一日。valuation 必须不晚于 last-trade instant，三个 MarketTime 的 timezone / local date 必须自洽。调用方不能覆盖日期。
6. base CTD 按既有 delivery basket 的最大 `implied_repo_rate` 与稳定 tie-break 选择。该指标及 conversion factor 与 financing rate 无关；R4d-b 不绑定 FundingRulePack，也不输出 carry / financing / IRR。若为了复用既有 engine 必须传 financing rate，只能传冻结的内部零值，并用对照证明任意非负 funding rate 不改变所消费的 CTD / conversion factor；不得把零值描述成主体资金成本。
7. shock 内 base CTD 与 conversion factor 固定。对每个 Factor 只 bump 对应 canonical curve node，以 R4d-a 同一完整节点集 / direction / `REBUILD + EXCLUDE` 重定价 CTD Bond。delivery engine 不得在 shock 内再次调用；不处理 CTD switching。
8. R4d-b 只接受含 `contract_size_in_quote_units` 的 cgb RulePack product rule，缺项准确返回 `RulePackItemMissing`。公式固定为：`ctd_quote_krd = ctd_registered_face_krd × RulePack.face_quote_basis / registered_face`；`one_contract_krd = ctd_quote_krd × RulePack.contract_size_in_quote_units / conversion_factor`；`position_krd = one_contract_krd × signed_contract_count`。所有乘除必须 exact 可表示，否则失败，不引入容差或隐式舍入。现有 `FuturesContract.multiplier` 不参与 R4d-b 数值，也不得作为 pack 缺项的兜底。
9. Futures `Position.quantity` 必须有符号、非零、整数 coefficient、scale=0、dimension=`contract_count`；FuturesContract `price_unit` dimension=`price_per_100`。owner、scale、precision 与输出 `dv01` Unit 全部精确校验；不做 FX 或 Unit 1:1 猜测。
10. curve node bindings 定义完整 Factor axis并返回 exact zero。每个非零 Futures exposure 必须同时存在 CTD Bond target → Factor 与 Futures Instrument target → Factor binding；任一侧缺失都整单失败。静态 binding 只证明拓扑，不提供数值权重。
11. Futures position content hash / lineage 覆盖 PositionSnapshot / Position、FuturesContract 完整 definition、DataSnapshot / exact quote bytes、RulePack definition / content及解析出的 contract size、base CTD Bond / Calendar / Unit、CTD id / conversion factor、CurveSnapshot / points、CurveNode / FactorDefinition 与两侧 binding、Unit 和算法 identity。portfolio hash 覆盖稳定排序的全部 Bond + Futures position hashes、完整 LineageRef 与 optional data snapshot id；相同输入逐字节稳定。
12. `CalculateKeyRateDv01Request` 与结果只做 §4 加法；service 名、method、既有字段 tag 与纯 Bond 行为保持。完整组合 algorithm identity 改为 §4 的新 profile；不把旧 bond-only identity 冒充全组合。
13. migration acceptance 必须把完整 forward inventory 精确冻结为 0001–0020，证明 0020 只登记一次、重复执行不改变 migration 集合，且人为注入 0020 末尾失败时新增列、约束和 migration history 全部原子回滚。只允许更新首个 forward-migration 测试及专用于 0020 的局部断言 / helper；既有 legacy / FK 判据、0017–0019 原子失败夹具、失败消息和其余三个测试保持，不得以只把 `19` 改成 `20` 顶替新迁移证据。
14. 不得修改 expected、Oracle、断言、容差、Golden、Phase 2C/2D matrix、guarded hash、selector、command、canonical Arrow schema/hash、`scripts/layering-allowlist.json` 或分层门禁断言。allowlist 必须保持 `[]`。任何冻结清单外路径在首次写入前停止并取得 Human 明确授权；不得编辑 §6 追认。

## 3. 非目标

- 动态 CTD switching、shock 后重建交割篮子、情景 CTD 概率、基差 / carry / IRR 风险分解；本轮只做固定 base CTD 的一阶 KRD。
- 调用方提交 product、价格、候选券、交割日、CTD、conversion factor、Factor convention、DV01、KRD、权重或 totals；现有 Rates 输入和已有 delivery Artifact 不得成为可信捷径。
- 单边 quote fallback、midpoint 之外的估值价格方法、结算价、可信度分层、CoverageDeclaration 或 DataHealthReport；后两者仍由 R5 承接。
- IRS、信用债、ETF、期权、浮息、不规则、含权、二阶风险、partial portfolio、多币种 / FX、跨 curve family 聚合。
- 修改 R4a `AnalyzeFuturesDelivery` 或 R3 `AnalyzeFuturesHedge` wire / 输出语义；修复其 caller-provided product / dates / prices 历史边界不在本轮。
- 修改 C++ / C ABI、既有定价公式、`analytics.proto`、`position.proto`、`factor.proto`、canonical Arrow schema、Golden、Oracle、matrix、allowlist、除新增 v2 校验外的 generator 行为、其他 scripts、authority 三件套、任何 ADR、`.github/**`、`cicd.yml` 或 `deploy/**`。

## 4. 公共契约变化

- `ficant.market.v1.FuturesContract` 保留字段 1–6；新增 `product_code = 7` 与 `price_unit = 8`。`product_code` 是 RulePack 内 exact product identity，不能从 symbol 推断；`price_unit` 是 verified snapshot 中该合约及其 CTD candidate clean-price quote 的 exact UnitRef。legacy contract 可读，但缺 7 / 8 时风险请求拒绝。既有 `multiplier = 5` 的历史语义未被合同或 RulePack 充分证明，R4d-b 不消费、不重解释也不删除它。
- `CgbFuturesProductRule` 保留字段 1–5；新增 optional `contract_size_in_quote_units = 6`。发布新的 `cgb-futures-v2` 内容：TS=`20000`，TF/T/TL=`10000`，分别来自中金所公布的 200 万 / 100 万合约标的面值除以每百元净价报价。v1 JSON / bin 与其 hash 保持逐字节不变，既有 delivery 路径继续可用；R4d-b 对缺字段的 v1 pack 失败关闭。生成脚本只扩大为同时验证 v1 / v2 deterministic payload，不改变 v1 目标或 bytes。
- `CalculateKeyRateDv01Request` 保留字段 1–5；新增 `futures_data_snapshot_id = 6`。字段在含任何 Futures exposure 时必填，在纯 Bond 请求时必须缺失。RulePack ref 由 exact FuturesContract 取得；请求不新增 product、price、candidate、CTD、conversion factor 或 FundingRulePack。
- `PortfolioKeyRateExposure` 保留字段 1–7；新增 `futures_data_snapshot_id = 8`，与请求中实际消费的 verified snapshot 精确一致；纯 Bond 结果缺失。position / total wire shape 不变，Instrument VersionRef 区分 Bond 与 Futures。
- 完整组合 algorithm identity 固定为 `ficant.fixed-income.portfolio-key-rate-yield` version 1、convention profile `linear-ytm-fixed-base-ctd-v1`。纯 Bond 请求保持 R4d-a `ficant.fixed-rate-bond.key-rate-yield` version 1；调用方不能选择 profile。Factor bump / direction / rebuild / second-order convention 仍完全来自 immutable FactorDefinition。
- application 增加 transport-neutral 的 registered-futures CTD materialization 路径，复用 R4a exact definition、verified DataSnapshot、RulePack parser 与 delivery engine；它从 quote universe 和 registered Bond pricing terms 构造输入，不接收 Rates protobuf。R4a 原 `MaterializeFuturesDeliveryInputs` 行为与测试保持。
- PostgreSQL forward-only migration 0020 为 FuturesContract 的 product / price Unit 增加 nullable legacy columns 与 all-or-none 一致性；不改写 0001–0019。新 risk-ready definition 使用完整字段，legacy row 保留但 R4d-b fail-closed。

## 5. 需 Human 决策

- **已裁决——轮次与 AC：** Human 批准严格顺序 `R4d-a → R4d-b`。R4d-b 只有在期货逐仓位 KRD 与 Bond + Futures totals 同时成立后才申请 AC16；不能以 R4d-a bond-only 结果顶账。
- **已裁决——shock 边界：** 每个请求先以 R4a verified DataSnapshot / exact RulePack 得到 base CTD 与 conversion factor；shock 内固定二者并重定价 CTD Bond，不处理 CTD switching。只有全部 included Bond / Futures 仓位都能内部重定价时才成功，不返回 partial coverage。
- **已裁决——具体合约的风险 selector：** Human 于 2026-08-01 批准 §4 的加法字段 `product_code = 7`、`price_unit = 8`。product 与 quote Unit 来自 exact definition；不得从 symbol 或 R4d-b 请求推断。现有 `multiplier = 5` 不在本轮重新定义。
- **已裁决——L3 合约规模与 authoritative source：** Human 于 2026-08-01 批准给 `CgbFuturesProductRule` 新增 `contract_size_in_quote_units = 6` 并发布不改 v1 的 v2 pack。中金所官方产品表明确 TS 合约标的面值 200 万且按每百元净价报价，TF / T / TL 为 100 万且同样按每百元报价，因此比值分别为 20000 / 10000。来源：[TS](https://www.cffex.com.cn/en_new/2ts.html)、[TF](https://www.cffex.com.cn/en_new/5tf.html)、[T](https://www.cffex.com.cn/en_new/10t.html)、[TL](https://www.cffex.com.cn/en_new/30yearCGBFutures.html)。该数值因交易所合约条款改变，按 SPEC 属 L3；放进 FuturesContract 或继续使用 C++ 的统一 100 万常量都会复制错误真相。generator 门禁 diff 必须独立呈现，且只许增加 v2 验证，v1 bytes 必须由冻结 object ID 证明不变。
- **已裁决——服务端价格：** Human 于 2026-08-01 批准所有 CTD candidate 与 Futures quote 均要求双边并采用 exact midpoint；单边或除以二不可精确表示时失败。当前 snapshot 没有 settlement / last price 或估值价格方法对象，本轮不另行推断其他价格口径。
- **已裁决——funding-neutral CTD：** Human 于 2026-08-01 批准只消费既有 basket 的 `implied_repo_rate` 与 conversion factor，不绑定 FundingRulePack；内部零 financing 仅用于满足旧 engine input，并以 funding-rate 对照证明不影响所消费结果。R4d-b 不输出或声称资金成本、net basis、funding-adjusted IRR。
- **已裁决——wire 与算法身份：** Human 于 2026-08-01 批准 request 新增 `futures_data_snapshot_id = 6`、result 新增同名 tag 8；含期货时必填 / 回显，纯 Bond 时缺失。完整组合使用新 algorithm id/profile，纯 Bond 保留 R4d-a identity，避免同一 metadata 同时指称两种输入闭包。
- **执行期事前授权——既有 Rates fixture 的加法字段初始化：** 2026-08-02 首次 `check-fast.ps1` 在编译 `crates/ficant-api/tests/rates_service.rs` 时因 `CgbFuturesProductRule.contract_size_in_quote_units` 新增而失败；全仓只读枚举确认该文件是冻结清单外唯一遗漏的既有 Rust 字面量。Agent 在首次写入前停止并取得 Human 明确授权，只允许给该 legacy delivery fixture 补 `contract_size_in_quote_units: None`，不得改断言、expected 或测试行为。此项是加法 proto 的机械编译兼容，不改变 R4d-b 语义，也不就地改写 §6 冻结清单。
- **事后发现的写路径偏差与后续保留授权——domain 公开导出的清单遗漏：** 2026-08-02 终态 changed-path 审计才发现 `crates/ficant-domain/src/research/mod.rs` 已被实施修改但未列入冻结写路径；因此这不是事前授权，后续必要性也不追认当时的越界。Agent 发现后立即停止，Human 随后只授权保留并继续验证 `scale_futures_key_rate_dv01` 这一项公开导出；不得改变其他 module、导出或 domain 语义。§6 冻结清单保持原文，最终 diff 必须证明该文件仅有这一行加法式导出变化。该实质边界干净的结论不消除原授权缺口。
- **已批准但尚未写 ADR 的 ADR-0015 建议 diff：** R4d-a §5 所记 Factor convention 执行说明继续有效；R4d-b 完成后 ADR-0015 的“权重 / 总敞口”数值要求才全部落实。ADR 不在本轮写路径，由 Human 决定是否单独修订。
- **authority 前置：** agent 不改 authority 三件套。公共候选 rebase merge 后，authority 必须以新 public SHA 重新冻结，Human 才能逐条批准并点亮 AC16，同时把 MANUAL 从“债券子组合”改为“Bond + exact Futures 全组合”及固定 CTD / midpoint 边界。

## 6. 最终真实测试证据

**双 base 冻结：** 2026-08-01 在公共 worktree `C:\git\ficant-r4d-b-futures-krd` 执行 `git fetch --prune origin`，亲自确认工作区干净、`HEAD == origin/main == cc19182ee1a5d857f5da1c25d29c27b0f3a9de7e`，branch 为 `codex/r4d-b-futures-krd`。authority worktree `C:\git\ficant-authority-r4d-b-base` 同样 fetch 后干净、detached `HEAD == origin/main == 0fd4073f8513317f99c220c5d1a98c5ee8d79b51`；`verify-authority.ps1 -ExpectedAuthorityCommit 0fd4073f8513317f99c220c5d1a98c5ee8d79b51` exit 0，3 份 authority 文档哈希成立，manifest 精确绑定公共 `cc19182ee1a5d857f5da1c25d29c27b0f3a9de7e`。以上双 base 自此固定不变。

**冻结允许写路径（Human 批准 §5 后也只能逐项写入）：**

- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/composition.rs`
- `binaries/ficant-server/tests/portfolio_risk_sit.rs`
- `crates/ficant-api/src/portfolio_risk.rs`
- `crates/ficant-api/tests/portfolio_risk_service.rs`
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/ports/fingerprint.rs`
- `crates/ficant-application/src/ports/rule_pack_parser.rs`
- `crates/ficant-application/src/use_cases/futures_delivery.rs`
- `crates/ficant-application/src/use_cases/portfolio_risk.rs`
- `crates/ficant-application/tests/definition_aggregate.rs`
- `crates/ficant-application/tests/futures_delivery_input_bindings.rs`
- `crates/ficant-application/tests/r4d_a_bond_krd_contracts.rs`
- `crates/ficant-application/tests/r4d_b_futures_krd_contracts.rs`（新建）
- `crates/ficant-cgb-futures-pack/src/lib.rs`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contracts/src/generated/ficant.market.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.rs`
- `crates/ficant-domain/src/futures_delivery.rs`
- `crates/ficant-domain/src/market/futures_contract.rs`
- `crates/ficant-domain/src/research/exposure.rs`
- `crates/ficant-domain/tests/r4d_b_futures_krd_contracts.rs`（新建）
- `crates/ficant-storage/src/postgres/codec.rs`
- `crates/ficant-storage/src/postgres/definitions.rs`
- `crates/ficant-storage/tests/factor_topology_postgres.rs`
- `crates/ficant-storage/tests/migration_acceptance.rs`
- `crates/ficant-storage/tests/r4d_b_futures_krd_postgres.rs`（新建）
- `docs/architecture/layering-refactor.md`
- `domain-packs/cgb-futures/cgb-futures-v2.bin`（新建）
- `domain-packs/cgb-futures/cgb-futures-v2.json`（新建）
- `docs/iterations/2026-08-r4d-b-futures-krd.md`（新建）
- `docs/iterations/README.md`
- `interface/proto/ficant/market/v1/definition.proto`
- `interface/proto/ficant/market/v1/cgb_futures_rule.proto`
- `interface/proto/ficant/research/v1/exposure.proto`
- `interface/README.md`
- `migrations/postgresql/0020_r4d_b_futures_risk_terms.sql`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/definition_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/cgb_futures_rule_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/exposure_pb2.py`
- `scripts/generate-cgb-futures-pack.ps1`
- `web-dm/packages/contracts-generated/src/ficant/market/v1/definition_pb.ts`
- `web-dm/packages/contracts-generated/src/ficant/market/v1/cgb_futures_rule_pb.ts`
- `web-dm/packages/contracts-generated/src/ficant/research/v1/exposure_pb.ts`

**禁止写路径：** 所有未逐项列出的路径。特别禁止 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、所有 ADR、`README.md`、既有 R1–R4d-a brief、`analytics.proto`、`position.proto`、`factor.proto`、其他 `.proto` / generated output、`crates/ficant-api/src/rates.rs`、`crates/ficant-domain/src/analytics.rs`、`crates/ficant-domain/src/curves.rs`、`crates/ficant-domain/src/futures_hedge.rs`、`crates/ficant-fixed-income-native/**`、`crates/ficant-kernel-sys/**`、`cpp/**`、除两个新建 v2 文件外的 `domain-packs/**`、除点名 generator 外的 `scripts/**`、`tests/golden-cases/**`、`tests/oracle/**`、`tests/phase2c/**`、`tests/phase2d/**`、`crates/ficant-data/src/canonical.rs`、`Cargo.lock`、`.gitignore`、`.github/**`、`cicd.yml` 与 `deploy/**`。本清单与双 base 同时冻结；扩权只能由 Human 在首次写入前批准并新增 §5 记录，本节不得就地改写。generator 属自管门禁，最终 diff 必须单独呈现且只能扩大 v2 覆盖；不得删除 v1 检查或降低 Buf / byte equality 判据。

**受保护 base 事实（Git object ID，实施期只能保持不变）：**

- `scripts/layering-allowlist.json`：blob `fe51488c7066f6687ef680d6bfaa4f7768ef205c`，内容为 `[]`
- `crates/ficant-data/src/canonical.rs`：blob `79e42b00c645710b8179d515ba02f79cd9d38fc4`
- `tests/golden-cases`：tree `11f981972612e617591de1c3daaa36d114a7cab9`
- `tests/oracle`：tree `539889f598c8118854ea679375695c9721696932`
- `tests/phase2c/acceptance-matrix.json`：blob `26e72186490a0ab2cae142c9d88436ae07cc8da8`
- `tests/phase2d/acceptance-matrix.json`：blob `d6feaed93a8df00176f2873d28d1e03d6d789f75`
- `cpp`：tree `e600f8de0a485d5db5edf7eac20e5ea89698716f`
- `crates/ficant-kernel-sys`：tree `3350c80bdc3c54159fa9b4e6bb4e26ca33218f0d`
- `crates/ficant-fixed-income-native`：tree `3e622b8a1e4786a8183530d63a4d3d41be8a953b`
- `interface/proto/ficant/rates/v1/analytics.proto`：blob `ae49a0b44959f7ec42b2639ae4a5fd29ece94335`
- `interface/proto/ficant/research/v1/position.proto`：blob `35ca895737cc0a19168fae0959b4dc7dac618acf`
- `interface/proto/ficant/research/v1/factor.proto`：blob `ed998a2a142836e8eb17e8861e0bbf1fc3bad1ac`
- `crates/ficant-api/src/rates.rs`：blob `3d6249b0c317a225aa2a6c6af2893b1d6aaa9930`
- `domain-packs/cgb-futures/cgb-futures-v1.json`：blob `1fe9db105d15f2f3924b8f488108311611ca7f07`
- `domain-packs/cgb-futures/cgb-futures-v1.bin`：blob `469445e4199020dae0a705be42a0569e72a73f05`

**RED-first 与 forward-only checkpoint：** domain RED 使用 `cargo test --offline --locked -p ficant-domain --test r4d_b_futures_krd_contracts`，exit 101，首个真实错误是尚不存在 `scale_futures_key_rate_dv01` 及 Futures risk selector / portfolio snapshot 加法 API；它不是 checkpoint。transport RED 使用 `cargo test --offline --locked -p ficant-api --test portfolio_risk_service`，exit 101，首个真实错误是生成合同尚无 `futures_data_snapshot_id` 字段；它不是 checkpoint。application 的 server-derived CTD 测试没有在实现前独立取得 RED：测试与实现同一子循环形成后首次运行即为 GREEN，因此本 brief 不宣称取得该 RED；这是流程偏差，不以最终绿灯倒推或补造 RED。forward-only checkpoint 依次为：domain 公式 / hash / selectors `3/3`；v2 parser 与 deterministic payload `1/1 + generator exit 0`；application 最终 `4/4`（其中终态审计新增 Futures topology 缺失时三个数值引擎均为 0 的负向判据）；storage risk-ready/legacy round-trip `1/1` 与 migration `4/4`；transport / descriptor / production composition `3/3 + 17/17 + 2/2`。

**最终针对性证据（全部必须在同一候选真实执行）：**

- `cargo test --offline --locked -p ficant-domain --test r4d_b_futures_krd_contracts`
- `cargo test --offline --locked -p ficant-cgb-futures-pack`
- 注入固定 Buf 1.56.0 后 `pwsh -NoProfile -NonInteractive -File scripts/generate-cgb-futures-pack.ps1 -Check`（v1 / v2 均须逐字节匹配）
- `cargo test --offline --locked -p ficant-application --test r4d_b_futures_krd_contracts`
- `cargo test --offline --locked -p ficant-application --test r4d_a_bond_krd_contracts`
- `cargo test --offline --locked -p ficant-application --test futures_delivery_input_bindings`
- `cargo test --offline --locked -p ficant-storage --test r4d_b_futures_krd_postgres`
- `cargo test --offline --locked -p ficant-storage --test migration_acceptance`（必须完整 4/4）
- `cargo test --offline --locked -p ficant-api --test portfolio_risk_service`
- 注入固定 Buf 1.56.0 后 `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory`
- `cargo test --offline --locked -p ficant-server --test portfolio_risk_sit`
- `pwsh -NoProfile -NonInteractive -File scripts/check-layering.ps1`
- `pwsh -NoProfile -NonInteractive -File scripts/test-layering-check.ps1`

2026-08-02 在最终候选逐条重跑的真实结果：

| 命令 | 结果 |
|---|---|
| `cargo test --offline --locked -p ficant-domain --test r4d_b_futures_krd_contracts` | exit 0，3/3 |
| `cargo test --offline --locked -p ficant-cgb-futures-pack` | exit 0，1/1；doc-tests 0/0 |
| 注入 Buf 1.56.0 后 `pwsh -NoProfile -NonInteractive -File scripts/generate-cgb-futures-pack.ps1 -Check` | exit 0；v1 / v2 bin 与 JSON deterministic 编码逐字节一致 |
| `cargo test --offline --locked -p ficant-application --test r4d_b_futures_krd_contracts` | exit 0，4/4 |
| `cargo test --offline --locked -p ficant-application --test r4d_a_bond_krd_contracts` | exit 0，4/4 |
| `cargo test --offline --locked -p ficant-application --test futures_delivery_input_bindings` | exit 0，3/3 |
| `cargo test --offline --locked -p ficant-storage --test r4d_b_futures_krd_postgres` | exit 0，1/1，真实 PostgreSQL |
| `cargo test --offline --locked -p ficant-storage --test migration_acceptance` | exit 0，4/4；精确 inventory 0001–0020、0020 单次登记 / repeat no-op / 注入失败原子回滚 |
| `cargo test --offline --locked -p ficant-api --test portfolio_risk_service` | exit 0，3/3 |
| 注入 Buf 1.56.0 后 `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory` | exit 0，17/17 |
| `cargo test --offline --locked -p ficant-server --test portfolio_risk_sit` | exit 0，2/2；含生产 native engine 的 0 / 非零 funding 对照，financing cost 改变而 IRR、conversion factor、CTD 不变 |
| `pwsh -NoProfile -NonInteractive -File scripts/check-layering.ps1` | exit 0；AC03=0、AC01=0、C++/FFI=0、Funding=0、Tax=0、allowlist=0 |
| `pwsh -NoProfile -NonInteractive -File scripts/test-layering-check.ps1` | exit 0，51 assertions |

补充回归：最终 `check-fast.ps1` 中 `definition_aggregate` 6/6、既有 `rates_service` 15/15，证明 legacy Futures 可读、risk selector fingerprint 加法，以及获授权的 `contract_size_in_quote_units: None` fixture 初始化没有改变 Rates 行为。

**完整本地检查（最终候选必须真实执行）：**

- `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`
- 使用仓库锁定工具链执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1`
- 导入六个 Windows User 级 `FICANT_TEST_*` 变量且不输出密钥后，执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration`

最终同一代码候选的真实结果：

- `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`：exit 0，`FICANT fast local checks passed.`
- 锁定 Node 22.17.0、pnpm 10.12.4、Buf 1.56.0、uv 0.7.13 / Python 3.12.11 后 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1`：exit 0，Rust strict Clippy / build / tests、descriptor 17/17、C++ 8/8、Phase 2C / 2D Oracle 各 3/3、Web 35/35 与其余规定切片全部通过。
- 从 Windows User scope 导入六个 `FICANT_TEST_*` 值且未输出值后，使用同一锁定工具链运行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration`：exit 0。PostgreSQL migration 4/4、lease 1/1、execution closure 3/3、worker 1/1、Phase 1 1/1、negative invariants 13/13、Phase 2B / 2C / 2D 各 1/1、Phase 3A registry / parity 各 1/1、Phase 3B codec 3/3 / publication 1/1，Ceph / PostgreSQL 所需切片全部通过。
- 两次环境前置失败不计入绿灯：首次直接入口由活动 Node 24.18.0 被锁定版本门禁拒绝；改用 Node 22.17.0 后曾因该新 worktree 缺 `web-dm/node_modules` 在 `tsc` 前退出。随后执行 `corepack pnpm@10.12.4 install --frozen-lockfile --offline`，178 个包全部从缓存复用、下载数 0，再从完整入口取得上述 exit 0。严格 Clippy 首次暴露的文档、enum size 与函数长度告警均在允许路径内机械修复并由最终完整入口复核。

**终态审计：** `git diff --check` exit 0。相对 execution base 的精确 changed-path 集合共 41 项：39 项属于冻结清单；`rates_service.rs` 在首次写入前取得 §5 扩权；`research/mod.rs` 是事后才发现、随后获准保留的已披露偏差，不能倒算为原写路径合规。按最终允许保留的集合复核 unexpected=0：

- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/portfolio_risk_sit.rs`
- `crates/ficant-api/src/portfolio_risk.rs`
- `crates/ficant-api/tests/portfolio_risk_service.rs`
- `crates/ficant-api/tests/rates_service.rs`（§5 扩权）
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/ports/fingerprint.rs`
- `crates/ficant-application/src/ports/rule_pack_parser.rs`
- `crates/ficant-application/src/use_cases/futures_delivery.rs`
- `crates/ficant-application/src/use_cases/portfolio_risk.rs`
- `crates/ficant-application/tests/definition_aggregate.rs`
- `crates/ficant-application/tests/r4d_b_futures_krd_contracts.rs`
- `crates/ficant-cgb-futures-pack/src/lib.rs`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contracts/src/generated/ficant.market.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.rs`
- `crates/ficant-domain/src/futures_delivery.rs`
- `crates/ficant-domain/src/market/futures_contract.rs`
- `crates/ficant-domain/src/research/exposure.rs`
- `crates/ficant-domain/src/research/mod.rs`（§5 事后偏差与后续保留授权，仅新增公式导出）
- `crates/ficant-domain/tests/r4d_b_futures_krd_contracts.rs`
- `crates/ficant-storage/src/postgres/codec.rs`
- `crates/ficant-storage/src/postgres/definitions.rs`
- `crates/ficant-storage/tests/migration_acceptance.rs`
- `crates/ficant-storage/tests/r4d_b_futures_krd_postgres.rs`
- `docs/iterations/2026-08-r4d-b-futures-krd.md`
- `docs/iterations/README.md`
- `domain-packs/cgb-futures/cgb-futures-v2.bin`
- `domain-packs/cgb-futures/cgb-futures-v2.json`
- `interface/README.md`
- `interface/proto/ficant/market/v1/cgb_futures_rule.proto`
- `interface/proto/ficant/market/v1/definition.proto`
- `interface/proto/ficant/research/v1/exposure.proto`
- `migrations/postgresql/0020_r4d_b_futures_risk_terms.sql`
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/cgb_futures_rule_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/definition_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/exposure_pb2.py`
- `scripts/generate-cgb-futures-pack.ps1`
- `web-dm/packages/contracts-generated/src/ficant/market/v1/cgb_futures_rule_pb.ts`
- `web-dm/packages/contracts-generated/src/ficant/market/v1/definition_pb.ts`
- `web-dm/packages/contracts-generated/src/ficant/research/v1/exposure_pb.ts`

15 项受保护 base object 全部保持 brief 冻结的 object ID，worktree diff 也为 0；因此 allowlist 仍为 `[]`，canonical Arrow、Golden、Oracle、Phase 2C/2D matrix、C++、C ABI、Rates proto / implementation、position / factor proto 与 v1 cgb JSON / bin 均未变。实际变更 proto 只有已授权的 `cgb_futures_rule.proto`、`definition.proto`、`exposure.proto`；点名禁止的 position / factor / health / constraint / policy proto 新增或修改数为 0。generator diff 保留 Buf 1.56.0、临时文件清理和逐字节 equality，只把同一检查循环扩大到 v2；v1 两个冻结 object ID 未变。

Acceptance sentence 与 AC16 本地判据成立：domain 3/3 证明 long / short、exact zero、conversion factor 与 10000 / 20000 的 2:1 缩放；application 4/4 证明 Bond + Futures 两仓位 × 三 Factor、响应外 exact totals、两个相邻非零轴 / 一个零轴、base 两券后不再调用 delivery 的固定 CTD 反证，以及普通 Futures topology 缺失时全部数值引擎为 0、CTD topology 缺失时 shock engine 为 0；server 2/2 使用生产 native engine 证明 funding-neutral CTD / conversion；storage、wire、descriptor 与生产路由证据如上。该结论只形成 R4d-b 本地自测候选，不等于 AC16 已点亮；AC16 仍只能在公共候选 rebase merge、authority 精确绑定且 Human 逐条批准后点亮。

## 7. 残余风险

- 固定 base CTD 是一期线性风险口径，不捕捉 shock 导致的 CTD switching、可交割券期权或基差非线性。若以后支持，必须使用新 algorithm version / convention profile，不能无痕改变本轮结果。
- 双边 exact midpoint 是在当前 quote schema 没有 settlement / last / valuation price method 时的窄口径，不是交易可执行价。R5 加入 price source / credibility 后应重新评估，但不能保持相同算法身份偷换价格方法。
- R4a 已批准的 candidate-universe coverage 边界继续存在：不可解析、非本 owner、非 Bond 或 unit 不匹配的 snapshot quote 不进入候选集合。本轮不把它扩写成全市场 coverage。
- `CgbFuturesProduct` enum 与 market-specific native engine 仍是一期现状；新增 `FuturesContract.product_code` 只消除具体合约上的 symbol 推断。R7 / AC04 验证新市场零 L0/L1/L2 改动时仍可能要求独立的 provider-neutral product selector 重构，成本不计入本轮。
- 既有 `AnalyzeFuturesHedge` 的 C++ 实现仍统一使用 100 万合约面值，因此对 TS 的 200 万规格不成立；该 RPC 接受 caller-provided CTD DV01 / conversion factor，且 `futures_hedge.rs`、C++ / C ABI 均不在 R4d-b 写路径。本轮新 PortfolioRisk 路径只消费 v2 RulePack 的 product-specific size，不拿 R4d-b 的正确性追认旧 hedge；修复旧 RPC 必须另开迭代并重取 Phase 2D 证据。
- R4d-b 点亮 AC16 只表示已导入 Bond + exact Futures 仓位的全组合 KRD；结果尚无 CoverageDeclaration。R5 / AC35 结束前不得把它作为无覆盖声明的组织级结论呈现。
- ADR-0015 在 R4d-b 后才完成其数值 Exposure / total 主张；若 Human 不另行修订 ADR，其验证段仍只写 AC05，属于文档债务但不改变 SPEC / ACCEPTANCE 判定权威。
- application RED 未在实现前独立取得，属于已披露的过程偏差；最终 GREEN、负向调用计数与完整检查不能追造该 RED。该偏差不改变本地技术判据，但应由 Human 在授权 commit / PR 前决定是否接受。
- `crates/ficant-domain/src/research/mod.rs` 在终态审计前已发生冻结路径外写入；后续 Human 只授权保留这一行公开导出，不追认原越界。它是本候选第二项需在 commit / PR 前由 Human 接受的过程偏差。
