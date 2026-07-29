# R3a 迭代 brief — Subject 绑定与资金规则

**迭代：** R3a · **本轮点亮：** AC06、AC07、AC29 · **execution base：** `af733ac6eb8ea88c9b4c16d775e98790b2e98897`

本 brief 是 R3a 面向 Human 的唯一 brief。R3b 另起独立 brief，只处理 Bond 发行属性、TaxRulePack 与 AC08–AC10；本轮不得预先写入其字段、pack 或业务数值。

## 1. 目标

将五个 `RatesAnalyticsService` RPC 的 Subject 从 R1 的可携带引用收紧为必填、精确读取、失败关闭的计算身份；仅让 `AnalyzeFuturesDelivery` 解析并使用 FundingRulePack 的资金档利率，移除调用方裸传的融资利率，并把精确 Subject 与实际使用的 FundingRulePack 写入响应元数据。

**Acceptance sentence：**

> 对同一交割篮子、快照和 CGB futures RulePack，分别绑定 `DrAvailable` 与 `ROnly` 的两个精确 Subject 后，服务从同一个非权威合成 FundingRulePack 解析不同年化资金成本，返回不同的 `funding_adjusted_irr`，并且 `financing_cost` / `holding_carry` 的差额等于资金成本差按实际持有期的 Decimal 手算；五个 Rates RPC 缺少、找不到或精确版本不符的 Subject 均在 native engine 前以指向 `context.subject_ref` 的 non-retryable `VALIDATION_FAILED` 失败关闭；给定具备所需市场与工具准入的 Subject，期货套保手数仍与公开手算公式一致，而缺准入 Subject 失败关闭；R1/R2 已点亮的 AC01、AC02、AC03、AC26、Golden、Oracle、canonical schema hash 和 Phase 2 matrix 均不变。

`implied_repo_rate` 保持其既有含义：市场隐含、未扣主体融资成本的年化回购率。它在 R3a 不被伪称为主体 IRR。新增的 `funding_adjusted_irr` 明确等于 `implied_repo_rate - annual_financing_rate`；`financing_cost` 与 `holding_carry` 保留为按实际持有期线性复核的金额结果。

## 2. 验收

| 条目 | R3a 可执行判据 |
|---|---|
| AC06 | 两个只在 Subject `FundingTier` 不同的精确 Subject 绑定同一交割请求；真实 FundingRulePack parser 选中不同条目，`funding_adjusted_irr` 不同，且融资成本和 carry 差额与独立 Decimal 手算一致。此为**机制性绿灯：非权威合成 fixture**，不代表产品资金曲线。 |
| AC07 | 五个 Rates RPC 的缺失 Subject、repository 未找到和返回 `id + version` 不符均在 engine 调用数为零时返回 non-retryable `VALIDATION_FAILED`，唯一 field violation 为 `context.subject_ref`。 |
| AC29 | `AnalyzeFuturesHedge` 先精确解析 Subject，再要求其准入集包含由 CGB delivery parser 声明的市场与稳定工具码 `futures-hedge`；获准请求的套保手数与手算一致，任一准入缺失时 engine 调用数为零并失败关闭。 |

R3a 闸门：

1. 先只加入 R3a proto 字段、生成契约和 AC07 缺 Subject + spy-engine 判据，亲眼取得非零 exit code；此 RED 不构成 checkpoint。Subject resolver、Funding parser、生产组合均不得早于该 RED 出现。
2. `AnalysisContext.subject_ref = 6` 是五个 RPC 唯一 Subject 入口；删除并 reserve `AnalyzeBondRequest.subject_ref = 10`。所有 RPC 必须精确读取 `id + version` 相同的 Subject 后才可调用 engine。
3. `AnalysisContext.funding_rule_pack = 7` 仅由 `AnalyzeFuturesDelivery` 接受、精确读取、解析并进入计算。其他四个 RPC 如收到该 binding 必须失败关闭，不能把它只带入元数据或血缘。`AnalyzeCarryRoll` 保持 `unfunded`，只绑定 Subject，不要求伪造 FundingRulePack。
4. 删除并 reserve `AnalyzeFuturesDeliveryRequest.financing_rate = 9`；不得保留调用方裸融资利率、默认资金成本或 Subject 内嵌数值。Funding pack 的 owner、id、version、content hash、半开有效区间、market、rule type、type URL、Decimal unit 与 FundingTier 必须在 engine 前逐项校验。
5. `funding_adjusted_irr = implied_repo_rate - annual_financing_rate` 是 API 层的确定性主体调整；native C++/ABI、原始 `implied_repo_rate`、Phase 2C Golden/Oracle 与原有结果字段不改变。
6. 分层门禁新增 Funding rate 数值检查，覆盖 Subject、SubjectState、domain 与 C++/FFI 禁止面；保留真实负向 fixture，allowlist 仍为 `[]`，且门禁改动只能扩大覆盖。
7. 不得改写 Phase 2C/2D matrix、guard hash、Golden、Oracle、expected、容差、selector、command、路径风格或 Phase 3A/3B canonical schema/hash。若冻结写路径外的文件成为必需项，停止并取得 Human 扩权；不得先改后补。

## 3. 非目标

- R3b 的 `tax_rule_pack`、TaxRulePack parser、Bond 首发/本期发行日和累计发行量拆分、券级税收属性、税后现金流与 AC08–AC10。
- SubjectStateSnapshot 的额度、约束、资本占用、ShadowPrice、Position、Factor、Coverage/DataHealth、Policy、AC27、AC28、AC30–AC37。
- FundingRulePack 的权威业务数值、产品资金曲线、外部来源或 `domain-packs/` 内容；本轮 fixture 是测试专用且非权威。
- C++、C ABI、native pre-tax 数值、Phase 2C/2D 公式、持久化 Artifact schema、Oracle、Golden 和 matrix rebaseline。
- 修改 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、ADR、`.gitignore`、`.github/**`、`cicd.yml`、`deploy/**`，或任何未列入 §6 的文件。

## 4. 公共契约变化

- `AnalysisContext` 新增 `ficant.core.v1.VersionRef subject_ref = 6` 与 `ObjectBinding funding_rule_pack = 7`。protobuf wire 允许缺字段，但 API 在五个 RPC 上将 Subject 解释为必填；Funding binding 只在交割 RPC 上必填。
- `ResultMetadata` 保留 `subject_ref = 5`，新增 `funding_rule_pack = 6`。所有成功 Rates 响应设置精确 Subject；只有交割响应设置 FundingRulePack，因为只有它消耗该 pack。
- `AnalyzeBondRequest.subject_ref = 10` 和 `AnalyzeFuturesDeliveryRequest.financing_rate = 9` 删除并 reserve，消除双入口和裸利率旁路。
- 新增 L3 payload `ficant.market.v1.FundingRulePack`：按 `ficant.core.v1.FundingTier` 排序且唯一的条目，每条含 canonical、带 `UnitRef` 的 `annual_financing_rate`。`ficant-funding-pack` adapter 只接收精确 `market = CN`、`rule_type = funding` 与固定 type URL；所需 tier 或 rate 缺失返回 `RulePackItemMissing { path }`。
- `FuturesDeliveryMeasures` 新增加法字段 `funding_adjusted_irr = 15`。旧 `implied_repo_rate` 保持 pre-funding 市场量，旧 wire tag 与含义不变。
- Subject resolution 的缺失、未找到、返回引用不符或准入不足统一映射为 `VALIDATION_FAILED`、non-retryable，且只暴露稳定 field `context.subject_ref`；不得泄露存储、payload 或凭据细节。

## 5. 需 Human 决策

- **已裁决并据此执行：** R3 拆为 R3a（AC06、AC07、AC29）与 R3b（AC08、AC09、AC10）。R3a 不添加 `tax_rule_pack`，避免把尚未解析的规则只放进血缘而违反 SPEC S3。
- **已作实现性澄清：** 现有 `implied_repo_rate` 不读取融资成本，不能作为“主体 IRR 不同”的判据。R3a 因此以加法字段 `funding_adjusted_irr` 表达净融资年化收益，同时保留原始市场 IRR 和可按持有期手算的金额差；这不引入任何资金数值或改变 native 公式。
- **事后范围偏差（不追认，已从候选移除）：** 实施期间曾为导出 `ResolveFundingRule` 与 `ResolveSubject` 写入 `crates/ficant-application/src/lib.rs`，该路径不在冻结 §6。范围审计发现后，API 改为直接引用已公开的 `use_cases` 模块，且该文件的全部 diff 已移除；这不需要、也未获得扩权，移除结果不消除曾发生未授权写入的事实。
- **仍待 Human、但不阻塞 R3a 编码：** `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md` 的私密、有历史版本化方案。其未决状态阻塞 AC06/AC07/AC29 的可追溯 Human 批准与 MANUAL 实然确认；它不授权 Agent 修改三件套，也不把本地副本当作 Git 证据。

## 6. 最终真实测试证据

**候选：** execution base `af733ac6eb8ea88c9b4c16d775e98790b2e98897` 上的本地工作树；填写本节前已完成所有代码改动。测试进程使用 Node `v22.17.0`、uv `0.7.13`、CPython `3.12.11` 与 Buf `1.56.0`。经 Human 授权的一次 `uv sync --locked --project python` 仅预热本机依赖缓存；以下 Python 检查均以 `--offline --locked` 执行。

**RED-first（非 checkpoint）：** 在 Subject resolver、Funding parser 与生产组合出现前，`cargo test --offline --locked -p ficant-api --test rates_service ac07_missing_subject_is_rejected_before_delivery_engine` 以 exit `101` 失败，原始断言为“an unbound Subject must fail closed before the delivery engine”。随后才实施 resolver 和组合。

**最终命令与结果：**

- `cargo test --offline --locked -p ficant-api --test rates_service` → exit `0`，10 passed。
- `cargo test --offline --locked -p ficant-application --test funding_rule_resolution` → exit `0`，3 passed。
- `cargo test --offline --locked -p ficant-funding-pack` → exit `0`，1 unit passed、0 doc tests。
- `cargo test --offline --locked -p ficant-contract-tests`（设置固定 `FICANT_BUF`）→ exit `0`，14 passed。
- `pwsh -NoProfile -File scripts/test-layering-check.ps1` → exit `0`，43 assertions passed；`pwsh -NoProfile -File scripts/check-layering.ps1` → exit `0`，`AC03=0`、`AC01=0`、Phase 2C C++/FFI rule values `=0`、R3a Funding rule values `=0`、allowlist `=0`。
- `pwsh -NoProfile -File scripts/check-phase2e-sdk.ps1` → exit `0`，live Python SDK parity 1 passed。
- `pwsh -NoProfile -File scripts/check-fast.ps1` → exit `0`，`FICANT fast local checks passed.`
- `pwsh -NoProfile -File scripts/check.ps1` → exit `0`，`FICANT complete local checks passed.`；其中 C++ 8/8、Q-001..Q-036 36 mapped、Phase 2B 16/16、Phase 2C/2D 各 18/18、两组独立 Oracle 各 3 passed、Python contract 1 passed/1 skipped、Phase 2E 1 passed、canonical 5、snapshot codec 2、Web 5 files/35 tests 均通过。

**最终范围与不变量审计：** 36 个变更路径均在冻结允许写路径内；`HEAD == origin/main ==` execution base；`cpp/**`、Golden、Oracle、Phase 2C/2D matrix、canonical schema 与五个点名禁止 proto 均无 diff；`git diff --check` 无空白错误。§5 的事后范围偏差记录仍保留，且其越界 diff 已不在候选中。

**冻结允许写路径（不得在执行中就地修改）：**

- `Cargo.lock`
- `binaries/ficant-server/Cargo.toml`
- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-worker/tests/phase4_worker_sit.rs`
- `crates/ficant-api/Cargo.toml`
- `crates/ficant-api/src/core_error.rs`
- `crates/ficant-api/src/rates.rs`
- `crates/ficant-api/tests/phase2e_sdk_live.rs`
- `crates/ficant-api/tests/rates_service.rs`
- `crates/ficant-application/src/error.rs`
- `crates/ficant-application/src/ports/funding_rule_parser.rs` (new)
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/use_cases/funding_rule.rs` (new)
- `crates/ficant-application/src/use_cases/mod.rs`
- `crates/ficant-application/src/use_cases/subject_resolution.rs` (new)
- `crates/ficant-application/tests/funding_rule_resolution.rs` (new)
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contracts/src/generated/ficant.market.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.rates.v1.rs`
- `crates/ficant-funding-pack/Cargo.toml` (new)
- `crates/ficant-funding-pack/src/lib.rs` (new)
- `crates/ficant-native-nodes/tests/cgb_bond_analytics.rs`
- `docs/iterations/2026-08-r3a-subject-funding.md`
- `docs/iterations/README.md`
- `interface/README.md`
- `interface/proto/ficant/market/v1/funding_rule.proto` (new)
- `interface/proto/ficant/rates/v1/analytics.proto`
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/funding_rule_pb2.py` (new)
- `python/node-contracts/src/ficant_contracts/generated/ficant/rates/v1/analytics_pb2.py`
- `python/tests/test_contract_import.py`
- `python/tests/test_rates_sdk_live.py`
- `scripts/check-layering.ps1`
- `scripts/test-layering-check.ps1`
- `web-dm/packages/contracts-generated/src/ficant/market/v1/funding_rule_pb.ts` (new)
- `web-dm/packages/contracts-generated/src/ficant/rates/v1/analytics_pb.ts`
- `web-dm/platform-shell/tests/contracts-consumer.test.ts`

**规定的针对性命令：**

- `cargo test --offline --locked -p ficant-api --test rates_service`
- `cargo test --offline --locked -p ficant-application --test funding_rule_resolution`
- `cargo test --offline --locked -p ficant-funding-pack`
- `cargo test --offline --locked -p ficant-contract-tests`
- `pwsh -NoProfile -File scripts/test-layering-check.ps1`
- `pwsh -NoProfile -File scripts/check-layering.ps1`
- `pwsh -NoProfile -File scripts/check-phase2e-sdk.ps1`
- `pwsh -NoProfile -File scripts/check-fast.ps1`
- `pwsh -NoProfile -File scripts/check.ps1`

**禁止写路径：** 所有未在上列逐项列出的文件，特别是 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、`docs/architecture/adr/**`、`.gitignore`、`.github/**`、`cicd.yml`、`deploy/**`、`cpp/**`、`tests/golden-cases/**`、`tests/oracle/**`、`tests/phase2c/acceptance-matrix.json`、`tests/phase2d/acceptance-matrix.json`、`crates/ficant-data/src/canonical.rs`、以及五个点名禁止的 proto。

## 7. 残余风险

- AC06 的 fixture 只证明机制：它不提供、暗示或替代真实的机构 Funding 曲线。权威数值与来源仍是 Human 的 L3 输入。
- 交割外四个 RPC 目前不消费资金成本；强制它们携带 FundingRulePack 会制造 SPEC S3 所禁止的“只进血缘、不进计算”。R3a 以拒绝该多余 binding 保持契约诚实。
- AnalyticsService 的响应元数据现在携带 Subject / 实际 Funding pack；完整持久化 Artifact 血缘与缓存键的终态属于 AC30/R7，不能因 R3a 的 metadata 绑定而提前宣称完成。
- R3b 仍将引入第二次 pre-1.0 破坏性变化（TaxRulePack 和 Bond 形状），并且必须先由 Human 提供可引用的税制来源与数值。
- 三件套尚未有可追溯私密历史；在此解决前，本轮只能形成技术自测候选，不能声称 AC 已获可复核批准或 MANUAL 已完成实然确认。
