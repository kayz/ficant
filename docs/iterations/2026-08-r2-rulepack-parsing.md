# R2 迭代 brief — 规则包必须被解析

> 本文是本迭代面向 Human 的唯一文档。Agent 交流、失败诊断、子循环 checkpoint 与命令原始输出保留在编排工具中，不另建状态页、子任务 brief 或治理 checklist。

**迭代** R2 · **状态** 本地自测候选完成 · **依据** SPEC v1.1、ACCEPTANCE v0.1、[`../architecture/layering-refactor.md`](../architecture/layering-refactor.md)、[ADR-0013](../architecture/adr/0013-layering-law-shape-in-core-content-in-rulepack.md)

**执行 Base commit SHA：** `29a941c1236dd17e93d7ae4f0745d82b071a4b47`（Root 于 2026-07-28 执行 `git fetch origin main` 后确认工作区干净且 `HEAD == origin/main`）。当前设计锚点为 R1 候选 `625acf4536a4c8c8ac183e2cb6692e825adc2f21`，**不是**R2 execution base。

---

## 1. 目标

**让 `cgb-futures` RulePack 的内容真正进入国债期货交割计算，并删除最后一条分层 allowlist。**

R2 只修复 Phase 2C 交割链路的规则输入：规则包从“只有引用与哈希”变成“内容可持久化、可校验、可解析、可注入数值内核”。L2 只保留规则形状，任何 CFFEX 交割规则值均不得留作内置默认值。

R2 同时是 R3 的物理前置，而不只是路线图中的较高优先级：R1 分层门禁已经在 `ficant-domain` 扫描 `tax-rate`，且 allowlist 被锁为只能删除。若 R2 不先建立可复用的 RulePack 内容承载、精确读取和 L3 解析边界，R3 的 TaxRulePack 就只能把税率硬编码回 domain，并被门禁结构性阻断。R2 不实现税收，只交付以后可由 TaxRulePack 复用的通用内容与解析接线。

**Acceptance sentence（一句可验证）：**

> 在完整本地拓扑中，将同一笔 `AnalyzeFuturesDelivery` 分别绑定两个仅版本、规则数值及由内容导出的 hash 不同的 `cgb-futures` RulePack 后，系统从持久化内容解析规则，且转换因子、基差、净基差、IRR 或 CTD 至少一项不同；删除任一必需规则项时以内部 `RulePackItemMissing` 失败关闭，在进入数值引擎前通过 `ErrorDetail.field_violations` 指明准确缺失路径；冻结 `cgb-futures` v1 pack 仍返回完整可交割结果并逐项复现既有 Phase 2C expected，且分层门禁报告 `AC01=0`、`AC03=0`、allowlist `=0`。

设计依赖方向固定为：

```text
interface typed schema
  → persisted opaque MarketRulePack content
  → L3 cgb-futures parser
  → L2 FuturesDeliveryRule shape
  → provider-neutral Rust/C ABI calculation
  → API result and reproducible lineage
```

---

## 2. 验收

**本轮点亮：**

| 条目 | 本轮机械判据 |
|---|---|
| **AC01** | `check-layering.ps1` 同时报告 domain rule values `=0`、Phase 2C production C++/FFI rule values `=0`、allowlist `=0`。C++/FFI 扫描必须机械覆盖产品期限/剩余期限表、交割月份表、名义票息、百元面值基准、转换因子/应计舍入位数与年化日基准；fixture 为真实 C++ 违规逐项证明 exit `1` |
| **AC02** | 两个精确版本仅版本、规则数值及由内容导出的 hash 不同，同一计算输入在切换精确 RulePack 绑定后，转换因子、基差、净基差、IRR 或 CTD 至少一项不同；删除 `products[product_code=…]` 或其任一必需字段，返回 non-retryable `ValidationFailed`，`field_violations[0].field` 为稳定的准确路径，且 spy engine 调用数为零 |
| **AC26** | 给定精确 Snapshot、RulePack 与具体合约，返回可交割候选、转换因子、基差、净基差、IRR 与 CTD；冻结 v1 pack 对全部 Phase 2C Golden Case 的结果逐项不变 |

**本轮明确不点亮：** AC27、AC28。它们是“当前清单回算历史”和“连续合约作为交割基础”的请求拒绝语义，需要新增时点事实或解析具体合约身份；既不是 RulePack 内容被解析的必要条件，也不是 R3 能复用 RulePack 管线的前置。本轮不借 AC02/AC26 顺手改变这两个入口面。

**本轮专属闸门：**

1. 子循环 1 必须先只加入 AC02 对照/缺项测试和夹具，在 parser 生产代码出现前运行并取得真实 RED；命令、非零 exit code 与实际失败原因保留为判据证据。RED 状态不是 checkpoint，同一用例转绿后才允许形成第一个 checkpoint。
2. `MarketRulePack.content_hash` 对存在的 typed content 按确定性 Protobuf payload bytes 做 SHA-256 校验；hash 不符时不得解析。
3. `content.type_url`、`rule_type` 与 `cgb-futures` schema 必须一致；未知 schema 不猜测、不降级、不使用默认 pack。
4. 所有进入计算的规则项都必须出现在 `FuturesDeliveryRule` 与输入 fingerprint 中；只进血缘、不进计算仍算失败。
5. `check-layering.ps1` 必须增加独立于 `Get-DomainRuleViolations` 且没有 allowlist 的 production C++/FFI 规则值检查。当前通用 source inventory 已覆盖仓库根，但 AC01 规则检查仍只遍历 domain；仅扩大 source inventory 不算完成。扫描范围至少包含 `cpp/fixed-income-kernel/src/futures_*`、交割 C ABI header、`ficant-kernel-sys` 与 native delivery adapter，并为 eligibility bounds、delivery months、standard coupon、face quote basis、rounding scale、annual day basis 产生稳定分类。
6. `test-layering-check.ps1` 必须以真实 C++ eligibility table/规则数值 fixture 证明新检查 exit `1`，移除违规后 exit `0`；最终还必须保留 domain 规则值、C++/FFI 规则值、市场分支与非空 allowlist 各至少一个“真实违规 → exit `1`”场景，不得通过删负向 fixture 制造通过。
7. `tests/phase2c/acceptance-matrix.json` 必须升级 schema，并把自管的 `guarded_files` 拆成互斥、不可漏项的 `immutable_facts` 与 `rebaselined_tests`：
   - **`immutable_facts`：** `tests/golden-cases/china-rates/phase2c-futures-delivery-inputs.json`、`tests/golden-cases/china-rates/expected/phase2c-futures-delivery-v1-expected.json`、`tests/golden-cases/china-rates/phase2c-cffex-source-manifest.json`、`tests/oracle/china-rates/phase2c_manual_oracle.py`、`tests/oracle/china-rates/test_phase2c_manual_oracle.py`；内容或 hash 任一变化立即失败，且不得移动到另一集合。
   - **`rebaselined_tests`：** `crates/ficant-domain/tests/futures_delivery_contracts.rs`、`crates/ficant-fixed-income-native/tests/futures_delivery_acceptance.rs`、`crates/ficant-storage/tests/futures_delivery_arrow.rs`、`crates/ficant-storage/tests/futures_delivery_sit.rs`、`cpp/fixed-income-kernel/tests/test_futures_delivery.cpp`、`cpp/fixed-income-kernel/tests/test_constants_and_layout.cpp`；允许因 R2 契约变化而修改。其中 `test_constants_and_layout.cpp` 同时受 Phase 2C 与 Phase 2D matrix 守卫，两个 matrix 必须登记相同的新 hash。
8. 预期重取证 tests 的最终 hash 只能在所有测试文件定稿后，由一个 diff 仅含 `tests/phase2c/acceptance-matrix.json` 与 `tests/phase2d/acceptance-matrix.json` 的独立提交更新；Phase 2D matrix 只准更新 `cpp/fixed-income-kernel/tests/test_constants_and_layout.cpp` 的 hash，不得修改 schema、base、其他 guarded 条目、P2D acceptance 条目、selector、command 或路径风格。该提交必须在 Human handoff 中单独呈现；绝对不可变 facts 不得进入该 rebaseline。
9. Phase 3A/3B canonical schema 与 schema hash 保持不变。
10. 不得新增 `position.proto`、`factor.proto`、`health.proto`、`constraint.proto` 或 `policy.proto`。
11. `scripts/layering-allowlist.json` 只能删除现有 R2 条目，最终必须为 `[]`；任何新增或替换条目立即失败。
12. 因本轮允许修改分层门禁，最终 handoff 必须单独呈现 base-to-candidate 的 `check-layering.ps1`、`test-layering-check.ps1` 与 allowlist diff；脚本变化只允许扩展覆盖、删除已兑现的唯一 allowlist 特例和维护相应 fixture，不得缩小扫描根、扩展排除项或弱化失败条件。

---

## 3. 非目标

本轮不得做以下工作：

- 不实现 Subject 的资金成本、税收待遇或约束语义，不点亮 AC06–AC10、AC29（R3）。
- 不迁移 Phase 2D 套保手数中的合约面值或 Subject 绑定，不修改 `futures_hedge` 语义；R2 只处理 Phase 2C 交割计算使用的规则。
- 不定义 Position、Factor、Coverage/DataHealth、Constraint、ShadowPrice 或 Policy 契约。
- 不拆 `Bond.issue_date`，不增加首发日、续发日、累计发行量或券级税收属性（R3）。
- 不改 canonical quote schema、价格来源类型、可信度或覆盖度（R5）。
- 不实现管理员/研究用户角色分离、数据源白名单或 Domain Pack 管理界面（R6）。
- 不实现外部中金所规则下载、自动更新、实时价格下载或自动生成完整交割篮子。
- 不引入通用表达式语言、脚本执行器或图灵完备的“规则引擎”；本轮只解析一个强类型的 `cgb-futures` 内容 schema。
- 不点亮 AC27、AC28，不新增篮子/期货价/现券价的 `observed_at` 请求字段，不实现“当前清单回算历史”或“连续合约作为交割基础”的入口拒绝语义。
- 不为 R2 精确读取完整 DataSnapshot 或 FuturesContract 实体；沿用既有绑定与血缘校验，只把精确 RulePack 从“携带”推进到“读取、解析并参与计算”。
- 不新增定价原语，不改变 Phase 2A/2B/2D/2E 的业务语义。
- 不修改 SPEC.md、ACCEPTANCE.md 或任何 ADR；若实现证据表明必须改变应然语义，只在本文 §5 提 diff 并停止。
- 不触碰 `.github/**`、`cicd.yml`、`deploy/**`，不创建版本 tag，不运行发布、部署或目标环境操作。

**不得修改的冻结事实：** Phase 2C Golden inputs/expected、独立 Decimal Oracle、CFFEX source manifest 及其 normalized facts hash、全部既有容差、Phase 3A/3B canonical schema hash。冻结 v1 pack 必须由这些事实派生，而不是反向修改事实适配实现。

---

## 4. 公共契约变化

### 4.1 通用 MarketRulePack 内容

在既有 `ficant.market.v1.MarketRulePack` 上加法式增加：

```proto
google.protobuf.Any content = 11;
```

- `content.type_url` 是内容 schema 的身份；R2 唯一接受 `type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack`，且 `market = "CFFEX"`、`rule_type = "cgb-futures"`。`content.value` 是确定性 Protobuf bytes。
- 对带内容的 pack，`content_hash = SHA-256(content.value)`；读取、持久化、fingerprint 与返回时均复核。
- 既有无内联内容的历史 RulePack 仍可读取，但不得用于声明需要 `cgb-futures` 内容的交割计算。
- PostgreSQL 表结构不变；内容随已有 definition payload 持久化。storage codec 必须同时可读旧 payload 与新 payload。

新增 `interface/proto/ficant/market/v1/cgb_futures_rule.proto`，只定义 L3 内容：

```text
CgbFuturesDeliveryRulePack
  - products[]:
      product_code
      original_term_max_months
      residual_min_months
      residual_max_months | unbounded
  - delivery_months[]
  - nominal_coupon
  - face_quote_basis
  - accrued_interest_day_count
  - conversion_factor_rounding_places
  - accrued_interest_rounding_places
  - annual_day_basis
```

Decimal 字段复用唯一的 `ficant.core.v1.DecimalValue`，不得引入 `double` 或第二套 Decimal。products 必须按 `product_code` 严格排序且无重复，delivery months 同样严格递增且无重复；所有必需 scalar 使用可判定 presence 的字段形态，确保“缺失”与合法零值不混淆。

首个可读内容放在 `domain-packs/cgb-futures/`，采用 Protobuf JSON Mapping；其确定性二进制 payload 由固定生成入口机械产生并做防漂移校验。新增市场只增加新的 L3 pack/schema/parser，不给 L0/L1/L2 加市场分支。

### 4.2 交割分析请求

R2 不修改 `ficant.rates.v1.AnalyzeFuturesDeliveryRequest`。既有 `CgbFuturesProduct product` 继续作为强类型选择器，L3 parser 用其稳定 code 精确选择 `products[product_code=…]`；`UNSPECIFIED` 继续按既有输入校验失败。`CgbFuturesProduct::code()` 属于身份形状，可以保留；`original_term_months()` 与 `residual_term_bounds()` 等规则数值查表必须删除。

本轮不增加 `product_code` 字符串替代字段，也不增加任何 `observed_at` 字段。这样既避免两个并行选择器产生冲突，也不以 RulePack 解析为名提前实现 AC27/AC28。

### 4.3 内部形状与执行边界

- `ficant-domain` 新增 provider-neutral `FuturesDeliveryRule` 形状；`FuturesDeliverableInput` 接收解析后的规则，不再由产品 enum 查表。
- L3 parser 放在独立 `ficant-cgb-futures-pack` adapter crate。它实现 application 定义的 provider-neutral parser port；domain/application 不反向依赖该市场 crate，由 `ficant-server` 负责生产组合。
- Application 在任何 engine 调用或可变 I/O 前，通过既有 DefinitionRepository 精确读取 RulePack，复核 tenant/owner/id/version/content hash、RulePack 半开有效区间、`market`、`rule_type` 与 type URL，再调用 parser port。DataSnapshot 与 FuturesContract 维持现有绑定处理，不在 R2 扩展成实体解析。
- Rust safe adapter 与交割专用 C ABI 显式传递已解析的期限边界、交割月份定义、名义票息、面值基准、舍入位数和年化日基准。C++ 只实现公式形状和数值防御，不保存 CFFEX 默认表。
- 所有解析值进入 `FuturesDeliverableInput::fingerprint`；Artifact lineage 继续绑定精确 RulePack id/version/content hash。

### 4.4 缺项错误

不新增公共错误 enum。Application error 增加内部命名 detail `RulePackItemMissing { path }`，只携带安全、受限的字段路径；`CoreBusinessErrorMapper` 复用既有 `ErrorDetail.field_violations`：

```text
code              = VALIDATION_FAILED
retryable         = false
field             = context.rule_pack.content.products[product_code=T].residual_min_months
description       = 规则包缺少计算所需项
```

错误不得包含原始 payload、SQL、凭据、文件路径或堆栈。类型不符、hash 漂移、版本/owner 漂移分别维持既有 Validation/HashMismatch/LineageIncomplete 分类，不伪装成缺项。

> Human 已批准 pre-1.0 期间可做破坏性契约变更，但 R2 无需动用该授权：`MarketRulePack.content` 是加法字段，新的 typed schema 是加法文件，既有 Rates 请求保持不变。三侧生成契约与真实 gRPC 仍必须证明新增内容可往返。

---

## 5. 需 Human 决策

### 已有权威裁决，本轮直接遵守

- core 只定义形状、RulePack 定义内容；规则引用而未解析是缺陷（SPEC S1/S3、ADR-0013）。
- 缺项必须失败关闭，禁止内置默认值（SPEC S3、ADR-0013）。
- R2 删除 `futures_delivery` 唯一 allowlist 并重跑 Phase 2C 取证（分层重构路线）。
- Human 已批准 R2–R5 可做 pre-1.0 破坏性契约变更；不得以此授权修改 Golden、Oracle 或应然验收。
- Human 已裁定 `observed_at` 与 `visible_at` 都是精确到时间的 instant，不是日期；R2 不新增这些字段，该语义留给后续 AC27/数据时点工作遵守。

### 本 brief 的范围裁决

[`../architecture/layering-refactor.md`](../architecture/layering-refactor.md) 的“缺口 1”方案段明确写 R2 点亮 AC01、AC02、AC26，而迭代序列表额外列出 AC27、AC28。R2 采用更贴近缺口定义且更窄的 AC01/AC02/AC26：它们共同证明“规则内容被解析并进入计算”。AC27/AC28 需要另一组时点与合约入口拒绝语义，会同时改变数据形状、实体解析和请求校验，因此退出本轮。

建议把 AC27/AC28 交给后续入口语义工作，并最迟在 R7 全量重取证前点亮；其精确归属应由 Human 在后续控制面归一化。R2 brief 只明确“不点亮、不声称覆盖”，不擅自修改路线文档。

### 本轮已冻结设计

1. **内容承载采用 `google.protobuf.Any` + 强类型 `CgbFuturesDeliveryRulePack`，并首次创建已规划且已被仓库策略允许的 `domain-packs/cgb-futures/`。**
2. **R2 只点亮 AC01/AC02/AC26；AC27/AC28 不进入本轮，后续归属在控制面另行归一化。**
3. **Phase 2C delivery 保留既有 `CgbFuturesProduct` 请求选择器，只删除 domain/C++ 中由它触发的规则数值查表；Phase 2D hedge 暂不改。**

Human 已审定上述三项及 §2 的门禁/重取证保护，本轮不再有待决语义。实现证据若迫使改变任一项，Root 必须先停下并在本节提出 diff，不得在代码中自行细化。

### 执行冻结前置条件

控制面提交、推送并恢复干净后，Root Orchestrator 必须重新执行：

```powershell
git status --short
git rev-parse HEAD
git rev-parse origin/main
```

仅当工作区干净且 `HEAD == origin/main` 时，把当时 `git rev-parse HEAD` 写为本轮唯一 base；任一条件不满足即停止，不开始子循环 1。

**当前无 SPEC、ACCEPTANCE 或 ADR diff 提案。**

---

## 6. 最终真实测试证据

**执行结果（2026-07-28；base `29a941c1236dd17e93d7ae4f0745d82b071a4b47`）：** Acceptance sentence、AC01、AC02、AC03 与 AC26 的业务/结构判据均已取得真实证据；经校验的 Node `v22.17.0`、uv `0.7.13 (62ed17b23 2025-06-12)`、pnpm `10.12.4` 与 Buf `1.56.0` 工具链上，`scripts/check.ps1` 和 `scripts/check.ps1 -IncludeIntegration` 均返回 exit `0`。这是本轮本地自测候选。

- AC02 RED-first：在任何 parser 生产代码出现前，`cargo test --offline --locked -p ficant-application --test futures_delivery_rule_resolution ac02_rule_pack_content_changes_result_and_missing_item_fails_closed -- --exact` 返回 exit `1`；原因是判据引用的 RulePack parser/resolver 生产契约尚不存在。实现后相同命令返回 exit `0`、`1 passed`。完整 application 直接测试同样为 `1 passed`。
- 分层门禁：`scripts/check-layering.ps1` 返回 exit `0`，报告 `AC03=0 market branches; AC01=0 domain rule values; Phase2C production C++/FFI rule values=0; allowlist=0`。`scripts/test-layering-check.ps1` 返回 exit `0`、`33 assertions`；它逐项保留 domain 规则值、C++/FFI 规则值、市场分支与非空 allowlist 的真实违规 exit `1`，并新增六类 C++ 交割规则值 fixture。
- 分层脚本的 base-to-current diff 已单独复核：`check-layering.ps1` 将通用 source inventory 覆盖仓库根，并新增独立、无 allowlist 的 Phase 2C C++/FFI 六类规则值扫描；`test-layering-check.ps1` 只扩大负向覆盖；`layering-allowlist.json` 只删除最后的 R2 特例并最终为 `[]`。没有缩小扫描根、增加排除项或弱化失败条件。
- 契约与规则包：冻结 pack 漂移检查、Buf format/lint 均返回 exit `0`；descriptor inventory 为 `14 passed`。CGB parser crate 为 `1 passed`；domain delivery contracts 为 `2 passed`；Rates service 为 `3 passed`；native delivery acceptance 为 `3 passed`；Phase 2C deterministic Arrow 为 `1 passed`。Rust strict Clippy 返回 exit `0` 且无 warning。
- Phase 2E live SDK：`scripts/check-phase2e-sdk.ps1` 返回 exit `0`、`1 passed`。它启动真实 API/gRPC-Web、native engines 与 CGB parser 的 live 测试组合；Python SDK 绑定冻结 pack 的实际 SHA-256 后完成全部 Phase 2 reference slices。该测试仅以只读 fixture DefinitionRepository 隔离普通离线门禁；持久化生产 DefinitionRepository 仍由下一条的真实拓扑与 Phase 2C SIT 覆盖。
- Phase 2C/2D 回归：`ctest --test-dir build/local-cpp-vs-llvm-19 --output-on-failure` 为 `8/8 passed`。Phase 2C matrix 为 `18/18 PASS`、Phase 2D matrix 为 `18/18 PASS`；Phase 2C 与 Phase 2D 独立 Decimal Oracle 各为 `3 passed`。Python generated-contract 测试为 `1 passed, 1 skipped`。`scripts/check-fast.ps1` 返回 exit `0`。
- 真实 PostgreSQL/Ceph：在脚本启动的可丢弃本地 PostgreSQL 16 + Ceph RGW 中，`cargo test --offline --locked -p ficant-storage --test futures_delivery_sit -- --test-threads=1` 返回 exit `0`、`1 passed`，覆盖发布、adapter 重建后的重放、metadata size 篡改的 HashMismatch、staging/orphan 清空。
- 完整本地拓扑：`scripts/dev-up.ps1` 成功构建并启动 PostgreSQL、Ceph、server、worker、web 与 UI；在本地 PostgreSQL DefinitionRepository backing store 登记三个精确 RulePack 后，经 UI 的真实 gRPC-Web `/ficant-api/ficant.rates.v1.RatesAnalyticsService/AnalyzeFuturesDelivery` 验证：冻结 v1 RulePack 的 4/4 单券 Golden Case 与 3/3 T basket 逐项匹配 Phase 2C expected，`ctd_index=1`；仅将 v2 的 `nominal_coupon.coefficient` 从 `3` 改为 `4` 后，同一 T basket 的 conversion factor 从 `0.965` 变为 `0.8991`；删除 `products[product_code=T].residual_min_months` 后返回 non-retryable `VALIDATION_FAILED`，唯一 field violation 为 `context.rule_pack.content.products[product_code=T].residual_min_months`。API 直接测试的 spy 同时证明该缺项请求的 engine call count 为 `0`。每次拓扑验证后均由 `scripts/dev-down.ps1` 停止容器并保留命名开发卷。
- 冻结事实：5 个 Phase 2C immutable facts 与 5 个 Phase 2D Golden/Oracle facts 的当前 SHA-256 均与 matrix 登记值一致；base-to-current 没有任何 Golden/Oracle 或 `crates/ficant-data/src/canonical.rs` diff，Canonical Quote Schema hash 仍为 `e804a0becec18e51dde1be4250384ffe667cf4149c34dc3d2cfc82a206d71502`。未出现 `position.proto`、`factor.proto`、`health.proto`、`constraint.proto` 或 `policy.proto`。PowerShell parser、tracked diff whitespace 与 R2 允许写路径审计也都通过。
- Matrix rebaseline：Phase 2C 已从自管 `guarded_files` 拆为固定的 5 个 `immutable_facts` 与精确 6 个 `rebaselined_tests`；两个 matrix 已校验 shared layout hash 相同。Phase 2D base-to-current diff 只有 `test_constants_and_layout.cpp` 的一个 hash 替换。最终提交时，这两个 matrix 文件必须仍是一个仅含它们的独立 rebaseline commit。
- 文档一致性：已逐项复核 `MANUAL.md` §3/§6；它已明确交割从精确 RulePack 内容解析、换内容会改变计算、缺项以 field violation 失败关闭，且不再声称规则硬编码或换版本不换结果。上述真实 gRPC-Web 对照切片是该表述的同一候选证据。
- 完整本地门禁：`scripts/check.ps1` 返回 exit `0`；随后在脚本启动的可丢弃 PostgreSQL/Ceph RGW 拓扑中，`scripts/check.ps1 -IncludeIntegration` 亦返回 exit `0`。后者覆盖 migration `4 passed`、Phase 4 queue/execution/worker、Phase 1、13 个 negative invariants、Phase 2B、Phase 2C、Phase 2D 与 Phase 3A/3B 的真实集成切片；结束后 `scripts/dev-down.ps1` 已停止容器并保留命名开发卷。

**允许写路径：**

- `docs/iterations/2026-08-r2-rulepack-parsing.md`
- `domain-packs/cgb-futures/**`
- `interface/proto/ficant/market/v1/rule.proto`
- `interface/proto/ficant/market/v1/cgb_futures_rule.proto`
- `crates/ficant-contracts/src/generated/ficant.market.v1.rs`、`crates/ficant-contracts/src/generated/ficant.market.v1.tonic.rs`
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/rule_pb2.py`、同目录新增 `cgb_futures_rule_pb2.py`
- `web-dm/packages/contracts-generated/src/ficant/market/v1/rule_pb.ts`、同目录新增 `cgb_futures_rule_pb.ts`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs` 与三侧 market contract 直接导入/consumer 测试，以及 `python/tests/test_rates_sdk_live.py` 的 R2 RulePack binding
- `Cargo.toml`、`Cargo.lock`、`crates/ficant-cgb-futures-pack/**`
- `crates/ficant-domain/src/market/market_rule_pack.rs`、`crates/ficant-domain/src/market/mod.rs`、`crates/ficant-domain/src/futures_delivery.rs` 与其直接测试
- `crates/ficant-application/src/ports/rule_pack_parser.rs`、`crates/ficant-application/src/ports/definitions.rs`、`crates/ficant-application/src/ports/fingerprint.rs`、`crates/ficant-application/src/ports/mod.rs`、`crates/ficant-application/src/use_cases/futures_delivery.rs`、`crates/ficant-application/src/error.rs`、必要的 module export 与 R2 直接测试
- `crates/ficant-api/src/rates.rs`、`crates/ficant-api/src/core_error.rs` 及 R2 直接测试（包括 `crates/ficant-api/tests/phase2e_sdk_live.rs`）
- `crates/ficant-storage/src/postgres/codec.rs`、`crates/ficant-storage/src/postgres/definitions.rs` 与 Phase 2C 直接测试夹具
- `crates/ficant-fixed-income-native/**`、`crates/ficant-kernel-sys/**` 中仅交割 ABI/adapter 的直接文件
- `cpp/fixed-income-kernel/include/ficant_kernel.h`、`cpp/fixed-income-kernel/src/futures_*`、对应 C++ tests
- `binaries/ficant-server/Cargo.toml`、`binaries/ficant-server/src/lib.rs` 中仅 Rates 的生产 resolver 组合
- `scripts/generate-cgb-futures-pack.ps1`、`scripts/check-layering.ps1`、`scripts/test-layering-check.ps1`、`scripts/check-phase2e-sdk.ps1`、`scripts/layering-allowlist.json` 及现有本地检查入口中的 R2 测试登记
- `tests/phase2c/acceptance-matrix.json`、`tests/phase2c/verify_acceptance_matrix.py`，仅用于拆分 immutable/rebaselined 集合、登记最终测试 hash 与校验该结构
- `tests/phase2d/acceptance-matrix.json`，仅允许在独立 rebaseline 提交中更新 `cpp/fixed-income-kernel/tests/test_constants_and_layout.cpp` 的 hash
- `docs/architecture/data-dictionary.md`、`docs/product/scope.md`、`docs/development.md`、`interface/README.md`、`README.md`、`MANUAL.md` 中仅 R2 事实同步

**禁止写路径：**

- `SPEC.md`、`ACCEPTANCE.md`、`docs/architecture/adr/**`
- `tests/golden-cases/**`、`tests/oracle/china-rates/phase2c_manual_oracle.py`、`tests/oracle/china-rates/test_phase2c_manual_oracle.py`
- `crates/ficant-data/src/canonical.rs`、Phase 3A/3B schema/hash 资产
- `crates/ficant-domain/src/market/bond.rs`、`crates/ficant-domain/src/subject.rs`
- `crates/ficant-domain/src/futures_hedge.rs`、`cpp/fixed-income-kernel/src/hedge_*` 及 Phase 2D expected/Oracle
- 任何 `position.proto`、`factor.proto`、`health.proto`、`constraint.proto`、`policy.proto`
- `migrations/postgresql/**`（本设计不需要物理 schema 变更）
- `.github/**`、`cicd.yml`、`deploy/**`

若实现必须超出允许路径，Root 必须停止并返回 Human 扩权；不得先改后补。

---

## 7. 残余风险

- **复现依赖精确工具版本。** 候选已经用 Node `v22.17.0`、uv `0.7.13 (62ed17b23 2025-06-12)`、pnpm `10.12.4` 与 Buf `1.56.0` 完成完整门禁；Human 重跑仍须提供同一版本或让 `check.ps1` 的版本断言失败关闭。这不是产品语义风险，也不应通过版本 shim、宽松 engine 或自动安装绕过。
- **旧 RulePack 没有内联内容。** R2 保证它们仍可读取，但凡交割计算声明需要 `cgb-futures` 内容就会失败关闭；历史数据若要重算，必须显式登记带内容的新版本，不能把当前内容补写进旧版本。
- **Domain Pack 的生产管理入口仍未交付。** R2 通过现有 DefinitionRepository 与本地受控登记完成真实解析/计算，不新增管理员 UI、外部抓取或白名单流程；这些属于 R6。正式环境导入仍受平台管理员边界约束。
- **AC27、AC28 在 R2 后仍未点亮。** R2 不新增时点事实，也不证明当前清单历史回算或连续合约输入会被拒绝；Human 必须在后续入口语义工作中归属并实现，最迟在 R7 全量重取证前关闭，不能把 R2 的 RulePack 成功解析当作替代证据。
- **产品身份仍固化为 core/public contract 的 `CgbFuturesProduct` enum。** R2 只把规则数值移出 core；新增 CFFEX 产品仍需修改 Rates proto、domain 与 ABI 身份枚举，会在 R7 的 AC04“虚构市场 L0/L1/L2 零改动”验证中正面暴露。R7 可能需要以字符串 `product_code` 或等价的 L3 所有身份替换 enum；该破坏性变更的设计与迁移成本不计入 R2。
- **Phase 2D 仍有独立的 CFFEX 合约面值与产品 enum。** 本轮按唯一范围源不扩展到 AC29；R3 处理 Subject 绑定与套保语义时必须继续遵守 S1/S3，不能把 R2 的 delivery 解析成功误写成 hedge 已完成规则解析。
- **交割专用 ABI 会发生有意的结构变化。** Phase 2A/2B/2D ABI 与算法不得随之漂移；R2 必须以布局测试、旧路径回归和完整 Phase 2C 重取证控制传播风险。
- **`google.protobuf.Any` 带来 type URL 治理风险。** parser 只接受一个精确、版本化 URL；未知 URL 失败关闭。不得建立别名表、模糊匹配或“最新 schema”回退。
