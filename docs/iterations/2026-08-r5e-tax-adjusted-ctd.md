# R5E 迭代 brief — 权威国债利息税收双口径与双 CTD

**迭代：** R5E · **点亮目标：** AC09 · **execution base：** `75388c7e570befd63620f0a7d291f49dc18e4fe4` · **authority base：** `0b09a7b0b4a297cefdb564552ea3125d8ad153d6`

本 brief 是 R5E 面向 Human 的唯一设计与最终证据载体。私有 authority PR #19 已将 D1–D8、官方来源边界、canonical semantic hash、typed v2 type URL 与精确 RATE Unit identity 固化到上述 authority base；2026-08-12 Human 已授权按该规划完成 R5。本文先冻结目标、公共契约、允许路径和 RED-first 判据；§6 中尚未实际执行的命令不得写成通过。

## 1. 目标

R5E 只交付一个产品结果：在不改变市场税前 Bond / Futures Delivery 数值、既有 funding 调整口径和 Phase 2C native 公式的前提下，把 Human 批准的境内证券机构一般纳税人国债利息 coupon output-VAT-only 规则物化为确定性 `TaxRulePackV2`，并让 `AnalyzeBond` 与 `AnalyzeFuturesDelivery` 在同一成功响应中分别返回市场税前与主体税收调整后口径。交割结果对每只候选返回双 IRR，同时返回市场税前 CTD 与主体税收调整后 CTD；所有实际消费的 TaxRulePack、Subject、Unit、Bond 首发日/税收属性、来源与时间继续进入 R5D 输入证据和请求指纹。

**Acceptance sentence：**

> 给定同 owner、同 Subject、精确 RATE Unit `01K2CGBVAT0000000000000000@1`、Human 批准的 exact `TaxRulePackV2`、2025-08-08 cutoff 两侧国债与 verified 市场事实，Application 必须在任何 Bond 或 Delivery 数值 handoff 前验证规则包 owner/version/definition hash/payload hash/type URL/source/effective window、Unit definition、Subject 完整 profile pair、每只 Bond 的首发日与税收属性。成功的 `AnalyzeBond` 保留逐位相同的市场税前结果，并以同一 clean price、仅将 gross coupon 按 `gross/(1+VAT rate)` 在 12 位 ties-to-even 调整后返回显式标注 `COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT` 的主体 YTM；成功的 `AnalyzeFuturesDelivery` 保留逐位相同的市场税前候选值、`ctd_index` 与 funding-adjusted IRR，并为每只候选新增主体税收调整后 interim coupons/IRR 及独立 `subject_ctd_index`。独立 Decimal Oracle 必须证明 cutoff 前、cutoff 当日、cutoff 后续发继承、一个市场/主体 CTD 反转篮子和一个无税差对照篮子；任一规则、单位、身份、内容、来源、时点、候选税收事实或 claim scope 漂移均失败关闭且 delivery engine 调用为零。相同输入产生逐位确定的 payload、响应、证据和指纹；AC10、AC26 及 R5D 已点亮/已批准行为不回退。

## 2. 验收

| 条目 | R5E 可执行判据 |
|---|---|
| 权威规则包 | `domain-packs/cgb-interest-tax/` 中唯一产品包由 authority canonical JSON 与一方 source manifest 机械生成。normalized facts UTF-8 单行 SHA-256 精确为 `54FA5ADBEB8B164DC779ECC250AB622AB5747CDEB36F2B6DA58F4D877CE5106A`；type URL 精确为 `type.googleapis.com/ficant.market.v1.TaxRulePackV2`。两次独立生成的 JSON、protobuf bytes、payload SHA-256 与 manifest 逐字节相同；生成脚本 `-Check` 拒绝任一 stale/missing/drifted 文件。 |
| Unit 与 profile | 所有税率 `DecimalValue` 只绑定 `01K2CGBVAT0000000000000000@1`；同 tenant/规则包 owner 的 exact Unit 必须为 `RATE/rate/scale=12/precision=18`，并与请求 `context.units.rate` 相同。Subject 必须精确匹配 `cn-vat-general-taxpayer` / `cn-cgb-interest-cit-exempt`；未知或不完整 profile、不同 Unit/version/definition 内容均在 engine 前失败。 |
| typed v2 treatment | v2 payload 必须显式表达首发区间、Bond VAT/CIT 属性、完整 profile pair、VAT/CIT 法定税率、`VAT_INCLUDED` gross basis、12 位 `TIES_TO_EVEN` 与闭枚举 `COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT`。cutoff 前 VAT exempt/CIT exempt；2025-08-08 当日及以后 VAT taxable/CIT exempt。v1 parser 只保留给既有 AC10 合成机制回归，生产 server 只组合 v2 parser；不存在 v1→v2 猜测、默认税率或 silent fallback。 |
| Decimal 运算 | L0 `FixedDecimal` 新增通用有符号 ties-to-even 除法与精确整数乘法，溢出、零除数或不可表示输入失败关闭。taxable coupon=`round_half_even(gross/(1+0.06),12)`，exempt coupon identity；不得使用预截断 `6/106` 或隐藏 retained-rate 常量。Delivery 年化顺序固定为：先换算 interim coupon；再 `round_half_even((invoice+coupon)/purchase_dirty,12)-1`；精确乘 `365`；最后 `round_half_even(.../actual_days,12)`。 |
| Bond 双口径 | 既有税前 cashflows/measures 字段与语义不变；`TaxAdjustedBondAnalytics` 新增显式 claim scope。只调整 coupon、本金不变，以同一市场税前 clean price走 PriceIn 重算主体 YTM。独立 Oracle 从 pack、日期、cashflows 与价格求解，不能 import 生产税后公式、测试 expected 或 native helper。cutoff 前、当日、后续发继承均覆盖；税前 bytes 不因税收口径改变。 |
| Delivery 输入与失败关闭 | `AnalyzeFuturesDeliveryRequest` 加法新增 exact `tax_rule_pack=13`；Materializer 在调用 delivery engine 前读取一次 exact v2 pack，并对每只由 verified snapshot/contract 派生的候选 Bond 解析 treatment。规则缺项、候选 Bond 无首发日/属性、profile/unit/type/source/effective/hash/owner/knowledge/valuation 漂移均令 delivery engine 调用为零；TaxRulePack 与每只 candidate treatment 都进入参数指纹/响应证据，不能只进入 lineage。 |
| 候选双 IRR | `FuturesDeliveryMeasures` 加法新增 `tax_adjusted_interim_coupons=16` 与 `subject_tax_adjusted_irr=17`；`FuturesDeliveryCandidateResult` 新增 claim scope。旧 `interim_coupons`、`implied_repo_rate`、`funding_adjusted_irr` 与全部市场量保持逐位不变。税收调整与 funding 调整是互斥字段和独立公式，不相加、不覆盖。 |
| 双 CTD | 旧 `ctd_index=2` 保持市场税前选择：最大市场 IRR、再最小市场 net basis、再 Bond id；新增 `subject_ctd_index=4`：最大主体 IRR、再同一市场 net basis、再 Bond id。明确反转篮子必须两个 index 不同；全部候选相同税收待遇的控制篮子必须相同；输入排序变化不得改变所选 Bond identity。 |
| claim 边界 | 成功税收结果必须返回闭枚举 claim scope，UNSPECIFIED 不得出现在成功响应。文档只称“coupon 销项 VAT 调整、抵扣进项前”，不称机构最终应纳 VAT、完整税后利润、金融商品转让/期货平仓/实物交割税务或完整税务会计。境外机构、小规模纳税人、附加税、进项抵扣/分摊、交易费用与非国债均失败关闭或明确不支持。 |
| 确定性与回归 | fixed Buf 1.56.0 双临时生成树一致；Rust/Python/TypeScript consumers 同步。R5D materialization、Rates API、生产 SIT、private native Bond 双端口、Worker、Python live construction、AC10、AC26、Phase 2C Oracle/Golden/matrix、R4d-a/b KRD 和三个统一入口全部转绿。不能改变既有 Phase 2C/2D expected、容差、C/C++/FFI/native 公式或 Delivery Artifact/Arrow schema。 |

RED-first 子循环按以下顺序执行，首个真实非零命令、exit code 与首错只在 §6 最终记录，不能事后补造：

1. **contract RED：** 先修改 descriptor/consumer 判据要求 v2 treatment、claim scope、delivery tax binding、双 IRR 与双 CTD，旧契约必须失败；随后才修改 proto/生成物。
2. **tax/decimal RED：** 先建立 v2 parser与 FixedDecimal ties-to-even测试，证明旧 v1 retained-rate 实现不能表达 `gross/(1+0.06)`；再实现 L0 算术、parser、权威 pack生成/校验。
3. **Bond Oracle RED：** 独立 Python Decimal Oracle先冻结输入与新 expected，并以受控扰动证明门禁；Rust/API 再逐项匹配，不得从生产输出生成 expected。
4. **Delivery RED：** 先建立 materialization/API 调用计数、候选双 IRR、反转/对照篮子和稳定择优测试；旧单口径实现必须失败；再实现 Application/API/生产组合。
5. **consumer/regression RED：** 生成契约后让旧 Rust/Python/TypeScript/native/Worker consumers 真实编译失败，再机械迁移；最后恢复所有受保护回归。

## 3. 非目标

- 不计算进项税抵扣/分摊、附加税、纳税期最终应纳 VAT、金融商品转让价差 VAT、期货平仓或实物交割税务、交易费用、会计分录或完整税后利润。
- 不支持境外机构、小规模纳税人、地方政府债、金融债或其他债券类型；不新增第二组 profile 或自动 profile 推断。
- 不让 Factor、Exposure、KRD、Carry/Roll、Curve 或 Hedge 承载双税收口径；不改变 AC10 的合成 v1 机制边界。
- 不改变 Phase 2C/2D native C/C++、C ABI、Rust native adapter、existing Golden/Oracle/matrix/expected/容差、Delivery Artifact/Arrow schema或既有市场 CTD 选择。
- 不实施 AC37、Definition/Fact/Snapshot/Artifact 服务组合、dead gRPC-Web/`ficant-web` 清理、完整 crate 拆分、AC30–AC33、跨编译器裁决、DMQuant、Policy/Constraint、完整 DataHealth、AI sandbox 或 UI 主动呈现。
- 不修改 private authority 三件套（post-merge 绑定除外）、公共根目录本地 authority 副本、现有未跟踪 `docs/review/full-audit-2026-08-07.md`、CI/CD、远端安全设置、版本 tag、镜像或部署。

## 4. 公共契约变化

R5E 对 v0.1 契约只做加法变化，不删除或改义既有 field/tag。固定 Buf 1.56.0 完整生成后同步登记 consumers。

`ficant.market.v1` 新增：

- `TaxRulePackV2 { repeated BondCouponTaxTreatmentRule coupon_rules = 1; }`。
- `BondCouponTaxTreatmentRule` 保留 v1 的 `first_issue_from=1`、`first_issue_to=2`、`tax_attributes=3`，并以 `repeated SubjectCouponTaxTreatment treatments=4` 承载 v2 treatment。
- `SubjectCouponTaxTreatment` 固定字段：`value_added_tax_profile=1`、`income_tax_profile=2`、`value_added_tax_rate=3`、`income_tax_rate=4`、`gross_coupon_basis=5`、`rounding=6`、`claim_scope=7`。所有 financial decimals 使用 `DecimalValue`。
- 闭枚举 `GrossCouponTaxBasis`: `UNSPECIFIED=0`、`VAT_INCLUDED=1`；`TaxRoundingMode`: `UNSPECIFIED=0`、`TIES_TO_EVEN=1`；`CouponTaxClaimScope`: `UNSPECIFIED=0`、`COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT=1`。生产成功路径拒绝所有 UNSPECIFIED。

`ficant.rates.v1` 加法变化：

- import market tax contract 并在 `TaxAdjustedBondAnalytics.claim_scope=3`、`FuturesDeliveryCandidateResult.claim_scope=3` 使用 `ficant.market.v1.CouponTaxClaimScope`。
- `AnalyzeFuturesDeliveryRequest.tax_rule_pack=13`；`FuturesDeliveryMeasures.tax_adjusted_interim_coupons=16`、`subject_tax_adjusted_irr=17`；`AnalyzeFuturesDeliveryResult.subject_ctd_index=4`。
- `ctd_index=2`、`implied_repo_rate=13` 与 `funding_adjusted_irr=15` 原 tag/原义不变。`TaxRulePack` response evidence 仍使用已有 `ANALYSIS_INPUT_ROLE_TAX_RULE_PACK=8`，不新增重复 role。

Application 的 provider-neutral shape 从单一 `CouponTaxRate` 收敛为不可变 `CouponTaxTreatment`：携带 VAT/CIT rate、Unit、basis、rounding、claim scope，并提供唯一的 `adjust_coupon`。v1 parser 仍可映射到 legacy retained-rate treatment 但只用于显式选择 v1 parser 的既有测试；v2 parser是生产唯一组合。Bond private materialized seam 必须序列化完整 treatment 与 proof，不再只传一个可漂移 scalar；implementation digest随私有 contract shape变化。

Delivery 税收组合位于 Application/API 边界：native `FuturesDeliveryBasketResult`、C/C++ 和 Arrow Artifact 继续只表示市场税前事实。Application 在 handoff 前完成候选 treatment解析；API在 native结果返回后以已验证 treatments计算税收字段和 subject CTD，不能重新读取仓储、猜测税率或改变 native候选值。

## 5. 需 Human 决策

本轮没有未决 Human 选择。以下事项已由 authority base `0b09a7b0b4a297cefdb564552ea3125d8ad153d6` 冻结：D1 适用主体/排除、D2 cutoff/续发分类、D3 税率、D4 gross含税与12位 ties-to-even约定、D5 Bond双口径、D6 Delivery双 IRR且与 funding隔离、D7双CTD稳定择优、D8 semantic hash/type URL/Unit/source/effective window。R5E 不得在实施中改写这些语义、扩大税务 claim 或把 AC09提前标为点亮。

若需改变任一枚举、字段号、算术顺序、TaxRulePack内容/profile/rate/source/effective window、允许路径、受保护 expected/Oracle/容差/断言方向、C/C++/Artifact schema、authority、CI/CD或部署，必须在首次写入前停止并取得 Human明确扩权；不得先改后补。

## 6. 最终真实测试证据

**规划落盘边界：** 2026-08-12 核验公共 `HEAD == origin/main == 75388c7e570befd63620f0a7d291f49dc18e4fe4`，authority `HEAD == origin/main == 0b09a7b0b4a297cefdb564552ea3125d8ad153d6` 且 verifier exit `0`。规划阶段只允许新建本文并修改 `docs/iterations/README.md`；未跟踪审计报告保持候选外。本段不是代码通过证据。

**R5E 实施允许写路径（冻结闭集）：**

- `Cargo.toml`、`Cargo.lock`
- `interface/proto/ficant/market/v1/tax_rule.proto`
- `interface/proto/ficant/rates/v1/analytics.proto`
- `crates/ficant-contracts/src/generated/ficant.market.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.rates.v1.rs`
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/tax_rule_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/rates/v1/analytics_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/rates/v1/analytics_pb2_grpc.py`
- `web-dm/packages/contracts-generated/src/ficant/market/v1/tax_rule_pb.ts`
- `web-dm/packages/contracts-generated/src/ficant/rates/v1/analytics_pb.ts`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `python/tests/test_contract_import.py`
- `web-dm/platform-shell/tests/contracts-consumer.test.ts`
- `crates/ficant-domain/src/primitives/fixed_decimal.rs`
- `crates/ficant-domain/tests/fixed_decimal_rounding.rs`（新建）
- `crates/ficant-application/src/ports/tax_rule_parser.rs`
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/use_cases/tax_rule.rs`
- `crates/ficant-application/src/use_cases/rates_materialization.rs`
- `crates/ficant-application/tests/tax_rule_resolution.rs`
- `crates/ficant-application/tests/r5d_rates_materialization.rs`
- `crates/ficant-application/tests/r5e_tax_materialization.rs`（新建）
- `crates/ficant-tax-pack/src/lib.rs`
- `crates/ficant-tax-pack/tests/r5e_authoritative_pack.rs`（新建）
- `crates/ficant-api/Cargo.toml`
- `crates/ficant-api/src/rates.rs`
- `crates/ficant-api/tests/rates_service.rs`
- `crates/ficant-api/tests/phase2e_sdk_live.rs`
- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/rates_sit.rs`
- `crates/ficant-native-nodes/src/lib.rs`
- `crates/ficant-native-nodes/tests/cgb_bond_analytics.rs`
- `binaries/ficant-worker/tests/phase4_worker_sit.rs`
- `python/tests/test_rates_sdk_live.py`
- `scripts/generate-cgb-interest-tax-pack.ps1`（新建）
- `domain-packs/cgb-interest-tax/cgb-interest-tax-v1.json`（新建）
- `domain-packs/cgb-interest-tax/cgb-interest-tax-v1.bin`（新建）
- `domain-packs/cgb-interest-tax/cgb-interest-tax-source-manifest.json`（新建）
- `tests/golden-cases/china-rates/r5e-tax-adjusted-analytics-inputs.json`（新建）
- `tests/golden-cases/china-rates/expected/r5e-tax-adjusted-analytics-expected.json`（新建；只由独立 Oracle首次冻结）
- `tests/oracle/china-rates/r5e_tax_adjusted_decimal_oracle.py`（新建）
- `tests/oracle/china-rates/test_r5e_tax_adjusted_decimal_oracle.py`（新建）
- `README.md`、`docs/product/scope.md`、`docs/development.md`
- `scripts/check-fast.ps1`、`scripts/check.ps1`
- `docs/iterations/2026-08-r5e-tax-adjusted-ctd.md`（实施期只填本节最终真实证据与 §7 残余风险；不得改写 §1–§5或本允许路径）

**禁止写路径：** 所有未逐项列出的路径。特别禁止 private authority（最终公共 merge 后的独立 post-merge绑定除外）、公共根目录 ignored authority、副本审计报告、既有 generated tonic文件（服务形状不变）、migration、storage/Arrow/Artifact codec、C/C++/FFI/native fixed-income实现、现有 `domain-packs/cgb-futures/**`、Phase 2B/2C/2D matrix及所有既有 Golden/Oracle/expected/容差、ADR、`.github/workflows/**`、`cicd.yml`、`deploy/**`、版本和远端设置。

**受保护事实：** 从 execution base 到最终候选，`cpp/**`、`crates/ficant-kernel-sys/**`、`crates/ficant-fixed-income-native/**`、`crates/ficant-storage/src/*arrow*`、migrations、`domain-packs/cgb-futures/**`、`tests/phase2c/acceptance-matrix.json`、`tests/phase2d/acceptance-matrix.json`、所有既有 `tests/golden-cases/**` 与 `tests/oracle/**`（除四个R5E新文件）、existing delivery schema/hash、R4d KRD fixture/expected、现有 tolerance和断言方向逐 blob不变。

**计划的最终候选命令（结果待真实执行）：**

- fixed Buf 1.56.0 format/lint；两个独立完整 `buf generate` 临时树逐路径/hash一致，并与登记 Rust/Python/TypeScript生成物一致
- `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory`
- `uv run --offline --locked --project python python -m pytest python/tests/test_contract_import.py -q`
- Node `22.17.0` + pnpm `10.12.4`：contracts consumer focused test
- `cargo test --offline --locked -p ficant-domain --test fixed_decimal_rounding`
- `cargo test --offline --locked -p ficant-tax-pack` 与 `--test r5e_authoritative_pack`
- `cargo test --offline --locked -p ficant-application --test tax_rule_resolution --test r5d_rates_materialization --test r5e_tax_materialization`
- `uv run --offline --locked --project python python -m pytest tests/oracle/china-rates/test_r5e_tax_adjusted_decimal_oracle.py -q`
- `cargo test --offline --locked -p ficant-api --test rates_service`
- `cargo test --offline --locked -p ficant-server --test rates_sit`
- `cargo test --offline --locked -p ficant-native-nodes`；Worker Phase4 SIT；Python live import/construction（live server用既有环境门禁）
- AC10、AC26/Phase2C、R4d-a/R4d-b和R5D聚焦回归
- `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`
- 固定 Node PATH 后 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1`
- 静默导入既有 User级 `FICANT_TEST_*` 后 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration`
- `git diff --check`；实际 changed paths完全包含于允许路径；逐项核验全部受保护事实

## 7. 残余风险

截至规划冻结尚未实施，暂无最终候选残余风险结论。即使 R5E 完成，claim仍只覆盖D1主体的单券coupon销项VAT调整、抵扣进项前；机构最终税负、金融商品转让/交割税务、其他主体/券种、Factor/Exposure/KRD双口径、完整税务会计与UI呈现继续不在产品承诺内。
