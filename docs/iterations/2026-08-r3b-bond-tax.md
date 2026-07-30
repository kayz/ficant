# R3b 迭代 brief — Bond 发行属性与 TaxRulePack

**迭代：** R3b · **本轮目标条目：** AC08、AC09、AC10 · **execution base：** `7e28f1bdb96c7a7a3496d131be487aced40878da` · **authority commit：** `5e333be15c0ce9ec2849d6640cf6c9888b57c6a9`

本 brief 是 R3b 面向 Human 的唯一 brief。它承接已合入的 R3a Subject / Funding 契约，只处理 Bond 的发行与税收属性、TaxRulePack 和税后 Bond 输出；不得借此处理 R4 的 PositionSnapshot、历史 CTD 或合约拒绝语义。

## 1. 目标

将公开 Bond 形状从含混的单一 `issue_date` 破坏性拆分为首发日、本期发行日、累计发行量和券级税收属性；让 `AnalyzeBond` 在 native 税前计算之外，精确解析、校验并消费一个 L3 `TaxRulePack`，以同一税前 clean price 重算税后 coupon cashflow 与税后 YTM。该实现只消费 Subject 的 `TaxTreatment`、Bond 的首发日/税收属性和解析后的 pack 值；不得把税率、cutoff 或 profile 数值写入 domain、C++、FFI 或调用方裸参数。

**Acceptance sentence：**

> 使用一个仅限测试、明确标注为非权威的 TaxRulePack：首发日 `2025-08-08` 前、但本期发行日在其后的同期限 Bond 仍按首发日命中免税规则；首发日不早于该界点的同期限 Bond 命中应税规则；二者的税后 YTM 不同。对同一应税 Bond，两个仅 `TaxTreatment` profile 不同的精确 Subject 得到相同税前 cashflow / YTM、不同税后 cashflow / YTM；删除所需规则项、profile、首发区间或使 Bond 属性与 pack 规则不符时，服务在 native engine 前以指名缺项的 `RulePackItemMissing` 失败关闭；Bond 同时暴露可不同的 `first_issue_date` 与 `current_issue_date`；R1/R2/R3a 已点亮条目、C++/C ABI、Golden、Oracle、canonical schema/hash 与 Phase 2 matrix 均不变。

本句中的 `2025-08-08` 来自既有 AC09 行为判定；测试 pack 的 profile code、税率和 payload source 均是**合成机制 fixture**，不是税制来源或可发布产品规则。因此它可证明实现机制与失败关闭，不能单独构成 AC09/AC10 的权威业务批准。

## 2. 验收

| 条目 | R3b 可执行判据 |
|---|---|
| AC08 | `ficant.market.v1.Bond` 和 `ficant.rates.v1.BondTerms` 都删除并 reserve 旧 `issue_date` tag；新的 strict domain Bond / terms 测试与 codec 往返证明 `first_issue_date != current_issue_date` 合法，累计发行量为正且两日期受约束。旧 Phase 2 直接适配器只令二者相等，不能作为公开形状的旁路。 |
| AC09 | 真实 `AnalyzeBond` 服务绑定精确 Subject、TaxRulePack 与两个首发日不同的 BondTerms；pack 按首发日期间和与 Bond 相符的税收属性选中规则，返回税前与税后结果。首发日前 / 后界点、界点后续发但首发日前的 case 都有独立 Decimal 对照；缺日期区间、tax attribute 或 rate 在 engine 调用数为零时失败关闭并指名缺项。该判据是非权威合成 pack 的机制证据。 |
| AC10 | 同一应税 Bond、快照和 TaxRulePack 只切换 Subject 精确版本中的 VAT / income profile pair：税前 cashflows 与 measures 逐值相同，税后 cashflows 与 YTM 不同；TaxRulePack 的 owner、id、version、content hash、半开有效区间、market、rule type、type URL、排序、profile pair 和 Decimal unit 任一漂移都在 engine 前失败关闭。该判据同样不替代真实税制内容批准。 |

R3b 闸门：

1. 先只加入 R3b proto / 生成契约和 AC09/AC10 direct service 判据，亲眼取得非零 exit code；这个 RED 不构成 checkpoint。Tax parser、TaxRulePack composition、税后结果映射和生产组合不得早于该 RED。
2. `definition.proto.Bond.issue_date = 2` 与 `analytics.proto.BondTerms.issue_date = 1` 必须删除并 reserve。公开输入只接受完整新形状；为冻结 Phase 2 Golden 保留的 Rust 直接适配器只能令 `first_issue_date == current_issue_date`、没有可消费税收属性，且不得被 TaxRulePack 路径使用。
3. `AnalysisContext.tax_rule_pack = 8` 和 `ResultMetadata.tax_rule_pack = 7` 只由 `AnalyzeBond` 接受、精确读取、解析并进入税后现金流 / YTM。`InterpolateYieldCurve`、`AnalyzeCarryRoll`、`AnalyzeFuturesDelivery` 与 `AnalyzeFuturesHedge` 收到该 binding 必须在 engine 前失败关闭，不能把它带进 metadata 或血缘来伪装消费。
4. `TaxRulePack` 固定为 `market = CN`、`rule_type = tax` 和一个精确 type URL。它按 `[first_issue_from, first_issue_to)` 选择规则，逐项校验 Bond tax attributes 与规则相符，并按 Subject 的完整 VAT / income profile pair 选择 canonical、带 `UnitRef` 的 coupon tax rate；缺项以安全 `RulePackItemMissing { path }` 失败关闭。税后 coupon 等于 `pre_tax_coupon × (1 − parsed_coupon_tax_rate)`，随后以原税前 clean price 的 `PriceIn` 路径重跑同一 native engine；任何不可精确表示、无效 unit 或无效税率均拒绝，不静默舍入或回退。
5. TaxRulePack 的 persisted `MarketRulePack` 读取必须沿用 R2/R3a 的 exact definition、owner、id、version、content hash、有效区间、market、rule type、type URL 与 content hash verification。解析结果和被消费的精确 binding 必须写入 Bond response metadata；不得添加 TaxRulePack 到未消费 RPC 的 metadata。
6. Bond definition 的 codec 与 PostgreSQL 采用 forward-only v2 形状，并保留旧 `market.bonds.issue_date` 与 legacy codec decode 以读取历史 payload。新增迁移必须以 nullable 新列兼容旧行；`migration_acceptance` 不得只改计数，必须同时验证新列与约束。不得修改已有 Phase 2C/2D 直接 SQL fixture 或 matrix hash。
7. 分层门禁新增 Tax rate 值检查，覆盖 Subject、Bond/domain、C++ 与 FFI 禁止面；fixture 必须保留真实违规 → exit 1 和移除后 → exit 0 的场景。allowlist 仍为 `[]`，门禁入口 diff 只能扩大覆盖。
8. 不得改写 Golden、Oracle、容差、Phase 2C/2D matrix、guard hash、selector、command、路径风格、canonical schema/hash 或 C++/C ABI。若冻结清单外的文件成为必需项，先停止并取得 Human 明确扩权；不得先改后补。
9. 最终 handoff 单独呈现 `check-layering.ps1`、`test-layering-check.ps1`、`check-fast.ps1`、`check.ps1`、`check.ps1 -IncludeIntegration`、`check-phase2e-sdk.ps1`、allowlist 与 Subject/Funding/Tax parser 的 base-to-candidate diff。门禁入口只能增加覆盖或保持既有执行语义。

## 3. 非目标

- R3a 的 Subject/Funding 契约、FundingRulePack 内容和 `funding_adjusted_irr`；不得重开已合入的 R3a 设计。
- Position / PositionSnapshot、Factor、Coverage/DataHealth、Constraint / ShadowPrice、Policy、AC04、AC05、AC11–AC19、AC27、AC28、AC30–AC37。
- R4 的双时间 CTD、当日清单 / 价格拒绝、具体合约身份拒绝或连续合约拒绝。
- C++、C ABI、native 税前数值实现、Phase 2C/2D 公式、Golden、Oracle、matrix rebaseline 和 Phase 3A/3B canonical 资产。
- 任何权威税率、profile code、法律解释、业务 cutoff 来源或 `domain-packs/` TaxRulePack 内容；本轮 test fixture 不能被表述为产品政策。
- 税后 coupon adjustment 之外的税种、资本利得、会计计税、申报或结算语义。
- 修改 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、ADR、`.gitignore`、`.github/**`、`cicd.yml`、`deploy/**` 或未列入 §6 的任何路径。

## 4. 公共契约变化

- `AnalysisContext` 在 R3a 的 `subject_ref = 6`、`funding_rule_pack = 7` 后新增 `ObjectBinding tax_rule_pack = 8`；`ResultMetadata` 在 `subject_ref = 5`、`funding_rule_pack = 6` 后新增 `ObjectBinding tax_rule_pack = 7`。protobuf wire 仍可缺字段，但 gRPC `AnalyzeBond` 将 TaxRulePack 解释为必填；其他四个 RPC 将它解释为未消费的无效 binding。
- `ficant.market.v1.Bond` reserve `issue_date = 2`，新增 `first_issue_date = 5`、`current_issue_date = 6`、`cumulative_issued_amount = 7`、`BondTaxAttributes tax_attributes = 8`。`ficant.rates.v1.BondTerms` reserve `issue_date = 1`，新增同样语义的 `first_issue_date = 6`、`current_issue_date = 7`、`cumulative_issued_amount = 8`、`BondTaxAttributes tax_attributes = 9`。首发日锚定现金流 / Phase 2C eligibility；本期发行日与累计发行量是 L2 资产事实，不按当前日期猜测。
- `BondTaxAttributes` 位于 `ficant.market.v1`，含 `ValueAddedTaxStatus` 和 `IncomeTaxStatus` 两个明确的 `EXEMPT` / `TAXABLE` 枚举；公开形状拒绝 UNSPECIFIED。它描述券级 L2 属性而不含任何税率。
- 新增 typed L3 payload `ficant.market.v1.TaxRulePack`：`repeated BondCouponTaxRule coupon_rules = 1`。每项有 `first_issue_from = 1`（inclusive ISO date）、`first_issue_to = 2`（exclusive ISO date；空值仅表示无上界）、`BondTaxAttributes tax_attributes = 3` 和按完整 Subject VAT / income profile pair 排序且唯一的 `repeated SubjectCouponTaxRate rates = 4`。每个 rate 只有 canonical、带 `UnitRef` 的 `coupon_tax_rate = 3`；业务数值不属于 proto、domain 或调用方。
- `AnalyzeBondResult` 保留原税前 `cashflows` 和 `measures`，新增 `TaxAdjustedBondAnalytics after_tax = 4`。后者携带实际用于求解的税后 cashflows 与 `yield_to_maturity`；旧字段的 tag 与税前含义不变。直连 native research-node 的 pre-tax helper 不产生 `after_tax`，也拒绝夹带未消费 TaxRulePack。
- R3b 的 Tax parser port 与 adapter 只接受上述精确 envelope。缺 profile / interval / attribute / rate 以 `RulePackItemMissing` 报告稳定 `context.tax_rule_pack.content...` 路径；hash、lineage 与有效期漂移保留现有 R2/R3a error 分类，且所有错误 non-retryable，绝不泄露 payload、SQL、凭据或文件路径。

## 5. 需 Human 决策

- **已裁决并据此执行：** R3 拆为 R3a（AC06、AC07、AC29）和本 R3b（AC08、AC09、AC10）。R3a 不提前加入 `tax_rule_pack`，从而不产生 SPEC S3 所禁止的“只进血缘、不进计算”。
- **authority binding（事后建立，不追溯冻结）：** 私有 authority 已在 `kayz/ficant-authority` 的 `5e333be15c0ce9ec2849d6640cf6c9888b57c6a9` 解决 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md` 的可追溯私密版本化；`verify-authority.ps1` 已在干净 authority 工作树上验证该精确 SHA。此 SHA 是 R3b handoff 时建立的 post-execution authority binding，不是开工前 execution base 或 authority freeze，不得倒写此前的执行、测试或批准历史。
- **AC08 已获 Human 批准，仍待候选固化：** authority 中的状态为“☑ 已批准待候选”。只有本 brief 所述精确公共候选经 Human rebase merge 后，authority 的后续变更才可将其改为“●”；本轮不替代该后续动作。
- **仍待 Human、但不阻塞结构和合成机制实现：** AC09/AC10 仍缺权威 TaxRulePack 的来源引用、profile pair code、首发日区间 / cutoff 解释、券级 VAT / income tax attribute、coupon tax rate、有效区间和确定性内容 hash。缺少这些外部事实时，R3b 不得将任何合成 fixture 写入 `domain-packs/`，不得宣称 AC09/AC10 已获权威业务批准，也不得将其表述为法规实现。
- **事后流程偏差（不追认，已恢复）：** 初次使用固定 `interface/buf.gen.yaml` 直接生成时，模板的 `clean: true` 在 remote plugin 超时前删除了 53 个生成文件，其中多数不在冻结 §6 清单。它们没有进入候选、没有被覆盖或发布；我已逐个从 execution base 精确恢复。这个恢复事实不消除首次越界写入，后续生成只会在临时目录完成，核对后机械同步 §6 明列的生成输出。
- **Human 明确扩权（2026-07-29，冻结后）：** 锁定 `buf.build/grpc/python:v1.73.1` 以 `RatesAnalyticsService` 为 types filter 生成时，因 `analytics.proto` 新增对 `definition.proto` 的 import，额外产生 `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/definition_pb2_grpc.py`。Human 已只授权该精确路径写入；该文件是无 service 的三行生成 stub，未改变 Python SDK 语义。此项记录授权发生在发现之后，不回溯修改冻结 §6，也不为任何其他路径建立通配或先改后补的先例。
- **闸门 1 的事后流程偏差（不追认）：** 生成契约的首次工具故障使 direct service 判据未能在 Tax parser / production composition 之前独立落为 RED；实施前实际取得的是 proto / generated consumer 不匹配的非零编译输出，而不是闸门文字要求的 AC09/AC10 service RED。因此此轮不得将该 RED-first 闸门表述为已满足，也不把后续通过的测试倒写为先前 checkpoint。实现保持 forward-only；该记录不消除偏差，是否接受此技术候选仍由 Human 决定。
- **执行工具链已恢复、无代码改动：** 本机具备 `uv 0.7.13` 与 Node `v22.17.0`，但当前 Codex 进程继承的 PATH 缺少分隔符，未能直接解析它们。最终正式检查仅在子进程前置这两个已验证目录及 Buf 1.56.0，原始 `scripts/check.ps1` 未修改并返回 exit 0。Human 所述 Node 24 信任声明未被当作替代或改写现有严格 Node 22 门禁的依据。

## 6. 最终真实测试证据

**冻结与当前状态：** execution base 为 `7e28f1bdb96c7a7a3496d131be487aced40878da`。以下结果来自 2026-07-29 的未提交本地工作树；正式 `check.ps1` 与 `check.ps1 -IncludeIntegration` 均已在锁定 `uv 0.7.13`、Node `v22.17.0`、CPython `3.12.11` 与 Buf 1.56.0 上 exit 0。后者只把 Windows User 范围内的六项测试配置导入当前测试进程，不输出其值；它证明本轮的本地整合候选，不代替 §7 所列 Human 批准和权威内容责任。测试结束后只更新了本 brief 的 §6/§7 证据文字；可执行源码和生成输出未再改变，范围及 `git diff --check` 已重新复核。

**规定的针对性命令：**

- `cargo test --offline --locked -p ficant-domain --test bond_issuance_contracts`
- `cargo test --offline --locked -p ficant-tax-pack`
- `cargo test --offline --locked -p ficant-application --test tax_rule_resolution`
- `cargo test --offline --locked -p ficant-api --test rates_service`
- `cargo test --offline --locked -p ficant-contract-tests`
- `cargo test --offline --locked -p ficant-storage --test migration_acceptance`
- `pwsh -NoProfile -File scripts/test-layering-check.ps1`
- `pwsh -NoProfile -File scripts/check-layering.ps1`
- `pwsh -NoProfile -File scripts/check-phase2e-sdk.ps1`
- `pwsh -NoProfile -File scripts/check-fast.ps1`
- `pwsh -NoProfile -File scripts/check.ps1`
- `pwsh -NoProfile -File scripts/check.ps1 -IncludeIntegration`

**已取得的真实结果（同一可执行源码候选；随后仅更新本 brief 的证据文字）：**

- `bond_issuance_contracts` 3/3、`ficant-tax-pack` 3/3、`tax_rule_resolution` 3/3、`rates_service` 14/14 均 exit 0；后者覆盖首发日界点、两种 Subject profile、税前/税后差异、三类 `RulePackItemMissing` 和所有未消费 RPC 的 engine 前拒绝。注入的 Bond engine 计数为 AC09 两请求共 4（每请求税前 / 税后各一次）、AC10 两请求共 4；三类缺项及未消费 RPC 均为 0。
- `rates_service` 的 Decimal 手算以面值 100、年 coupon 2.5% 为例：免税 `2.5 × (1 − 0) = 2.5`，13% 为 `2.5 × (1 − 0.13) = 2.175`，25% 为 `2.5 × (1 − 0.25) = 1.875`；每笔 coupon / total 以 12 位 MidpointNearestEven 和一个 fixed-decimal tick 容差逐笔比对，税后 YTM 在不同规则 / profile case 均显式断言不同。
- 用 SHA-256 `968978abd84b39465de1a08c4d84b788502fa82565a4f718b58200f921819e51` 的 Buf 1.56.0 执行 `buf format --diff --exit-code interface`、`buf lint interface` 与 `ficant-contract-tests`，分别 exit 0，descriptor inventory 14/14；生成输出只含 §6 明列路径和已授权的 `definition_pb2_grpc.py`。
- `test-layering-check.ps1` exit 0（51 assertions）；`check-layering.ps1` exit 0：`AC03=0`、`AC01=0`、Phase 2C C++/FFI=0、R3a Funding=0、R3b Tax=0、allowlist=0。
- `cargo clippy --offline --workspace --all-targets --locked --exclude ficant-contracts --exclude ficant-contract-tests --no-deps -- -D warnings`、`cargo build --offline --workspace --all-targets --locked` 与 `check-fast.ps1` 均 exit 0。C++ 配置 / 构建 / `ctest` 也 exit 0（8/8）。
- CPython 3.12 的直接诊断下，36 条总 acceptance matrix、Phase 2B 16/16、Phase 2C 18/18、Phase 2D 18/18、两组 Oracle 各 3/3 以及 Python contracts 1 passed / 1 skipped 均 exit 0；`generate-cgb-futures-pack.ps1 -Check` 在同一 Buf 环境 exit 0。这些结果证明冻结资产未漂移，但不代替缺失的 `uv --offline --locked` 入口。
- 正式 `check.ps1` 在上述锁定工具链 exit 0：Phase 2E live Python SDK 1/1、Web typecheck / build、Vitest 5 files / 35 tests，以及前述 Matrix、Oracle、C++、Rust 和 generated-contract 步骤均由原始入口完成，不再是 Node 24 的诊断替代。
- 早期 `migration_acceptance` 的直接运行曾因当前进程缺少 `FICANT_TEST_DATABASE_URL` 在测试启动前 0/4 退出；该失败没有被掩盖或重写。调用者随后提供的 Windows User 范围测试配置仅导入当前测试进程，Docker Desktop Linux engine 与既有本地 PostgreSQL/Ceph RGW 拓扑均已就绪，`check.ps1 -IncludeIntegration` 现 exit 0：migration 4/4、Phase 4C lease queue 1/1、Phase 4 execution closure 3/3、production worker 1/1、Phase 1 business loop 1/1、negative invariants 13/13、Phase 2B/2C/2D 各 1/1、Phase 3A registry/parity 各 1/1，以及 Phase 3B codec 2/2 与 publication 1/1；原始入口最终输出 `FICANT complete local checks passed.`
- 逐项 scope audit 为 49 个变更路径、49 个授权路径（含 Human 明确扩权的一项）；`cpp/**`、Golden、Oracle、canonical schema、Phase 2C/2D matrix 与五个点名禁止 proto 均无 diff；`git diff --check` exit 0。

最终还须逐项呈现：RED-first 的原始非零 output；Tax parser / service 的 engine call count；税前 / 税后 Decimal 手算；Buf format/lint 与三侧 generated diff；legacy codec 读写与 PostgreSQL migration 结果；5 个 Phase 2C immutable facts、Phase 2D Golden/Oracle facts、Golden/Oracle 内容和 Phase 3A/3B canonical hash 均未变化；以及 `git diff --check`、allowlist `[]`、五个点名禁止 proto 和 `cpp/**` 无 diff。

**冻结允许写路径（不得在执行中就地修改）：**

- `Cargo.lock`
- `binaries/ficant-server/Cargo.toml`
- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-worker/tests/phase4_worker_sit.rs`
- `crates/ficant-api/Cargo.toml`
- `crates/ficant-api/src/rates.rs`
- `crates/ficant-api/tests/phase2e_sdk_live.rs`
- `crates/ficant-api/tests/rates_service.rs`
- `crates/ficant-application/src/ports/fingerprint.rs`
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/ports/tax_rule_parser.rs` (new)
- `crates/ficant-application/src/use_cases/mod.rs`
- `crates/ficant-application/src/use_cases/tax_rule.rs` (new)
- `crates/ficant-application/tests/tax_rule_resolution.rs` (new)
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contracts/src/generated/ficant.market.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.rates.v1.rs`
- `crates/ficant-domain/src/analytics.rs`
- `crates/ficant-domain/src/curves.rs`
- `crates/ficant-domain/src/futures_delivery.rs`
- `crates/ficant-domain/src/market/bond.rs`
- `crates/ficant-domain/src/market/mod.rs`
- `crates/ficant-domain/tests/bond_issuance_contracts.rs` (new)
- `crates/ficant-fixed-income-native/src/lib.rs`
- `crates/ficant-native-nodes/tests/cgb_bond_analytics.rs`
- `crates/ficant-storage/src/postgres/codec.rs`
- `crates/ficant-storage/src/postgres/definitions.rs`
- `crates/ficant-storage/tests/migration_acceptance.rs`
- `crates/ficant-tax-pack/Cargo.toml` (new)
- `crates/ficant-tax-pack/src/lib.rs` (new)
- `docs/iterations/2026-08-r3b-bond-tax.md` (new)
- `docs/iterations/README.md`
- `interface/README.md`
- `interface/proto/ficant/market/v1/definition.proto`
- `interface/proto/ficant/market/v1/tax_rule.proto` (new)
- `interface/proto/ficant/rates/v1/analytics.proto`
- `migrations/postgresql/0015_bond_issuance_tax.sql` (new)
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/definition_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/tax_rule_pb2.py` (new)
- `python/node-contracts/src/ficant_contracts/generated/ficant/rates/v1/analytics_pb2.py`
- `python/tests/test_contract_import.py`
- `python/tests/test_rates_sdk_live.py`
- `scripts/check-layering.ps1`
- `scripts/test-layering-check.ps1`
- `web-dm/packages/contracts-generated/src/ficant/market/v1/definition_pb.ts`
- `web-dm/packages/contracts-generated/src/ficant/market/v1/tax_rule_pb.ts` (new)
- `web-dm/packages/contracts-generated/src/ficant/rates/v1/analytics_pb.ts`
- `web-dm/platform-shell/tests/contracts-consumer.test.ts`

**禁止写路径：** 所有未逐项列出的路径，特别是 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、`docs/architecture/adr/**`、`.gitignore`、`.github/**`、`cicd.yml`、`deploy/**`、`cpp/**`、`tests/golden-cases/**`、`tests/oracle/**`、`tests/phase2c/acceptance-matrix.json`、`tests/phase2d/acceptance-matrix.json`、`crates/ficant-data/src/canonical.rs`、以及 `position.proto`、`factor.proto`、`health.proto`、`constraint.proto`、`policy.proto`。

## 7. 残余风险

- TaxRulePack 的合成 fixture 只能证明 parser、精确 binding、fail-closed 与税后计算链路；它不提供或暗示可用的税制、税率或机构 profile。权威内容仍是 Human 的 L3 输入。
- R3b 的税后层仅对 coupon cashflow 应用已解析的 coupon tax rate；资本利得、会计计税、申报与其他税种不在本轮范围，不能由“税后 YTM”这一名称掩盖。
- Bond 是 pre-1.0 的破坏性形状变化。新 codec / PostgreSQL 迁移保留 legacy read 兼容，但历史 legacy 记录没有可消费 tax attributes，因而不能静默进入税后路径。
- Tax binding 只在 `AnalyzeBond` 进入计算。四个未消费 RPC 的显式拒绝是防止 S3 假血缘的约束，不代表其永远不需要税收语义；将来真正消费时必须另轮建立计算和验收。
- Subject / TaxRulePack metadata 还不是 AC30 所要求的完整持久化 Artifact lineage 或缓存键；不得据此提前声称证据链闭合。
- 三件套现由私有 authority 的 `5e333be15c0ce9ec2849d6640cf6c9888b57c6a9` 提供可追溯历史；这个 post-execution binding 不重写本轮冻结历史。AC08 已获 Human 批准、在精确公共候选经 Human rebase merge 前保持“☑ 已批准待候选”；AC09/AC10 仍缺权威税制内容，不能批准或表述为法规实现。
- PostgreSQL/Ceph RGW 整合已在一次性本地拓扑中由 `check.ps1 -IncludeIntegration` 复跑为 exit 0；这只是本地自测证据，不构成部署、远端 CI 或持续环境健康结论。
