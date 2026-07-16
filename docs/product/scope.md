# ficant 产品范围

**状态：** Phase 0 / Phase 1 当前实现范围

**实现状态：** 已形成可运行候选；最终验收由 Human 基于 Orchestrator 的确定性证据、Quality test report 与最终一致性 Audit 决定

**来源：** `README.md`、`interface/`、`web-dm/` 与当前生产实现

## 产品定位与终点

ficant 是面向专业投资研究团队的 AI 原生量化研究平台。平台把数据、领域知识、研究方法、运行和结果组织为可版本化、可追踪、可复现的研究资产，并通过统一后台合同向 WebApp、Python SDK 和 Agent 提供能力。

正式产品终点保持为 `ResearchArtifact`、`SimulationResult`、`ReportArtifact`、`SignalSet` 和 `TargetExposure`。平台不拥有订单和外部交易执行，不建设 OMS、EMS、对外报单、清算、结算或投资组合会计。

首个市场仍是中国国债现券与国债期货；但 iteration-2 只交付 Phase 0 仓库/合同基线和 Phase 1 领域内核，不把后续固定收益算法或完整研究产品页面包装成已实现能力。

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

## 明确不在本轮范围

- Phase 2 的现金流生成、应计利息、净价/全价、收益率、久期、DV01、曲线、基差、IRR、CTD 与套保算法；当前 C++ 仅证明固定工具链、可重放构建和 ABI 边界。
- 完整 DMQuant 策略生成、回测、Artifact 浏览、多 run 比较及静态原型中的高级分析页面。
- openGauss 迁移、GeneratedNode/gVisor 业务运行、OMS/EMS 和任何外部交易执行。
- 信用债、ABS、可转债、完整利率互换生命周期、真实询价通讯与清算交割。

## Validity

Valid: long-term until superseded
