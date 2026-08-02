# 分层审计与重构路线

> 性质：编译产物。规范条款在 `SPEC.md`，判定条款在 `ACCEPTANCE.md`，本文只是通往它们的施工说明。
> 本文过时不影响系统正确性；SPEC 过时才影响。

**日期** 2026-07-26 · **依据** SPEC v1.0 §1 分层法则 · **Human 决定** 允许破坏性重构；一期补齐全部五个 L1 对象

---

## 一、诊断：倒金字塔

当前各层的实际状态：

| 层 | 状态 | 说明 |
|---|---|---|
| L0 内核不变量 | **扎实** | primitives 完整，双时间性在 ingest → snapshot 全程 fail-closed，血缘链甚至覆盖了运行镜像与源码摘要 |
| L1 量化形状 | **空心** | Factor / Exposure / Position / Constraint / ShadowPrice / Subject / Policy 全部缺失 |
| L2 定价原语 | **领先，但塌了一角** | 九类中覆盖第一类与第六类，质量高、有独立 Oracle；但内嵌了 L3 内容 |
| L3 规则包 | **有壳无实** | MarketRulePack 的结构与生效日期机制完备，但规则内容没有被计算路径消费 |
| L4 主体包 | 未开始 | — |
| L5 研究方法 | 未开始 | — |

**核心问题：细节比骨架先建成。** Phase 2 的定价库站在一个还没有 Factor、没有 Position、没有 Subject 的 L1 上。继续往 L2 加原语只会加深倒挂——每加一类，将来接入因子体系的成本就多一份。

---

## 二、缺口清单

按危害排序。每条给出：现状证据 → 违反的条款 → 改进方案 → 归属迭代。

### 缺口 1 · 规则包进了血缘，没进计算 ⚠ 最危险

**现状。** `crates/ficant-domain/src/futures_delivery.rs` 把中金所规则硬编码在 domain 里：`original_term_months()` 返回 60/84/120/360，`residual_term_bounds()` 返回 (18,27)/(48,63)/(78,None)/(300,None)。`FuturesDeliverableInput` 确实接收 `rule_pack: AnalyticsObjectRef` 并写入血缘，但**计算不从该 pack 解析任何数值**。

**为什么这是最危险的一条。** 它比完全没有规则包更糟——血缘会诚实地记下"用的是 v2 规则包"，算出来的却仍是 v1 的数字，**而且不会报错**。这正是"不报错、只让你得出自信而错误的结论"的那类缺陷。

**违反：** SPEC S1（L2 依赖了 L3）、S3（引用而未解析）。

**方案。**
1. 把 CFFEX 合约规则移入 `cgb-futures` RulePack 内容：产品代码、原始期限、可交割剩余期限区间、交割月定义、票面基准。
2. domain 侧只保留**形状**：`DeliverabilityRule { original_term_months, residual_min, residual_max }`，由 pack 解析注入。
3. 解析失败或规则项缺失一律 `RulePackItemMissing` 失败关闭，禁止默认值兜底。
4. 重跑 Phase 2C 全部取证。

**归属：** R2。**点亮：** AC01、AC02、AC26。

---

### 缺口 2 · L1 七个对象缺失

**现状。** 全 crate 检索无 `Factor` / `FactorId` / `Exposure` / `Position` / `Constraint` / `ShadowPrice` / `Subject` / `Policy` 的一等表达（`FactorDefinition` 只出现在 README 的对象清单里，代码中不存在）。

**违反：** SPEC §1 L1 定义、S5、I4、I6、I7。

**方案。** 七个对象一次性建立 proto 契约，分两批落实现：

**契约在各自实现轮次定义，不提前批量冻结**——Human 已批准全程破坏性变更，提前冻结的收益因此消失，反而会造成 speculative generality（方法论 R11：不预留插槽）。

| 对象 | 最小内容 | 归属 |
|---|---|---|
| `Subject` | 准入集、资金档可得性、税收待遇、约束集引用、考核机制；可版本化、进血缘 | **R1 契约 + 注册**，R3 语义 |
| `SubjectStateSnapshot` | 净资本、各项额度上限；走双时间通道 | **R1 契约 + 注册** |
| `Position` / `PositionSnapshot` | 仓位、会计分类三态、回购与借贷穿透标记；走双时间通道 | R4 |
| `Factor` / `FactorId` / `Exposure` | 全局唯一标识、经济量定义、单位、敏感度口径；资产↔因子双向可查 | R4 |
| `CoverageDeclaration` | 组合级覆盖度元数据、分母、缺失项与字段可信度分布 | R5b |
| `DataHealthReport` | 缺失与异常的可查询预警清单 | R5c |
| `Constraint` / `ShadowPrice` | 约束形状（上下限、口径、绑定的 Subject 与 RulePack），**不含任何具体数值** | **v0.2** |
| `Policy` / `PolicyArtifact` | 参数、适用券域、有效期、标定证据引用、求值器版本 | **v0.2** |

**关键：** `Constraint` 的**形状**在 L1，`500%`、`NSFR`、`LCR`、套保额度这些**数值**在 L3 的券商 RulePack。公募的 140%/200%、保险的偿付能力是另外两个 pack，core 一行不改。

**点亮：** AC05、AC06、AC07、AC16–AC19、AC20–AC23、AC24、AC25。

---

### 缺口 3 · Bond 只有一个发行日

**现状。** `ficant-domain/src/market/bond.rs` 的 `Bond` 与 `analytics.rs` 的 `BondTerms` 都只有单一 `issue_date`。

**后果。** 中国国债是续发制度。用一个字段同时承担"首发"与"本期发行"两个语义，**税收属性判定与累计发行量判定两头都错**：2025-08-08 增值税新老划断锚定的是首发日，而 CTD 与活跃券判定关心的是累计发行量。

**违反：** SPEC §4（一期必须能判定税收属性）。

**方案。** 破坏性拆分为 `first_issue_date` / `current_issue_date` / `cumulative_issued_amount`，并新增券级税收属性（增值税状态、企业所得税状态，锚定首发日）。**税收属性是 Instrument 的一等属性（属于什么类），税率与计算口径是 L3 TaxRulePack（怎么算）——两者不可混。**

**归属：** R3。**点亮：** AC08、AC09、AC10。

---

### 缺口 4 · 价格不区分来源，且单源单行

**现状。** Canonical Quote 的 16 列只有 `bid/ask` 系数与标度、`observed_at`、`visible_at`、instrument 与 unit 引用。没有来源类型、没有可信度、同一 instrument × 时点无法并存多源。

**后果。** DataSnapshot 会把中债估值与真实成交无差别冻结进 Parquet。快照可复现，但复现出来的可能是被平滑过的假波动率——回测最大回撤会系统性小于真实值。且多源分歧无处存放，而分歧本身是有效信号。

**违反：** SPEC I9。

**方案。** R5a 采用 DataSource 级的 B′ 契约：封闭的 `PriceSourceType`（真实成交 / 活跃报价 / 模型估值 / 曲线插值）属于精确、不可变的 DataSource 版本；每个 dataset 必须语义同质，物理源内混合的不同价格语义必须拆成不同注册源。`FactSource` 绑定精确 DataSource 版本，记录类型与注册类型不相容、类型缺失或枚举越界均在数值引擎前失败关闭。canonical quote v1 的 16 列、schema id 与 hash 保持不变，快照通过 manifest 中的精确 DataSource 引用解析类型；既有未分类 source / fact / snapshot 不作推断，进入 typed 计算时明确失败。内部由曲线插值形成的价格由算法路径标记为 `CURVE_INTERPOLATION`，不得伪装成外部 DataSource 属性。风险指标输出稳定的来源类型摘要；混合类型只标记，不降级、不设阈值、不阻断。

R5b 在 R5a 之后建立 `CoverageDeclaration`，R5c 在 R5a 之后建立 `DataHealthReport`；三轮分别冻结 base 与逐文件写路径，各自只点亮一条 AC。

**归属：** R5a。**点亮：** AC15。

---

### 缺口 5 · 枚举只有单变体，扩展点未留

**现状。** `DayCountConvention` 仅 `ActActBondIsma`；`BusinessDayConvention` 仅 `Following`；`CouponFrequency` 仅年 / 半年。

**后果。** 这不是当下的错误（一期只做国债，够用），但它意味着**扩展点的形状尚未被验证**。第二个惯例进来时才发现抽象不对，成本远高于现在留好。

**方案。** 不急于补变体，但在 R1 冻结契约时确认这些 enum 的承载方式能容纳"惯例由 RulePack 指定"而非"由代码枚举穷举"。

**归属：** R1（仅契约层面确认）。

---

### 缺口 6 · 边界表述与新决定冲突

**现状。** README §3.2 把"投资组合会计"列入不负责清单；README 多处以"不做 OMS/EMS"划界。

**冲突。** 持仓与会计科目已决定进入 ficant；且"不做 OMS"这条界会误伤 TCA、冲击成本、渠道选择——那些是日频或交易前查询的**建模**，不是报单。

**方案。** 已在 SPEC §2 以 B1 / B2 两条澄清取代：ficant 负责执行相关的建模与标定，不负责运行时调用；计量不核算。README 相应章节在 R1 迁出或改写。

**归属：** R1。

---

### 缺口 7 · AnalyticsService 无一等地位

**现状。** Phase 2E 的 `RatesAnalyticsService` 事实上已是一个幂等、绑定快照与规则版本、结果确定性可校验的服务，但它游离在 ResearchGraph 之外，架构文档中没有它的位置。

**后果。** 频率分层里"交易前查询 + 分钟到日内"那约 9 个组件（边际资本成本、冲击成本、渠道选择、组合穿透、残差扫描、波动率曲面、LCR/NSFR）无处安放——它们要求被**查询**，不是被**批处理**，也不该被包装成一次性实验。

**方案。** 立 ADR 承认 `AnalyticsService` 为与 `ResearchGraph` 并列的一等执行形态：同样绑定快照 / 规则 / 主体，同样进血缘，但语义是幂等查询而非有状态运行。ficant 由此具备批处理与在线服务两种运行模式。

**归属：** R1（ADR）+ R5（承载约束查询）。

---

### 缺口 8 · 求值器边界未定义

**现状。** ficant 的正式输出只有 SignalSet / TargetExposure，都是**数值**。做市定价能力的输出是一个**函数**，装不下。

**后果。** 若执行层照文档重新实现策略，ficant 的血缘链在边界断裂，下游全部不可验证——这会抵消 ficant 存在的理由。

**方案。** 新增 `PolicyArtifact` 输出类型，且**必须与求值器实现一同交付**，回测与实盘共用同一份实现。已写入 SPEC §2 B2。

**归属：** R1（契约）+ R6（落地）。

---

## 三、迭代序列

每轮必须交付**可亲手操作的垂直切片**，结束时系统可运行。

| 轮 | 名称 | 交付 | 点亮 |
|---|---|---|---|
| **R1** | **分层门禁与主体契约** | 分层检查脚本 + 递减式 allowlist；`subject.proto` / `subject_state.proto`；`RegistryService` 注册与查询；ADR 0011–0018 批准；文档权威交接。**垂直切片：注册一个主体、查回、并让主体版本出现在一次 `AnalyzeBond` 的血缘中（只携带，不参与计算）** | AC03 |
| **R2** | 规则包必须被解析 | CFFEX 规则移入 `cgb-futures` pack；解析器与 fail-closed；从 allowlist 移除 `futures_delivery`；重跑 Phase 2C 取证 | AC01 AC02 AC26 |
| **R3** | 主体语义与税收 | 主体解析资金成本与税收待遇；`Bond` 拆首发/本期 + 累计发行量；券级税收属性；税后现金流；全部 rates 调用绑定主体 | AC06–AC10 AC29 |
| **R4a** | **CTD 双时间与具体合约** | verified DataSnapshot 的历史 CTD 清单 / 价格边界；具体 FuturesContract 输入拒绝语义 | AC27 AC28 |
| **R4b** | **PositionSnapshot 与会计视图** | `PositionSnapshot` 走双时间通道；会计三态与 fail-closed；回购穿透的敞口 / 可用流动性边界 | AC14 AC17–AC19 |
| **R4c** | **Factor 身份与拓扑** | 全局 immutable `FactorId`、敏感度口径、稳定曲线节点定义及 asset ↔ Factor 双向索引；不产生数值 Exposure | AC05 |
| **R4d-a** | **可验证风险输入与债券 KRD** | 完整 Bond 定价条款、verified curve points 与 Factor convention 执行；生成逐债券仓位 KRD 和债券子组合 totals；非债券敞口失败关闭 | —（AC16 前置） |
| **R4d-b** | **具体期货 KRD 与全组合聚合** | 依赖 R4d-a；复用 R4a 的 exact FuturesContract / CTD materializer，内部生成期货 KRD 并与债券逐仓位结果完整聚合 | AC16 |
| **R5a** | **价格来源类型与可信度标记** | 精确 DataSource 版本绑定封闭来源类型；Fact / verified snapshot 可解析该类型；内部曲线插值价格显式标记；混合来源的风险结果携带类型摘要。canonical quote v1/schema/hash 不变 | AC15 |
| **R5b** | **组合覆盖度声明** | 依赖 R5a；所有多仓位聚合输出携带含分母、参与数与总额、缺失关键字段数及字段可信度分布的 `CoverageDeclaration`；机械门禁与真实负向 fixture 禁止裸组合数值 | AC35 |
| **R5c** | **数据健康度预警** | 依赖 R5a；`DataHealthReport` 以 AnalyticsService 形态落地；阈值来自显式配置并进入结果证据；同一 UNKNOWN snapshot 健康度预警但不阻断，资本占用仍按 AC17 失败关闭 | AC36 |
| **R6** | 角色与白名单 | 平台管理员与研究用户分离；数据源导入白名单；基础数据变更留痕 | AC37 |
| **R7** | 一期收口 | **虚构市场零核心改动验证**（健康度指标）；全量重取证；MANUAL 走查 | AC04 AC11–AC13 AC30–AC33 |

### R1 为什么这么小

R1 是唯一一轮**以建立判据为主**的迭代——这是它的性质，不是缺陷。它只点亮 AC03 一条，但同时建立 AC01 / AC02 / AC04 的自动化判据。

两点值得说明：

- **不再提前批量冻结七个契约。** Human 已批准全程破坏性变更，"早冻结以免将来返工"的理由随之消失；提前定义 Constraint 与 Policy 反而是在没有实现经验的情况下猜接口。改为在各自实现轮次定义。
- **分层门禁必须最先，且带递减式 allowlist。** `futures_delivery` 现在就违反 S1，门禁一上会立刻变红。allowlist 让它可控地红着：条目**只能移除、不能新增**，R2 移除唯一一条后 allowlist 清空。这样门禁从第一天起就阻止**新增**塌层，而不必等 R2 修完才生效。

### 与原 Phase 计划的关系

- Phase 0–4 的既有成果**保留**，但 Phase 2C 与 Phase 3A 的取证在 R2 / R4 后**必须重跑**（破坏性变更已获批准）。
- 原 Phase 5（Rates Research Lab）**后移至 R7 之后**——在 L1 建成之前做业务界面，界面会绑死错误的对象模型。
- 原 Phase 7 / 8（AI 基础设施与 GeneratedNode）**整体后移**。理由：两份研究文档一致判定瓶颈是领域知识与人工投入，不是算力；且 GeneratedNode 要替换的撮合假设，其输入数据（盘口历史、中介报价流、询价日志）在数据层尚不存在。**沙箱建好了没有料。**
- AI 在本阶段的正确用武之地在 L5 与数据管道：条款解析、报价文本实体识别、非市场化成交打分。这些不需要 gVisor。

---

## 四、待立 ADR

全部 **Accepted**（2026-07-26）：

| 编号 | 标题 | 归属 |
|---|---|---|
| [0011](adr/0011-position-as-snapshot-not-state.md) | 持仓是快照不是状态；计量不核算；会计分类显式三态 | R1 |
| [0012](adr/0012-research-subject-identity-and-state.md) | 研究主体为一等对象；身份版本化、状态快照化 | R1 |
| [0013](adr/0013-layering-law-shape-in-core-content-in-rulepack.md) | 分层法则：core 定义形状、RulePack 定义内容；规则必须被解析 | R1 |
| [0014](adr/0014-policy-artifact-and-shared-evaluator.md) | PolicyArtifact；交付策略不执行策略；求值器共用同一份实现 | R1 |
| [0015](adr/0015-global-factor-identity.md) | 全局因子体系与 FactorId 唯一性；敏感度口径统一 | R1 |
| [0016](adr/0016-analytics-service-as-first-class-execution.md) | AnalyticsService 为与 ResearchGraph 并列的一等执行形态 | R1 |
| [0017](adr/0017-data-health-and-coverage-declaration.md) | 只按已导入数据评估；覆盖度显式声明与数据健康度自检 | R1 |
| [0018](adr/0018-platform-admin-and-researcher-separation.md) | 平台管理员与研究用户的职责分离 | R1 |

0017 与 0018 源自 2026-07-26 的两项 Human 裁决，二者引入的横切原则已写入 **SPEC v1.1**（不变量 I10 覆盖度显式、§5 角色分离），对应验收为 `AC35` / `AC36` / `AC37`。

---

## 五、健康度指标

重构完成后应持续监测一个数字：

> **新增一个市场所需的 L0 / L1 / L2 源码改动行数。理想值为 0。**

它是"多资产野心是否失控"的唯一客观度量，对应 ACCEPTANCE AC04。非零即分层错误，须以 ADR 记录并修正，不得默默接受。
