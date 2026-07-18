# ficant 产品范围

**状态：** Phase 0 / Phase 1 已完成；iteration-3 已交付 Phase 2A 固定收益纵向切片，其余 Phase 2 与 Phase 3+ 仍为后续范围

**实现状态：** 当前能力以已合并代码、冻结合同和可重放本地证据为准，不把局部纵向切片扩写为完整 Phase 或最终产品

**来源：** `README.md`、`interface/`、`web-dm/` 与当前生产实现

## 产品定位与终点

ficant 是面向专业投资研究团队的 AI 原生量化研究平台。平台把数据、领域知识、研究方法、运行和结果组织为可版本化、可追踪、可复现的研究资产，并通过统一后台合同向 WebApp、Python SDK 和 Agent 提供能力。

正式产品终点保持为 `ResearchArtifact`、`SimulationResult`、`ReportArtifact`、`SignalSet` 和 `TargetExposure`。平台不拥有订单和外部交易执行，不建设 OMS、EMS、对外报单、清算、结算或投资组合会计。

首个市场仍是中国国债现券与国债期货。当前已完成 Phase 0 仓库/合同基线和 Phase 1 领域内核，并在 iteration-3 交付小范围 Phase 2A 国债分析纵向切片；这不表示完整 Phase 2、外部数据适配、完整研究产品页面或 Phase 3+ 已实现。

## Phase 0 已落地边界

- Rust Workspace 是唯一后台实现；Python 只承担生成节点运行时/合同消费，C++20 只保留稳定 C ABI 数值库边界。
- `interface/` 是后台 Protobuf 唯一来源，并生成 Rust、Python、TypeScript consumer；不建立平行 REST/OpenAPI DTO。
- PostgreSQL Migration、MinIO 内容寻址对象、开发 Compose、固定工具链和多语言构建已进入发布候选。
- React Platform Shell 已实现真实 Rust gRPC-Web 路径、会话、应用目录和短期应用启动授权。
- 多 WebApp 的页面设计、代码和测试统一位于 `web-dm/`；后台接口设计保留在根 `interface/`，避免未来 WebApp 各自复制后台合同。

## Phase 1 已落地业务切片

当前实现覆盖 README 列出的 17 个核心对象，并完成一条真实业务链：

```text
市场事实
→ DataSnapshot / UniverseSnapshot
→ ExperimentRun
→ Artifact / SignalSet
→ RunJournal
→ 重启后 required read 与确定性重放
```

这条链路使用真实 PostgreSQL 与 MinIO，约束租户/所有者、精确版本、单位、规则生效时间、内容哈希、大小、血缘、幂等、并发和不可变性。已发布内容的正式读取是 required read：metadata 存在而对象缺失、哈希漂移或大小漂移属于完整性损失，不会被解释为“未找到”。

`Artifact` 与 `SignalSet` 是不同身份的根对象；`SignalSet` 通过内容寻址引用真实 Artifact，并与 Snapshot、Run、RulePack 和输入产物形成可复核血缘。平台输出仍然是信号和研究证据，不是订单。

## Iteration 3 / Phase 2A 已落地固定收益切片

当前实现覆盖固定利率和贴现国债的现金流、应计利息、净价、全价、到期收益率（YTM）、麦考利久期、修正久期、凸性和 DV01。该切片已经贯通：

```text
C++20 固定收益内核
→ 稳定 C ABI
→ 安全 Rust adapter 与应用用例
→ 确定性 Arrow Artifact
→ PostgreSQL / MinIO stage、校验、发布、读取与重放
```

平台生成的现金流、估值和风险结果保持内部 `BondAnalyticsResult` / Artifact 语义，不写成外部来源的 `Cashflow` 或 `Valuation` 事实。该内部闭环没有实现曲线、Carry/Roll-down、国债期货数值、转换因子（CF）、基差、隐含回购利率（IRR）、最便宜可交割券（CTD）或套保算法，也没有实现外部市场数据源适配。

## WebApp 产品边界

当前可用产品界面是 Platform Shell，不是完整 DMQuant：

- 从服务端读取当前会话和可见应用，不从前端角色字符串推导最终授权；
- 通过短期授权加载 iframe 应用，校验精确 origin、entrypoint、capability、CSP、sandbox 和有效期；
- 区分会话、目录、授权、不可用、拒绝、加载失败等状态，并展示安全错误码、`trace_id` 与允许的恢复动作；
- token 不进入 URL 或 `localStorage`，应用退出、过期和边界失败会触发撤权。

`web-dm/webapps/dmquant/design.md` 仍描述首个业务 WebApp 的后续目标。AI 草稿、策略版本、异步回测、指标/产物浏览和编辑重跑尚未实现，不得用 Platform Shell 或静态原型冒充该业务闭环。

## 多 WebApp 目录约定

```text
web-dm/
├── platform-shell/                 # 共享宿主、会话、Registry 与 iframe 边界
├── packages/contracts-generated/   # 从根 interface/ 机械生成
└── webapps/<app-id>/                # 每个 WebApp 的中文设计、源码与测试

interface/                           # 所有 WebApp 共用的后台 Protobuf 合同
```

WebApp 可以定义独立研究体验，但不能自建身份权限、直连外部数据库、绕过 Snapshot/ResearchGraph、覆盖共享事实或携带平行后台。

## 明确尚未实现与后续范围

- Phase 2 剩余的曲线、Carry/Roll-down、国债期货数值、CF、基差、IRR、CTD 与套保算法。
- Phase 3 的外部数据源适配、采集与快照数据平台；当前 Snapshot 领域对象和内部 Artifact 闭环不能视为外部数据接入。
- 完整 DMQuant 业务 WebApp，包括策略生成、回测、Artifact 浏览、多 run 比较及静态原型中的高级分析页面；当前 UI 仍仅为 Platform Shell。
- openGauss 迁移、GeneratedNode/gVisor 业务运行、OMS/EMS 和任何外部交易执行。
- 信用债、ABS、可转债、完整利率互换生命周期、真实询价通讯与清算交割。
- README Phase 4–9 的 ResearchGraph 运行时、研究 Lab、仿真、AI 基础设施和后续发布流程仍为规划能力，不得描述为当前已完成。

## Validity

Valid: long-term until superseded
