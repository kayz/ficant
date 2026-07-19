# ficant

> **Fixed Income Connected AI-Native Technology**  
> 固定收益优先、AI 原生、领域驱动、可复现的量化研究操作系统。

**文档版本：** v0.2  
**文档状态：** 系统级 README / 架构基线  
**首个市场：** 中国国债现券与国债期货  
**唯一后台语言：** Rust  
**研究节点语言：** Python  
**数值库：** C++20，通过稳定 C ABI 接入 Rust  
**普通开发环境：** Windows 11 + PowerShell 7
**部署与兼容目标：** Linux x86_64（仅在明确的兼容、发布或 UAT 门禁验证）
**开发数据库：** PostgreSQL 16  
**信创目标数据库：** openGauss  
**平台终点：** 研究结果、回测结果、SignalSet 与 TargetExposure  
**平台不负责：** OMS、EMS、对外报单、订单管理、清算与结算

## 开源许可证

FICANT 是公开开源项目，源代码采用 [MIT License](LICENSE)。第三方依赖和随项目分发的第三方材料仍分别适用其原有许可证与署名要求，详见 [`docs/delivery/third-party-notices.md`](docs/delivery/third-party-notices.md)。

## 开发与发布边界（2026-07-17）

- [OPAID](docs/development.md) 管理从任务冻结、实现和本地测试到精确自测候选；它不执行 CI/CD、部署、UAT 或服务器管理。
- Windows PowerShell 7 统一入口为 `.\scripts\check-fast.ps1` 与 `.\scripts\check.ps1`；两者支持 `-ListOnly`，完整检查可显式增加 `-IncludeIntegration`。
- 中央 `kayz/cicd` 管理候选合并后的 GitHub CI、Linux 镜像、GHCR、测试环境部署、健康/冒烟检查和回滚。本地通过不能替代正式质量门槛。
- 历史 HOQA/PROQAID 材料保留在 `docs/history/hoqa/` 作为当时证据，不再驱动当前工作；权威边界见 [ADR-0009](docs/architecture/adr/0009-opaid-local-development-and-cicd-release-boundary.md)。

> 本文是 ficant 当前唯一的系统技术基线。除非通过正式 ADR 修改，后续设计和实现不得引入平行后台语言、平行数据库、平行 API 契约或平行运行体系。

## GitHub 测试环境发布（2026-07-17）

- 本节属于中央 `cicd` 发布管理边界，不是 OPAID 本地自测的一部分。
- 中央管理源位于私有仓库 `kayz/cicd` 的 `ficant/`；本仓库中的 `cicd.yml`、`.github/workflows/release-test.yml` 和 `deploy/test/` 是固定平台版本生成的业务接入文件。
- `main` 的现有十项 `ci` 全部成功后，GitHub Linux Runner 从精确 Commit SHA 构建 `ficant-server`、`ficant-worker` 和 `ficant-web` 镜像，推送 `sha-<commit>` 与 `test-latest` 标签到 GHCR。测试机始终部署 SHA 标签，不依赖 `latest`。
- GitHub `test` Environment 通过专用 SSH 身份连接测试机的 `ficant-deploy` 账号；测试机只拉镜像、执行版本化 PostgreSQL migration 和 Docker Compose，不现场编译源码。
- 发布脚本记录 current、previous、镜像 SHA、部署时间、migration、健康检查和冒烟结果；失败时如存在 previous SHA，直接切回上一组镜像。
- 当前测试发布拓扑仍未把对象存储 adapter 装配进三个发布二进制；源码 Workspace 已统一迁移到 Apache `object_store` + Ceph RGW，锁文件与可达依赖图均不再包含 `minio`/`async-std`，既有 `RUSTSEC-2025-0052` 风险接受已退出。开发与 CI 使用锁定摘要的单节点 RGW 夹具，生产 Ceph 集群拓扑仍需独立运维授权。
- 该环境只证明发布链路和当前最小服务探针，不等于完整业务 UAT、生产发布或对象存储验收。

---

## 1. 产品定义

ficant 是面向专业投资研究团队的 AI 原生量化研究平台。平台连接外部数据源，将数据、领域知识、研究方法、因子、模型、策略、仿真和信号组织为可版本化、可追踪、可复现的研究资产，并通过统一接口向 WebApp、Python SDK 和 AI Agent 提供能力。

ficant 不是传统研究平台外加一个聊天框。它把 AI 视为研究运行时的一部分：

- 平台固化相对稳定的量化领域知识和市场约束；
- 用户以自然语言、参数或代码表达研究意图；
- AI 可以实时生成、修改和组合研究节点；
- Rust 确定性运行时负责执行、校验、记录和重放；
- AI 生成的代码只能在隔离环境中运行；
- 每次实验都留下完整数据、代码、环境、规则、模型调用和结果证据；
- WebApp 将平台能力组合为面向特定研究场景的独立产品。

ficant 的总体构成是：

```text
ficant
  = Domain Kernel          稳定领域内核
  + Typed Research Graph   强类型研究图
  + Generative Runtime     生成式研究运行时
  + Evidence Ledger        证据化运行账本
  + App Fabric             WebApp 应用体系
```

产品原则：

> **稳定领域内核，生成研究节点，证据化运行，应用化交付。**

---

## 2. 完整愿景

### 2.1 面向全市场的研究操作系统

ficant 的长期目标是覆盖股票、固定收益、期货、期权、基金、外汇和其他可研究资产，但不通过一张“万能表”强行统一所有市场。

平台采用：

```text
稳定核心对象
+ 可版本化 Domain Pack
+ 市场规则包
+ 强类型研究节点
```

核心对象负责时间、证券身份、单位、快照、实验、产物、信号、权限和血缘。不同品种的现金流、合约条款、交易方式、风险量和市场规则由 Domain Pack 逐步扩展。

### 2.2 数据源中立

ficant 不是数据库。数据库、数据文件、数据服务和机构内部系统都是平台输入。

平台通过统一的数据适配层完成：

- 数据源注册；
- Schema 发现；
- 证券身份映射；
- 时间和交易日历统一；
- 点时可见性处理；
- 数据质量检查；
- 不可变研究快照；
- 来源、版本和血缘记录。

平台保存元数据、快照 Manifest、必要的列式数据、缓存和研究产物，但不承担原始市场数据供应商的职责。

### 2.3 研究资产全生命周期

以下对象在 ficant 中都是一等资产：

- 数据视图与数据快照；
- Universe 与样本定义；
- 因子、标签和特征；
- 模型定义和模型产物；
- 策略定义与组合规则；
- 定价、风险、成本、滑点和撮合策略；
- ResearchGraph 与实验运行；
- 分析报告和归因结果；
- SignalSet 与 TargetExposure；
- AI 生成的可复用研究能力。

每个资产都具有：

```text
type
version
owner
permission
status
dependency
lineage
created_at
content_hash
```

### 2.4 AI 实时实现研究节点

研究过程中的方法选择不全部写死在平台中。用户可以在实验过程中临时调整一个研究节点，AI 根据节点契约生成代码并插入当前实验分支。

例如：

> 当前国债期货成交假设偏乐观。根据盘口深度和撤单率降低队列推进速度，并限制策略成交量占同期市场成交量的比例。

平台执行：

```text
用户意图
  → ResearchPatchSpec
  → AI 生成 Python 节点和测试
  → 静态检查
  → 领域不变量检查
  → 沙箱运行
  → 创建新的 ResearchGraph 版本
  → Rust 仿真引擎执行
  → 生成实验结果和证据凭证
```

AI 不直接进入逐 Tick 的非确定性决策过程。AI 负责生成程序，冻结后的程序负责执行。

### 2.5 平台能力随使用增长

AI 临时生成的能力具有明确生命周期：

```text
Ephemeral
  → Validated
  → Experimental
  → Registered
  → Approved
  → Deprecated
```

临时节点只属于当前实验。经过测试、复核和发布后，可以成为个人能力、团队能力、WebApp 内置能力或平台标准能力。

### 2.6 WebApp 应用体系

WebApp 是独立的研究产品，不是平台后台中的普通页面。

每个 WebApp 可以拥有：

- 独立 UI 和 UE；
- 独立导航与工作流；
- 专属研究图模板；
- 专属 Agent 指令；
- 专属报告和可视化；
- 随应用分发的 Domain Pack 与研究节点包。

每个 WebApp 必须复用：

- 平台用户和权限；
- 领域对象；
- 数据快照；
- ResearchGraph；
- 实验和证据账本；
- AI 工具；
- SignalSet 发布机制。

> **WebApp 可以定义研究体验，但不能重新定义平台事实。**

---

## 3. 系统边界

### 3.1 ficant 负责

| 领域 | 平台职责 |
|---|---|
| 数据接入 | 连接数据库、文件和数据服务，转换为统一研究数据 |
| 数据语义 | 证券身份、时间、点时性、日历、单位、来源、版本和质量规则 |
| 数据快照 | 创建不可变 Snapshot Manifest 和列式研究快照 |
| 领域知识 | 固定收益、国债期货、市场规则、参考算法和领域不变量 |
| 研究资产 | 因子、模型、策略、节点、实验、报告和信号生命周期 |
| 研究计算 | 定价、风险、因子、模型、组合、回测、仿真和归因 |
| AI 能力 | 研究计划、代码生成、节点替换、测试生成、解释和实验比较 |
| 治理 | 版本、权限、审批、血缘、审计、证据和可复现性 |
| 开放接口 | gRPC、gRPC-Web、Python SDK 和 Agent Tool Gateway |
| WebApp | 应用注册、权限、版本、依赖、发布和运行管理 |

### 3.2 ficant 不负责

| 非目标 | 说明 |
|---|---|
| 数据库产品 | 不取代数据仓库、时序数据库和数据湖 |
| 数据供应商 | 不承担数据授权、销售和原始数据长期供应 |
| OMS / EMS | 不管理订单生命周期和交易柜台状态 |
| 对外报单 | 不向交易所、银行间市场、券商或柜台发送订单 |
| 清算与结算 | 不处理券款交收、交割操作和会计记账 |
| 投资组合会计 | 不承担估值核算、总账和监管报表 |
| 高频实盘执行 | 不追求微秒级实盘延迟 |
| 通用低代码平台 | 不承载与量化研究无关的企业应用 |
| 通用数据治理平台 | 只治理进入量化研究范围的数据与产物 |
| 通用大模型训练平台 | 只调用模型服务，不建设基础模型训练集群 |
| 无限制代码执行 | AI 代码只在受控节点契约和沙箱内运行 |

### 3.3 平台输出边界

ficant 的正式输出是：

```text
ResearchArtifact
SimulationResult
ReportArtifact
SignalSet
TargetExposure
```

平台不定义 `Order` 为正式领域输出。下游系统如何将信号转换为订单，不属于 ficant。

### 3.4 WebApp 边界

WebApp 不得：

- 直接连接外部数据库；
- 自建用户和权限体系；
- 绕过 DataSnapshot 使用未冻结数据；
- 绕过 ResearchGraph 执行代码；
- 覆盖共享研究资产；
- 自行发布无法追踪的信号；
- 携带独立后台服务绕过 Rust 平台。

WebApp 的后端能力必须通过 Domain Pack、ResearchGraph 模板和 CapabilityArtifact 接入平台。

---

## 4. 首个市场：国债现券与国债期货

ficant 的第一个市场不是泛固定收益，而是中国利率品种中的一条完整研究链路：

```text
国债现券
+ 国债期货
+ 收益率曲线
+ 资金成本
+ 可交割券与交割规则
+ 基差、IRR 与 CTD
+ 期现套保与曲线风险对冲
```

### 4.1 首版核心品种

- 银行间国债现券；
- 2 年期国债期货；
- 5 年期国债期货；
- 10 年期国债期货；
- 30 年期国债期货。

### 4.2 首版支撑数据

- 国债发行、存续和兑付信息；
- 票息、计息规则和现金流；
- 现券报价、成交和估值；
- 收益率曲线和曲线节点；
- 回购和资金成本；
- 国债期货行情、结算价和持仓；
- 可交割券篮子；
- 转换因子；
- 交易日历、结算日历和市场规则；
- 用于相对价值参照的利率债数据。

### 4.3 首批研究场景

- 债券现金流生成；
- 应计利息、净价和全价；
- 到期收益率反解；
- 久期、修正久期、凸性和 DV01；
- 关键期限风险；
- 收益率曲线构建、插值和快照比较；
- Carry 和 Roll-down；
- 相对价值分析；
- 期货基差、净基差和隐含回购利率；
- 可交割券与 CTD；
- 交割情景分析；
- 期现套保；
- 曲线风险对冲；
- 移仓和展期研究；
- 流动性、成本和撮合假设；
- 信号生成、回测、归因和实验比较。

### 4.4 首版不覆盖

- 信用主体和违约模型；
- 企业信用债完整择券体系；
- ABS 和结构化产品；
- 可转债；
- 利率互换完整生命周期；
- 债券一级发行；
- 银行间真实询价和交易通讯；
- 真实交割、清算和结算。

### 4.5 市场仿真边界

国债期货和银行间现券使用不同仿真模型：

```text
VenueSimulation
  ├── FuturesOrderBookSimulation
  ├── InterbankQuoteSimulation
  └── HistoricalTradeReplay
```

Rust 仿真引擎固化：

- 事件顺序；
- 数量守恒；
- 价格步长；
- 订单剩余量；
- 市场时钟；
- 数据可见性；
- 随机数种子；
- 结算和估值时点。

AI 生成节点可以调整：

- 队列推进；
- 成交概率；
- 参与率；
- 流动性；
- 滑点；
- 成本；
- 延迟；
- RFQ 成交假设。

AI 节点返回 `SimulationProposal`，由 Rust 引擎验证后形成正式成交事实。AI 节点不能直接写入成交账本。

---

## 5. 领域知识体系

### 5.1 三层领域知识

ficant 将领域知识分为三层：

| 层次 | 内容 | 实现方式 |
|---|---|---|
| 领域不变量 | 时间、单位、现金流、净价全价、事件顺序、点时性、血缘 | Rust Domain Kernel 固化 |
| 市场规则 | 合约条款、交易时段、可交割券、节假日、交割规则 | MarketRulePack 带生效日期版本化 |
| 研究方法 | 曲线、因子、模型、套保、成本、撮合和归因方法 | ResearchNode，可由 AI 生成和替换 |

原则：

```text
领域不变量      固化
市场规则        版本化
研究方法        生成化
```

### 5.2 Domain Pack

Domain Pack 是领域能力的交付单元。

```text
DomainPack
  - protobuf_descriptors
  - object_types
  - semantic_rules
  - market_rules
  - reference_algorithms
  - invariant_tests
  - research_node_contracts
  - agent_instructions
  - ui_metadata
  - effective_dates
```

Domain Pack 使用 Protobuf 描述对象和节点契约，使用 Protobuf JSON Mapping 保存可读规则数据，使用内容哈希确定版本。

### 5.3 首批 Domain Pack

| Domain Pack | 范围 |
|---|---|
| `core-quant` | Instrument、Market、Calendar、Unit、Snapshot、Experiment、Artifact、Signal、权限和血缘 |
| `china-rates` | 国债、现金流、计息、应计利息、净价、全价、收益率、曲线、久期、凸性和 DV01 |
| `cgb-futures` | 国债期货、可交割券、转换因子、基差、净基差、IRR、CTD、移仓和交割情景 |
| `venue-simulation` | 订单簿、历史回放、报价成交、流动性、成本和撮合不变量 |
| `research-methods` | 数据清洗、因子、标签、模型、回测、稳定性、归因和报告 |

后续市场按新的 Domain Pack 扩展，不修改已有历史对象的语义。

### 5.4 MarketRulePack

任何市场规则都必须具有来源、生效日期和版本。

```text
MarketRulePack
  - rule_pack_id
  - market
  - rule_type
  - effective_from
  - effective_to
  - schema_version
  - content_hash
  - source_reference
  - verification_status
  - content
```

历史实验始终绑定运行时生效的规则包，不使用“当前最新规则”重算历史。

---

## 6. 核心领域对象

### 6.1 数据与市场对象

```text
DataSource
DatasetView
DataSnapshot
Instrument
Bond
FuturesContract
DeliverableBond
Cashflow
Quote
Trade
Valuation
CurveSnapshot
RepoCurveSnapshot
Calendar
MarketRulePack
UniverseSnapshot
```

### 6.2 研究对象

```text
FactorDefinition
FactorRun
LabelDefinition
ModelSpec
ModelArtifact
StrategySpec
ResearchGraph
ResearchNodeContract
ResearchPatchSpec
CapabilityArtifact
ExperimentRun
SimulationResult
ReportArtifact
SignalSet
RunJournal
```

### 6.3 Definition、Run、Artifact

所有研究对象遵循三分法：

- **Definition：** 定义研究方法和参数；
- **Run：** 记录某次确定性执行；
- **Artifact：** 保存不可变结果。

修改 Definition 会创建新版本，不覆盖旧版本。重新运行会创建新 Run，不覆盖旧 Run。Artifact 一旦发布即不可变。

### 6.4 血缘

```text
SignalSet
  ├── StrategySpec version
  ├── ResearchGraph version
  ├── FactorRun versions
  ├── ModelArtifact version
  ├── UniverseSnapshot
  ├── DataSnapshot
  ├── MarketRulePack versions
  ├── CapabilityArtifact versions
  ├── runtime_image_digest
  └── ExperimentRun
```

任何正式信号必须能够反向追踪到完整研究证据。

---

## 7. AI 原生研究运行时

### 7.1 强类型 ResearchGraph

研究过程由有向无环图表达。节点之间通过 Protobuf 契约连接，不依赖 Notebook 单元格顺序和隐式全局变量。

```text
DataSnapshot
  → DataCleaningPolicy
  → CashflowBuilder
  → CurveSelectionPolicy
  → CurveBuilder
  → PricingPolicy
  → RiskCalculation
  → Factor / Feature
  → Model
  → PortfolioHedgePolicy
  → VenueSimulation
  → PnL / Risk / Attribution
  → SignalSet
```

### 7.2 两类节点

ficant 只定义两类执行节点：

1. **NativeNode**  
   Rust 实现，必要时调用 C++20 数值库。用于平台参考算法、领域不变量、高性能计算和核心仿真。

2. **GeneratedNode**  
   Python 3.12 实现。由 AI 或用户创建，用于研究方法、临时规则、因子、模型、组合、成本和撮合策略。

不允许在运行时向平台主进程加载第三类原生插件。

### 7.3 ResearchNodeContract

每个节点必须声明：

```text
contract_id
contract_version
input_types
output_types
state_schema
parameter_schema
determinism_class
permissions
resource_limits
required_invariants
```

示例：

```yaml
contract_id: ficant.venue.match-policy
contract_version: v1
stateful: true
determinism: seeded
permissions:
  network: false
  database: false
  filesystem: temporary-only
resource_limits:
  cpu_cores: 1
  memory_mb: 256
  timeout_seconds: 60
required_invariants:
  - no_overfill
  - no_fill_before_order_arrival
  - price_tick_compliance
  - deterministic_with_same_seed
```

### 7.4 ResearchPatchSpec

AI 修改实验前先生成结构化补丁：

```text
ResearchPatchSpec
  - target_experiment
  - target_graph_version
  - target_node
  - operation
  - replacement_contract
  - intent
  - assumptions
  - expected_effect
  - validation_plan
```

补丁不会修改原图，而是创建新的 ResearchGraph 版本。

### 7.5 生成流程

```text
用户研究意图
  → Agent 生成 ResearchPlan
  → Agent 生成 ResearchPatchSpec
  → 生成 Python 代码、Manifest 和测试
  → Python AST 与依赖白名单检查
  → Protobuf 契约检查
  → 单元测试
  → 属性测试
  → 领域不变量测试
  → 基准结果比较
  → gVisor 沙箱执行
  → 创建 CapabilityArtifact
  → 插入实验分支
  → Rust 运行时执行
  → 写入 RunJournal 和 Artifact
```

### 7.6 AI 代码沙箱

GeneratedNode 统一运行在 gVisor 隔离的 OCI 容器中。

默认约束：

- 无网络；
- 无数据库连接；
- 无密钥；
- 非 root；
- 只读根文件系统；
- 独立临时目录；
- CPU、内存、进程数和运行时限额；
- 固定 Python 镜像摘要；
- 固定依赖白名单；
- 平台注入确定性时钟；
- 平台注入带种子的随机数生成器；
- 输入通过 Arrow IPC 提供；
- 输出必须通过 Protobuf 契约返回。

GeneratedNode 不能直接修改数据库、Artifact Store 或 RunJournal。

### 7.7 撮合节点的安全边界

AI 生成的 `MatchPolicy` 不直接产生正式 `Fill`。它只返回：

```text
MatchProposal
  - order_id
  - proposed_quantity
  - proposed_price
  - queue_progress
  - reason_code
  - model_state_delta
```

Rust 仿真引擎执行：

1. 校验订单存在；
2. 校验事件时间；
3. 校验价格步长；
4. 校验剩余数量；
5. 校验市场成交量约束；
6. 校验确定性状态；
7. 生成正式 Fill 和账本事件。

### 7.8 CapabilityArtifact

AI 生成能力必须产物化：

```text
CapabilityArtifact
  - artifact_id
  - contract_id
  - contract_version
  - source_bundle
  - source_hash
  - runtime_image_digest
  - dependency_lock_hash
  - generated_by_model
  - model_version
  - prompt_hash
  - context_manifest
  - permissions
  - resource_limits
  - test_results
  - benchmark_results
  - determinism_class
  - owner
  - lifecycle_status
```

### 7.9 风险级别

| 级别 | 节点 | 默认处理 |
|---|---|---|
| R0 | 指标、因子、数据转换等纯函数 | 自动生成、测试和运行 |
| R1 | 只读研究数据的无状态节点 | 沙箱验证后自动运行 |
| R2 | 有状态模型、组合、成本和撮合节点 | 创建实验分支，强制属性测试与差异报告 |
| R3 | 发布到团队能力库 | 需要发布权限 |
| R4 | 发布正式 SignalSet | 需要发布策略和审计 |
| R5 | 订单与外部交易执行 | 不属于 ficant |

---

## 8. 系统架构

```text
┌───────────────────────────────────────────────────────────────────────┐
│                           Experience Plane                            │
│ Platform Console │ Rates Research Lab │ CGB Futures Lab │ Python SDK │
└───────────────────────────────┬───────────────────────────────────────┘
                                │ gRPC-Web / gRPC
┌───────────────────────────────▼───────────────────────────────────────┐
│                            Rust Backend                               │
│ API Gateway │ Domain Registry │ Object Registry │ Research Graph      │
│ Experiment  │ Data Gateway    │ Model Gateway   │ App Registry        │
│ Permission  │ Audit           │ Signal Registry │ Job Coordinator     │
└───────────────────────────────┬───────────────────────────────────────┘
                                │ Typed Node Protocol
┌───────────────────────────────▼───────────────────────────────────────┐
│                            Compute Plane                              │
│ Rust Native Workers │ C++ Fixed-Income Library │ Python Sandbox       │
│ Vector Compute      │ Event Simulation          │ Report Generation    │
└───────────────────────────────┬───────────────────────────────────────┘
                                │ Arrow IPC / Parquet / Protobuf
┌───────────────────────────────▼───────────────────────────────────────┐
│                              Data Plane                               │
│ PostgreSQL │ Ceph RGW │ Data Adapters │ Snapshot Store │ Evidence Ledger │
└───────────────────────────────┬───────────────────────────────────────┘
                                │
               External Database / File / Data Service / Market System
```

### 8.1 进程模型

首版采用一个 Rust Cargo Workspace，形成以下固定进程：

```text
ficant-server          控制平面和统一 API
ficant-worker          NativeNode 执行与批量任务
ficant-sandbox         GeneratedNode 沙箱调度
ficant-web             WebApp Shell 和静态资源服务
python-node-runtime    Python GeneratedNode 运行镜像
```

C++ 数值库作为共享库由 `ficant-worker` 调用，不单独形成服务。

### 8.2 模块化单体

`ficant-server` 是模块化单体，现在不拆分微服务。模块边界通过 Rust crate、数据库 Schema、Protobuf service 和内部 trait 保持清晰。

只有以下执行单元物理隔离：

- 批量计算 Worker；
- Python GeneratedNode 沙箱；
- Web 静态服务。

### 8.3 数据流

```text
外部数据源
  → Rust Data Adapter
  → Canonical RecordBatch
  → Data Quality Check
  → DataSnapshot Manifest
  → Parquet Snapshot
  → ResearchGraph
  → Artifact
```

研究代码不得直接访问外部数据源。所有正式实验只读取 DataSnapshot。

### 8.4 RunJournal 与 Evidence Ledger

每次实验记录有序事实：

```text
UserIntentRecorded
ResearchPlanCreated
ToolInvoked
SnapshotBound
GraphVersionCreated
CapabilityGenerated
ValidationCompleted
NodeStarted
NodeCompleted
ArtifactCreated
SignalPublished
RunClosed
```

事实先写入 PostgreSQL 的 append-only journal，再由投影表形成可查询状态。Artifact 使用内容哈希保存在 Ceph RGW。

### 8.5 WebApp 运行方式

每个 WebApp 是独立 React 应用，构建为静态包并由平台 App Registry 注册。

App 包含：

```text
app.yaml
web-dist/
protobuf-descriptors/
research-graph-templates/
domain-pack-dependencies/
agent-instructions/
```

WebApp 由平台 Shell 通过 iframe 加载，使用短期 App Token 调用 gRPC-Web。WebApp 不携带独立后台服务。

---

## 9. 唯一技术选型

本节定义 v0.1 的唯一实现方式。任何替换都必须经过 ADR，不在业务开发中临时增加第二套技术。

| 类别 | 唯一选择 | 用途 |
|---|---|---|
| 普通开发操作系统 | Windows 11 + PowerShell 7 | 主模型、开发 Worker、测试编写与普通测试执行 |
| 后台语言 | Rust，Edition 2024 | 控制平面、数据接入、运行时、仿真、任务和审计 |
| Rust 工具链 | `rust-toolchain.toml` 固定 stable 版本 | 保证可复现构建 |
| 异步运行时 | Tokio | 网络、任务和异步 IO |
| RPC | Protobuf 3 + tonic gRPC | 后台、Worker 和 Python SDK 的统一接口 |
| 浏览器 API | tonic-web gRPC-Web | WebApp 调用平台 |
| Web 网关 | Axum | 静态资源、健康检查和 gRPC-Web 入口 |
| 数据库访问 | SQLx | PostgreSQL 查询、事务和 Migration |
| 元数据数据库 | PostgreSQL 16 | 开发版唯一数据库 |
| 信创数据库 | openGauss | 信创改造阶段唯一目标数据库 |
| 对象存储 | Ceph RGW | 数据快照、代码包、模型和报告 Artifact |
| 内存数据格式 | Apache Arrow | Rust 与 Python 之间的列式数据交换 |
| 持久数据格式 | Apache Parquet | 不可变研究快照和大规模结果 |
| 后台序列化 | Protobuf | 领域对象、节点契约和服务接口 |
| 配置格式 | TOML | Rust 服务与开发环境配置 |
| 研究节点语言 | Python 3.12 | 用户代码和 AI GeneratedNode |
| Python 数据框 | Polars | GeneratedNode 的标准表计算接口 |
| Python 数值库 | NumPy + SciPy | 数值和统计计算 |
| Python 数据交换 | PyArrow | Arrow IPC 输入输出 |
| Python 验证 | Pydantic + pytest | 参数、输出和节点测试 |
| Python 包管理 | uv | 依赖解析、锁定和隔离安装 |
| 数值库语言 | C++20 | 固收定价、风险和性能热点 |
| C++ 编译器 | Clang 18 | Ubuntu 开发和 CI 的唯一编译器 |
| C++ 构建 | CMake + Ninja | 构建共享数值库 |
| Rust/C++ 边界 | C ABI | 稳定二进制边界，由 Rust 封装 unsafe 调用 |
| 前端语言 | TypeScript | WebApp 开发 |
| 前端框架 | React | Platform Console 和业务 WebApp |
| 前端构建 | Vite + pnpm | 前端构建和依赖锁定 |
| 图表 | Apache ECharts | 金融图表和研究可视化 |
| 本地 SIT 编排 | Windows Docker Desktop + Docker Compose | 阶段特定集成服务启动和测试 |
| AI 代码隔离 | gVisor OCI Sandbox | GeneratedNode 安全运行 |
| 任务队列 | PostgreSQL Lease Queue | 基于事务和 `SKIP LOCKED` 的任务领取 |
| 工作流 | Rust Research State Machine | 实验状态、恢复、补偿和发布流程 |
| 日志 | Rust `tracing` 结构化日志 | 服务和节点日志 |
| 可观测协议 | OpenTelemetry OTLP | Trace、Metric 和 Log 关联 |
| 身份协议 | OIDC | 用户身份接入 |
| 权限模型 | RBAC + ABAC | 对象、工具、数据和发布权限 |
| 内容哈希 | SHA-256 | 快照、代码、环境和 Artifact 标识 |
| 全局 ID | ULID 字符串 | 有序、跨进程对象标识 |

### 9.1 Rust 后台约束

后台业务代码只使用 Rust。不得新增第二种后台服务语言。

Rust Workspace 内部按 crate 分层：

```text
api
application
domain
infrastructure
runtime
simulation
ai
sandbox
signal
app_registry
```

依赖方向固定为：

```text
api → application → domain
infrastructure → domain
runtime → domain
simulation → domain
ai → application
sandbox → runtime
```

`domain` crate 不依赖数据库、网络、文件系统、模型服务和 Web 框架。

### 9.2 C++ 使用边界

C++ 只用于：

- 固收定价；
- 收益率反解；
- 曲线算法；
- 风险量；
- 经过性能分析确认的热点计算。

C++ 不实现权限、实验、数据血缘、API、任务调度和业务状态机。

所有 C++ 接口通过 C ABI 暴露，Rust 为每个接口提供安全封装、边界检查、Golden Case 和数值误差测试。

Application 只能依赖领域化计算 port，具体数值 provider 通过独立 adapter 在 composition root 显式注入；FFI unsafe 只能存在于唯一 sys crate，平台派生计算结果不得冒充外部市场事实。数值边界见 [ADR-0002](docs/architecture/adr/0002-fixed-income-kernel-and-ffi-safety-boundary.md)，全局模块原则见 [ADR-0003](docs/architecture/adr/0003-deep-modules-and-explicit-internal-boundaries.md)。

### 9.3 Python 使用边界

Python 只用于：

- Python SDK；
- 用户研究代码；
- AI GeneratedNode；
- 模型训练和推断节点；
- 报告中的研究型计算。

Python 不实现平台控制平面，不直接访问 PostgreSQL，不直接访问 Ceph RGW，不持有平台密钥。

### 9.4 API 契约

Protobuf 是唯一契约源。

同一 `.proto` 生成：

- Rust 服务类型；
- Python SDK 类型；
- TypeScript Web 类型；
- Domain Pack 动态描述符；
- Agent Tool 参数定义。

不得手工维护重复的 Rust struct、Python class 和 TypeScript interface 作为跨边界契约。

### 9.5 数据存储分工

PostgreSQL 保存：

- 用户、权限和应用注册；
- 领域对象元数据；
- Definition、Run 和 Artifact 索引；
- ResearchGraph；
- RunJournal；
- 任务队列；
- 审批和发布记录。

Ceph RGW 保存：

- Parquet 数据快照；
- Arrow IPC 中间包；
- 模型文件；
- GeneratedNode 源码包；
- 测试结果；
- 报告；
- 大型仿真结果。

PostgreSQL 不保存大规模行情矩阵和因子矩阵。

### 9.6 数据库使用规范

为后续迁移 openGauss，开发阶段即遵守：

- 不使用 PostgreSQL Extension；
- 不使用数据库存储过程承载领域逻辑；
- 不使用触发器实现核心状态机；
- 不使用厂商专有全文检索；
- SQL 集中在 SQLx query 和 migration 中；
- Repository 与 SQL 方言隔离；
- 金额、价格和利率使用定点 Decimal；
- 时间统一存储 UTC，并单独保存市场时区；
- 对象 ID 使用 ULID 字符串；
- 大字段和大矩阵进入 Ceph RGW；
- Migration 必须具备前向升级和数据校验脚本。

---

## 10. 开发环境与信创路径

### 10.1 当前开发基线

v0.1 的普通开发与测试在 Windows 11 上使用 PowerShell 7、Windows Git/worktree
和 Windows 路径。能力按任务和阶段检查，不把 SIT、发布或 Linux 工具链作为
普通开发的统一准入条件：

```text
Windows 11 + PowerShell 7
Rust stable，版本由 rust-toolchain.toml 固定
Python 3.12
VS LLVM 19.1.5（C++/ABI 主工具链）+ standalone Clang 18（回退）+ CMake + Ninja
PostgreSQL 16
Ceph RGW
Docker Desktop + Docker Compose（仅在进入本地 SIT 时）
gVisor
Node.js 22 + pnpm
```

Docker Desktop 的 PostgreSQL/Ceph RGW 隔离环境由 Orchestrator 的 delivery work 在 SIT 阶段管理；
Human 负责 Docker Desktop GUI、启动和管理员操作。命令从 Windows 执行，不要求 WSL Integration。Linux 只在明确命名的兼容、CI/container、
发布或 UAT 门禁中验证；仓库现存 WSL runner/config/evidence 是历史来源，在另行退役前保留。

首版不把国产 CPU、国产操作系统和国产容器平台纳入日常开发阻塞项，也不以其作为 v0.1 发布条件。

### 10.2 从第一天保留的信创约束

虽然首版不在信创服务器上开发，代码必须从第一天遵守以下约束：

- Rust 核心不依赖 x86 专用汇编；
- 核心算法不依赖特定 CPU SIMD；
- C++ 使用标准 C++20 和 C ABI；
- 构建不依赖公网；
- Cargo、Python、pnpm 和系统包均可镜像和离线归档；
- 数据库逻辑不依赖 PostgreSQL Extension；
- Protobuf 契约与 CPU、OS 和数据库无关；
- Artifact 格式只使用 Arrow、Parquet 和 Protobuf；
- 容器镜像构建过程可重放；
- 所有第三方依赖生成 SBOM 和许可证清单；
- 数值算法具备跨平台 Golden Case；
- 浮点比较使用明确误差标准，不依赖位级完全一致。

### 10.3 信创改造阶段

信创适配作为独立工程阶段执行，不在业务功能开发中零散兼容。

固定步骤：

1. 确定目标国产 CPU、国产操作系统和编译工具链；
2. 在目标环境构建 Rust、C++ 和 Python 运行镜像；
3. 将 PostgreSQL 迁移到 openGauss；
4. 完成 SQL Migration、事务、索引和并发行为测试；
5. 重新构建全部 Python Wheel 和 C++ 共享库；
6. 验证 gVisor 可用性，并完成沙箱运行时适配；
7. 执行固定收益 Golden Case、回放测试和性能基线；
8. 生成目标环境 SBOM、兼容报告和离线安装包。

### 10.4 PostgreSQL 到 openGauss

openGauss 是信创改造阶段唯一目标数据库。

迁移策略：

```text
PostgreSQL 16 schema
  → SQL compatibility scan
  → openGauss migration scripts
  → repository conformance tests
  → transaction and lock tests
  → data reconciliation
  → cutover rehearsal
```

平台不假设 PostgreSQL 与 openGauss 完全等价。迁移完成的标准是：

- 全部 SQLx Repository 测试通过；
- 所有 Migration 在空库和存量库通过；
- 任务领取、幂等、锁和事务语义通过；
- RunJournal 顺序和唯一性约束通过；
- 数据校验结果一致；
- 固定收益 Golden Case 结果在约定误差内一致。

---

## 11. 首批实现顺序

首批建设以一条完整的国债研究闭环驱动平台抽象，不先建设无业务验证的通用框架。

### Phase 0：仓库和契约基线

**目标：** 冻结唯一技术栈和代码组织方式。

交付：

- Rust Cargo Workspace；
- `rust-toolchain.toml`；
- Protobuf 契约仓库；
- PostgreSQL 16 Migration；
- Windows Docker Desktop 阶段特定 SIT 环境；
- S3 Bucket 规范；
- Python GeneratedNode 基础镜像；
- React Platform Shell；
- CI、格式化、静态检查和依赖审计；
- ADR 模板。

退出条件：

- 一条命令启动完整开发环境；
- Rust、Python、C++ 和 Web 构建全部可重放；
- Protobuf 可生成 Rust、Python 和 TypeScript 类型；
- PostgreSQL Migration 可从空库执行。

### Phase 1：领域内核

**目标：** 建立稳定的市场事实和研究资产模型。

优先实现：

- Instrument；
- Bond；
- FuturesContract；
- Cashflow；
- Calendar；
- Unit；
- Quote；
- Trade；
- Valuation；
- CurveSnapshot；
- MarketRulePack；
- DataSnapshot；
- UniverseSnapshot；
- ExperimentRun；
- Artifact；
- SignalSet；
- RunJournal。

退出条件：

- 核心对象可创建、查询、版本化和追踪；
- 领域对象具备 Protobuf 契约和 PostgreSQL 映射；
- 历史对象不可被后续修改覆盖。

### Phase 2：固定收益参考数值库

**目标：** 建立平台权威基准，供 AI 节点和研究实现比较。

**当前状态（2026-07-19）：** Phase 2A 已交付固定利率/贴现国债的现金流、价格、YTM、久期、凸性与 DV01；Phase 2B 已交付区间内实际日数线性 YTM 曲线和未融资 Carry/Roll-down 分解；Phase 2C 已交付中金所 `TS`/`TF`/`T`/`TL` 合约与可交割券资格、CF、交割发票价、基差、含融资成本净基差、IRR 和 CTD。三个切片均贯通独立 Oracle、确定性 Arrow 与真实 PostgreSQL/Ceph RGW 发布重放。Phase 2 尚余期现套保比例；Phase 2C 验收口径见 [iteration brief](docs/iterations/2026-07-phase2c-futures-delivery.md)。

优先实现：

- 债券现金流；
- 计息和应计利息；
- 净价、全价；
- 到期收益率；
- 久期、修正久期、凸性、DV01；
- 曲线节点和插值；
- Carry 和 Roll-down；
- 国债期货合约；
- 可交割券；
- 转换因子；
- 基差、净基差、IRR；
- CTD；
- 期现套保比例。

实现方式：

- Rust 负责类型、校验和调用；
- C++20 负责数值算法；
- C ABI 固定边界；
- Golden Case 固定输入和期望结果。

退出条件：

- 所有参考算法有边界测试；
- Rust/C++ 边界有内存和异常测试；
- Python SDK 调用结果与参考结果一致；
- 每个结果绑定规则版本和输入快照。

### Phase 3：数据适配与不可变快照

**目标：** 从异构输入形成可复现研究数据。

优先实现：

- DataSource 注册；
- PostgreSQL 数据源适配；
- 文件数据源适配；
- Instrument 映射；
- 时间和交易日历；
- 数据可见时间；
- 点时查询；
- 数据质量规则；
- Arrow RecordBatch；
- Parquet Snapshot；
- Snapshot Manifest；
- 内容哈希和来源血缘。

退出条件：

- 至少两个输入来源可形成同一 Canonical Schema；
- 实验绑定快照后不再访问外部数据源；
- 快照可通过 Manifest 完整校验和重读。

### Phase 4：ResearchGraph 与实验运行时

**目标：** 将研究从脚本升级为强类型、可重放实验。

优先实现：

- ResearchNodeContract；
- ResearchGraph；
- NativeNode；
- 节点输入输出校验；
- Rust Research State Machine；
- PostgreSQL Lease Queue；
- RunJournal；
- Artifact 管理；
- 环境摘要和随机种子；
- 中断恢复；
- 实验比较。

退出条件：

- 同一快照、图、参数、代码和种子得到相同结果；
- 实验中断后可从安全点恢复；
- 任意输出可追踪到每个节点。

### Phase 5：Rates Research Lab

**目标：** 交付第一条可用的国债现券研究体验。

功能：

- 国债搜索和筛选；
- 现金流查看；
- 估值与收益率；
- 曲线构建；
- Carry 和 Roll-down；
- 久期、DV01 和关键期限风险；
- 相对价值；
- 研究图查看；
- 实验对比；
- 报告导出。

退出条件：

- WebApp 只通过 gRPC-Web 使用平台能力；
- 不直接访问数据库和对象存储；
- 所有分析绑定 DataSnapshot 和 MarketRulePack。

### Phase 6：国债期货与市场仿真

**目标：** 完成现券与期货的统一研究闭环。

功能：

- 期货合约与交割篮子；
- 转换因子；
- 基差、净基差和 IRR；
- CTD；
- 交割情景；
- 期现套保；
- 曲线风险对冲；
- 移仓与展期；
- FuturesOrderBookSimulation；
- HistoricalTradeReplay；
- MatchPolicy、CostPolicy 和 SlippagePolicy。

退出条件：

- 仿真事件严格有序；
- 成交满足数量、价格和时序不变量；
- 仿真结果可重放；
- 形成 CGB Futures Lab。

### Phase 7：AI 基础设施

**目标：** 让 AI 通过强类型工具理解和操作平台。

优先实现：

- ModelGateway；
- Tool Registry；
- ResearchPlan；
- ResearchPatchSpec；
- 结构化模型输出；
- Prompt、Model 和 Context 版本；
- Agent RunJournal；
- 对象语义检索；
- 代码生成；
- 测试生成；
- 结果解释和实验比较。

退出条件：

- 每次模型调用和工具调用可审计；
- AI 只能通过平台 Tool 访问对象；
- AI 无法直接访问数据库、Ceph RGW 和密钥。

### Phase 8：GeneratedNode 与实时研究补丁

**目标：** 实现用户在实验过程中临时改变研究节点。

按顺序开放：

1. DataTransform；
2. Factor；
3. CurveSelectionPolicy；
4. CurveInterpolationPolicy；
5. CostPolicy；
6. PortfolioHedgePolicy；
7. MatchPolicy；
8. 完整 ResearchGraph 组合。

退出条件：

- AI 生成 Python 包可在 gVisor 中运行；
- 每个节点具备 CapabilityArtifact；
- R2 节点通过属性测试和差异报告；
- 补丁只产生新的 ResearchGraph 版本；
- 历史实验不受模型升级影响。

### Phase 9：SignalSet 发布

**目标：** 建立清晰研究终点。

功能：

- SignalSet Registry；
- TargetExposure；
- 信号有效期；
- 数据和规则版本；
- 发布权限；
- 订阅、导出和告警；
- 下游回执作为分析输入。

退出条件：

- SignalSet 与订单严格分离；
- 每个正式信号有完整血缘；
- 下游系统不能反向修改历史研究产物。



---

## 12. v0.1 最小可交付范围

v0.1 必须形成以下纵向链路：

```text
数据接入
  → 点时快照
  → 债券现金流
  → 曲线
  → 估值与风险
  → 国债期货与 CTD
  → 组合与套保
  → 回测与仿真
  → SignalSet
  → 证据与血缘
```

v0.1 验收项：

1. Windows 11 + PowerShell 7 可执行普通开发与测试；需要集成服务时由 Windows Docker Desktop 启动阶段特定 SIT 环境；
2. 后台代码全部为 Rust；
3. PostgreSQL 16 是唯一元数据数据库；
4. 至少两个异构输入来源可创建统一快照；
5. 能完成国债定价、收益率和风险计算；
6. 能完成基差、IRR、CTD 和套保研究；
7. 能保存、恢复和重放完整实验；
8. Rates Research Lab 和 CGB Futures Lab 使用同一平台对象；
9. AI 能生成一个无状态 GeneratedNode；
10. AI 能生成一个有状态 MatchPolicy；
11. GeneratedNode 通过测试、沙箱和 Artifact 化；
12. 所有正式结果具有完整数据、代码、环境和规则血缘；
13. 平台不包含订单管理、报单、清算和结算功能；
14. 信创硬件和 openGauss 不属于 v0.1 阻塞项。

---

## 13. 仓库结构

当前权威结构如下。目录按语言、构建系统和所有权边界组织，不设置统一的根 `src/`；详细依据见 [ADR-0001](docs/architecture/adr/0001-polyglot-monorepo-source-ownership.md)。

```text
ficant/
├── README.md
├── Cargo.toml / Cargo.lock         # Rust workspace 与依赖锁
├── rust-toolchain.toml             # 固定 Rust 工具链
├── interface/                      # 唯一 Protobuf 契约源及生成配置
│   └── proto/ficant/{core,market,research,app}/
├── crates/                         # Rust 库；各 crate 自有 src/ 与 tests/
│   ├── ficant-domain/
│   ├── ficant-application/
│   ├── ficant-api/
│   ├── ficant-storage/
│   ├── ficant-runtime/
│   ├── ficant-contracts/
│   ├── ficant-contract-tests/
│   └── ficant-acceptance/
├── binaries/                       # Rust composition roots
│   ├── ficant-bootstrap/
│   ├── ficant-server/
│   ├── ficant-worker/
│   └── ficant-web/
├── cpp/                            # C++20 数值库
│   └── fixed-income-kernel/
│       └── {include,src,tests}/
├── python/                         # 节点运行时与生成契约，不含控制平面
│   ├── node-runtime/
│   ├── node-contracts/
│   └── tests/
├── web-dm/                         # pnpm workspace 与全部 WebApp
│   ├── platform-shell/
│   ├── packages/contracts-generated/
│   └── webapps/dmquant/
├── migrations/postgresql/
├── deploy/dev/                    # Compose、镜像与固定工具链
├── tests/golden-cases/            # 跨语言业务基准
├── docs/{product,architecture,interface,quality,delivery,review}/
└── .github/scripts/               # CI、供应链、Compose 与发布门禁
```

规划中的 `ficant-data`、`ficant-research`、simulation/AI/sandbox crates、`domain-packs/`、Python SDK、Rates Research Lab、CGB Futures Lab、openGauss migration 与信创部署目录，仅在对应 Phase 进入并通过设计确认后创建。根 `proto/` 永久禁止；跨边界契约只在 `interface/` 定义。

---

## 14. 非功能要求

### 14.1 可复现

- DataSnapshot 不可变；
- Artifact 不可变；
- 代码包具有 SHA-256；
- 运行镜像具有 digest；
- Python 依赖具有 lock hash；
- 随机数由平台注入种子；
- 市场规则带版本；
- 实验可重放；
- 模型升级不改变历史运行。

### 14.2 正确性

- 固定收益算法具有 Golden Case；
- 市场规则具有生效日期；
- 撮合满足数量、价格、时间和状态不变量；
- 数据处理满足点时性；
- 所有 Decimal 单位明确；
- 跨语言边界具备误差和内存测试。

### 14.3 安全

- 最小权限；
- OIDC 身份；
- RBAC + ABAC；
- AI 代码禁网；
- 数据库和对象存储密钥不进入沙箱；
- gVisor 隔离；
- 依赖白名单；
- SBOM；
- 模型调用、工具调用和发布操作审计。

### 14.4 可观测

每个请求和实验统一关联：

```text
trace_id
experiment_run_id
agent_run_id
node_run_id
artifact_id
user_id
app_id
```

Rust 使用 `tracing` 产生结构化事件，通过 OTLP 输出。

### 14.5 性能

首版优先保证研究吞吐、批量计算和交互式分析，不追求实盘极限延迟。

性能优化顺序固定为：

1. 完成正确性和可复现性测试；
2. 使用基准定位瓶颈；
3. 优化 Rust 数据流和内存分配；
4. 将稳定数值热点移入 C++20；
5. 保持 Protobuf 契约和 Artifact 语义不变。

### 14.6 可维护

- Rust crate 边界清晰；
- Protobuf 是唯一契约源；
- SQLx Migration 是唯一数据库变更入口；
- Domain Pack 是唯一领域扩展入口；
- CapabilityArtifact 是唯一动态研究能力入口；
- WebApp 不携带后台服务；
- 技术变更必须有 ADR。

---

## 15. 首版明确不建设

为了防止平台边界弥漫，v0.1 不建设：

- 微服务体系；
- 通用企业数据湖；
- 通用 ETL 产品；
- 通用 BPM；
- 通用低代码平台；
- 自研数据库；
- 自研对象存储；
- 自研消息队列；
- 自研容器编排；
- 通用向量数据库；
- 通用大模型训练平台；
- 多后台语言体系；
- 多数据库并行适配；
- 原生插件动态加载；
- OMS、EMS、交易柜台和清算系统。

---

## 16. 不可偏离的技术决策

| 决策 | 结论 |
|---|---|
| 产品定位 | AI 原生量化研究操作系统 |
| 首个市场 | 中国国债现券与国债期货 |
| 后台语言 | Rust |
| 数值库 | C++20，通过 C ABI 接入 Rust |
| 研究代码 | Python 3.12 GeneratedNode |
| 前端 | TypeScript + React |
| 契约 | Protobuf 3 |
| API | gRPC + gRPC-Web |
| 普通开发 OS | Windows 11 + PowerShell 7 |
| 开发数据库 | PostgreSQL 16 |
| 信创数据库 | openGauss |
| 对象存储 | Ceph RGW |
| 数据格式 | Arrow + Parquet |
| 控制平面 | Rust 模块化单体 |
| 任务执行 | Rust Worker + PostgreSQL Lease Queue |
| 工作流 | Rust Research State Machine |
| AI 沙箱 | gVisor OCI Sandbox |
| WebApp 扩展 | 静态 React App + Domain Pack + CapabilityArtifact |
| 正式输出 | SignalSet / TargetExposure |
| 交易执行 | 不属于 ficant |
| 信创适配 | 在 v0.1 业务闭环后独立实施 |

---

## 17. 当前优先事项

```text
1. 冻结 Protobuf 核心契约
2. 建立 Rust Cargo Workspace，并在进入 SIT 时准备 Windows Docker Desktop Compose 环境
3. 完成 PostgreSQL 16 元数据模型与 RunJournal
4. 实现国债现金流、定价和风险参考库
5. 实现 DataSnapshot 和 Parquet 快照
6. 实现 ResearchGraph 与 Rust 确定性运行时
7. 打通 Rates Research Lab
8. 完成国债期货、CTD、套保和市场仿真
9. 引入 AI Tool、GeneratedNode 和 gVisor 沙箱
10. 完成 SignalSet 发布边界
```

项目采用 MIT License；贡献流程、发布节奏和运维手册将随公开协作需要继续完善。
