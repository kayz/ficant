# ficant 产品范围

**状态：** 当前产品与范围基线  
**实现状态：** 设计与治理阶段；没有生产源码或已验证产品行为  
**来源：** `README.md`、`UI-DM/`、iteration-1 Product 评审

## 产品与用户

ficant 是面向专业投资研究团队的 AI 原生量化研究平台。它连接外部数据源，把数据、领域知识、研究方法、因子、模型、策略、仿真和信号组织为可版本、可追踪、可复现的研究资产，并通过 WebApp、Python SDK 和 AI Agent 提供能力。

首个体验资料体现两类用户：

- **researcher：** 表达研究意图、生成与修改策略、提交回测、检查证据并发布受治理的研究产物。
- **viewer：** 只读查看已授权的研究与结果，不能提交回测；导出、下载、编辑和删除仍须由平台 RBAC/ABAC 决定，不能只依赖前端隐藏控件。

## 正式产品终点

ficant 的正式输出是：

- `ResearchArtifact`
- `SimulationResult`
- `ReportArtifact`
- `SignalSet`
- `TargetExposure`

平台不拥有订单和外部交易执行，不建设 OMS、EMS、对外报单、清算、结算或投资组合会计。

## 首个市场与范围

首个市场是中国国债现券与国债期货，覆盖国债现金流、定价与风险、收益率曲线、资金成本、可交割券、转换因子、基差/IRR/CTD、套保、曲线风险、仿真、回测、归因与信号。

首版不覆盖完整信用债、ABS、可转债、利率互换生命周期、一级发行、真实询价通讯和真实清算交割。

## 核心业务概念

- **Definition / Run / Artifact：** 定义版本化，运行不覆盖，正式产物不可变。
- **DataSnapshot：** 冻结研究所见数据、来源、版本与点时可见性。
- **ResearchGraph：** 以强类型 DAG 表达研究过程。
- **CapabilityArtifact：** 把经校验的生成式研究能力保存为可追溯资产。
- **ExperimentRun / RunJournal：** 记录确定性执行和完整证据。
- **SignalSet / TargetExposure：** 平台对下游的治理终点。

## WebApp 边界

WebApp 可以定义独立研究体验，但不能重新定义平台事实。它必须复用平台身份权限、领域对象、数据快照、ResearchGraph、实验、证据账本、AI 工具和信号发布机制；不得自建后台、直连外部数据库、绕过快照/研究图或发布无血缘信号。

## DMQuant 首个体验

DMQuant 当前是首个 WebApp 的目标体验设计，不是已实现产品。其预期业务闭环是：

```text
研究员表达意图
→ AI 生成草稿与检查/试跑信息
→ 用户应用或修改参数并保存策略版本
→ 幂等提交异步回测
→ 查看运行状态、指标、序列、校验和复现指纹
→ 查看/下载有权限的策略源码与成功运行产物
→ 编辑后生成新版本并重新回测
```

完整验收必须同时覆盖成功、缓存、失败、警告、只读权限、审计动作和可复现性。静态 HTML 中的写死数据与交互不是行为证据。

## 事实、假设与待决策项

- **事实：** 当前仓库没有生产源码或运行时；`README.md` 是唯一系统技术基线。
- **设计要求：** DMQuant 通过平台 Protobuf 生成契约和 gRPC-Web 使用能力，不能建立平行 OpenAPI/REST 后台体系。
- **未验证来源：** UI-DM 中出现的 `experiences/dm-ai/`、hooks、API client、生成类型和后端方法名可能源于外部/先前设计，也可能仅是目标名称；当前仓库无法证明其实现存在。
- **实施前人类决策：** 明确 DMQuant 与 Rates Research Lab、CGB Futures Lab 的产品层级、路线阶段及首个必须闭环的研究场景。

## Validity

Valid: long-term until superseded
