# ficant 产品范围

**状态：** 当前候选已具备 Phase 0 开发环境/migration 合同与 Phase 4 持久化执行闭环，并通过本地多语言及真实 PostgreSQL + Ceph RGW 验证；正式镜像冷构建和测试环境交付待 version Action 闭合；对象存储统一为 Ceph RGW + Apache `object_store`

**实现状态：** 当前能力以冻结合同、当前候选代码和已记录的真实本地证据为准；尚未运行的集成命令不得写成通过，不把局部纵向切片扩写为最终产品

**来源：** `README.md`、`interface/`、`web-dm/` 与当前生产实现

## 产品定位与终点

ficant 是面向专业投资研究团队的 AI 原生量化研究平台。平台把数据、领域知识、研究方法、运行和结果组织为可版本化、可追踪、可复现的研究资产，并通过统一后台合同向 WebApp、Python SDK 和 Agent 提供能力。

正式产品终点保持为 `ResearchArtifact`、`SimulationResult`、`ReportArtifact`、`SignalSet` 和 `TargetExposure`。平台不拥有订单和外部交易执行，不建设 OMS、EMS、对外报单、清算、结算或投资组合会计。

首个市场仍是中国国债现券与国债期货。Phase 1 领域内核、Phase 2 固定收益参考数值库和 Phase 3 可复现数据链已有各自冻结证据；Python SDK 通过同一 Protobuf/gRPC 合同调用真实 Rust/C++ 生产路径，文件与 PostgreSQL adapter 把外部报价转换为同一 Canonical Arrow Schema 后发布为可校验、可脱离外源重读的不可变 Parquet `DataSnapshot`。当前候选补齐了 Phase 4 持久化执行路径，但仍不表示完整研究产品页面、GeneratedNode 或 Phase 5 业务体验已经实现。

## Phase 0 已落地边界

- Rust Workspace 是唯一后台实现；Python 只承担生成节点运行时/合同消费，C++20 只保留稳定 C ABI 数值库边界。
- `interface/` 是后台 Protobuf 唯一来源，并生成 Rust、Python、TypeScript consumer；不建立平行 REST/OpenAPI DTO。
- PostgreSQL Migration、Ceph RGW 内容寻址对象、开发 Compose、固定工具链和多语言构建有冻结合同；`scripts/dev-up.ps1` 构建并启动七个服务，从真实 Worker 镜像派生 runtime/source identity，并通过 React UI 反代验证已认证的 gRPC-Web Session。
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

这条链路使用真实 PostgreSQL 与 Ceph RGW，约束租户/所有者、精确版本、单位、规则生效时间、内容哈希、大小、血缘、幂等、并发和不可变性。已发布内容的正式读取是 required read：metadata 存在而对象缺失、哈希漂移或大小漂移属于完整性损失，不会被解释为“未找到”。

`Artifact` 与 `SignalSet` 是不同身份的根对象；`SignalSet` 通过内容寻址引用真实 Artifact，并与 Snapshot、Run、RulePack 和输入产物形成可复核血缘。平台输出仍然是信号和研究证据，不是订单。

## Iteration 3 / Phase 2A 已落地固定收益切片

当前实现覆盖固定利率和贴现国债的现金流、应计利息、净价、全价、到期收益率（YTM）、麦考利久期、修正久期、凸性和 DV01。该切片已经贯通：

```text
C++20 固定收益内核
→ 稳定 C ABI
→ 安全 Rust adapter 与应用用例
→ 确定性 Arrow Artifact
→ PostgreSQL / Ceph RGW stage、校验、发布、读取与重放
```

平台生成的现金流、估值和风险结果保持内部 `BondAnalyticsResult` / Artifact 语义，不写成外部来源的 `Cashflow` 或 `Valuation` 事实。Phase 2A 本身不包含曲线或持有期分解；这些能力由下述 Phase 2B 切片补充。

## 2026-07 / Phase 2B 已落地曲线与 Carry/Roll-down

当前实现新增 CFETS 风格的“剩余期限—到期收益率”参考曲线：精确节点原值返回，节点间按实际日数线性插值，节点范围外 fail closed，不隐式外推。持有期结果固定为：

```text
carry = horizon_dirty_at_initial_yield + paid_cashflows - initial_dirty
roll_down = horizon_dirty_at_rolled_curve_yield - horizon_dirty_at_initial_yield
total_return = carry + roll_down
```

固定利率债与贴现债已贯通 Rust 领域校验、C++20、加法式 C ABI、安全 Rust adapter、独立 Decimal/QuantLib 1.42.1 Oracle、确定性 Arrow，以及真实 PostgreSQL 16 + Ceph RGW 发布、重启重放和篡改检测。该结果不包含融资成本、税费、交易成本或现金流再投资，也不是 bootstrap 后的无套利即期/远期曲线。

## 2026-07 / Phase 2C 已落地国债期货交割价值链

当前实现以精确版本的 `cgb-futures` RulePack 解析中金所 `TS`、`TF`、`T`、`TL` 的期限资格、交割月份、票息、百元面值基准、舍入和年化日基准。RulePack 内容按确定性 Protobuf bytes 做 SHA-256 绑定；服务端在进入数值引擎前复核 owner、版本、有效区间、hash、market、rule type 与 type URL，缺项失败关闭。冻结债券日程随后推导转换因子、购入/交割应计利息和持有期票息，进而计算交割发票价、基差、融资成本、净基差、未再投资 IRR 与篮子 CTD。CTD 按 IRR 最大、净基差最小、稳定 bond ID 排序。

该切片贯通 Rust 领域校验、C++20、加法式 C ABI、安全 Rust adapter、独立 Decimal Oracle、确定性 Arrow，以及真实 PostgreSQL 16 + Ceph RGW 发布、adapter 重建后重放和篡改检测。中金所规则以带来源和摘要的冻结 fixture 进入测试，不冒充实时交易所数据；保证金、手续费、真实交割流程和外部行情/篮子适配仍未实现。

## 2026-07 / Phase 2D 已落地单合约 CTD DV01 套保比例

当前实现把带符号的现券或组合目标 DV01，结合 Phase 2C 的 CTD、转换因子和 CTD 每百元 DV01，换算为连续期货合约数、推荐整数手数、整数化剩余 DV01 与套保有效性。正目标风险对应卖出期货，负目标风险对应买入期货；整数手数在向下取整、向上取整和零手中最小化绝对剩余风险，并以最小绝对手数和稳定有符号整数打破平局。

结果绑定目标 Risk Artifact、Delivery Artifact、CTD Analytics Artifact、FuturesContract、CTD Bond、RulePack 和 DataSnapshot 七段血缘，并贯通独立 Decimal Oracle、C++20/C ABI/Rust、确定性 Arrow 以及真实 PostgreSQL 16 + Ceph RGW 发布、重启重放和篡改失败关闭。该方法只处理单一 CTD 的平行 1bp 一阶风险，不是关键期限或多合约曲线风险优化，也不包含基差、CTD 切换、凸性、流动性和动态再平衡风险。

## 2026-08 / R5D 已落地 Rates 精确输入物化

当前实现保留 `ficant.rates.v1.RatesAnalyticsService` 五个一元 RPC 和可安装的 `ficant-sdk`。Python 只消费由 `interface/` 确定性生成的 Protobuf/gRPC 类型，通过认证后的 `ficant-server` 调用 Phase 2A–2D 的真实 Rust application 与 C++ 数值 provider；它不重写算法，不直连 PostgreSQL、Ceph RGW 或 C ABI。

公共请求不再携带 Bond 条款、Calendar 内容、曲线节点、候选券、价格、转换因子、交割日期、CTD 或 DV01 的重复副本。现券、曲线、Carry/Roll-down、交割篮子/CTD 和套保分别提交所需的 exact Object/Snapshot/Artifact 引用；Application 从权威仓储物化实际数值输入，并在进入 provider 前失败关闭 owner、version、hash、knowledge/valuation/as-of/visible/effective time 与内容漂移。响应回显稳定排序的全部实际消费输入、参数摘要和请求指纹。R5D 不包含 AC09 双口径税后分析、AC37 权限分层或尚未组合的 Definition/Fact/Snapshot/Artifact 服务面。

## 2026-07 / Phase 3A 已落地双源 Canonical Quote 接入

当前实现新增版本化 `DataSource` 注册和 `ficant-data` 接入边界。文件 NDJSON 与 PostgreSQL adapter 只消费冻结的 raw quote 合同，通过精确 Instrument、Calendar、Unit 版本映射以及 observed/visible 双时间点时选择，生成固定 16 列的 `ficant.market.quote.canonical.v1` Arrow RecordBatch。

质量规则对重复 source record、非法时间、映射缺失/重叠、闭市或会话外数据、空双边、交叉报价、Decimal scale 与 Unit 漂移失败关闭，整批失败而不返回部分结果。真实 PostgreSQL 双源验收已证明 schema ID/hash、字段类型、nullable、metadata、稳定排序与业务列一致。该切片的进程内 RecordBatch 由下述 Phase 3B 固化，不单独冒充研究快照。

## 2026-07 / Phase 3B 已落地不可变 Parquet Snapshot

当前实现把 Canonical Quote RecordBatch 编码为 Apache Arrow/Parquet Rust `59.1.0` 的确定性单 row-group、无压缩、无 dictionary、writer/data page v2 文件，并生成 `ficant.data.snapshot-manifest.v1` canonical JSON。Manifest 精确绑定 owner、schema、Parquet hash/size/rows、点时窗口、DataSource、Instrument mapping、Calendar、Unit、实际 Instrument、质量规则和 writer 参数。

Application 复用既有 `BlobStore`、`VerifiedSnapshotProof::data` 与 `SnapshotRepository` 完成 Parquet/Manifest 双 payload 发布；正式读取复用 `VerifiedReadFacade` required read，再由 `ficant-data` 对 metadata、两个 payload、canonical Manifest、Parquet 元数据、schema、行数与血缘失败关闭。真实 PostgreSQL 16 + Ceph RGW 验收证明外源只在 ingest 时调用一次；销毁 source adapter 并重建存储 adapter 后，仍可只按 `DataSnapshot` ID 取得完全相同的 Canonical RecordBatch。

## 当前候选 / Phase 4 持久化 ResearchGraph 执行闭环

ResearchNodeContract 与 ResearchGraph 是版本化 Definition：节点合同绑定输入/输出 type ID、version、schema hash、状态/参数 schema、确定性等级、权限、资源限制和不变量；图将节点、边、外部输入声明和外部输入绑定规范化，并拒绝类型不匹配、循环或未满足的必需输入。实际外部值连同内容 hash 进入可复现身份；身份还绑定 Data/Universe Snapshot、参数、runtime/environment、seed、RulePack 的 ID/version/content hash 与节点实现 digest。`ExperimentRun` 只标识一次执行实例，不进入计算或 Artifact 的可复现 digest。

首个生产 NativeNode 是 CGB 固收分析节点：它消费既有 `ficant.rates.v1.AnalyzeBondRequest`，经与 gRPC API 共用的 Rust/C++ 生产计算路径生成确定性 `AnalyzeBondResult`；不是测试桩、模拟定价或第二套算法。节点输出采用确定性多端口 envelope，Artifact 和结果 digest 绑定可复现身份、合同、实现、上游 Artifact 和输出 hash，因此同一冻结身份的不同 Run 必须得到同一结果，任何漂移均失败关闭。

生产 `ExperimentService` 从认证 scope 接收 graph run 提交；Server required-read 并交叉校验 Data/Universe Snapshot、RulePack、外部 Artifact 和 Ceph payload，只使用部署注入的 runtime、environment 与 source identity。Repository 在一个 PostgreSQL transaction 内创建并启动 Run、写 Journal、冻结 graph/identity/bindings 并发布拓扑首节点；相同幂等请求精确重放，任一字段漂移冲突。每个节点任务冻结计划 Artifact ID；worker 用数据库时钟和 `FOR UPDATE SKIP LOCKED` 领取 lease，只有当前 lease/fencing epoch 才能开始、续租或完成。enqueue/begin/complete 都从冻结 graph 与 Journal 校验 resume node，后继节点由 Repository 唯一派生。输出先提升到 Ceph RGW，随后在同一 PostgreSQL 事务中校验并写 Artifact、canonical output manifest、Journal、checkpoint、节点状态、后继任务或 Run 完成并释放 lease。持久查询支持 run、manifest/checkpoint、递归输出追踪和 11 个可复现维度比较。

真实 PostgreSQL 16 + Ceph RGW 已验证正式 application 提交与持久查询、对象提升后/事务前 worker 中断、attempt 2 重新领取、旧 fencing epoch 拒绝、`AnalyzeBondResult -> RiskSummary` 强类型两节点推进、上游篡改失败、Artifact/Journal/checkpoint/Run 原子收口与最终重放。Phase 4 的退出范围仍限于 Rust NativeNode 执行闭环，不包含 GeneratedNode/gVisor、业务 UI 或 Phase 5 Lab。

## WebApp 产品边界

当前可用产品界面是 Platform Shell，不是完整 DMQuant：

- 从服务端读取当前会话和可见应用，不从前端角色字符串推导最终授权；
- 通过短期授权加载 iframe 应用，校验精确 origin、entrypoint、capability、CSP、sandbox 和有效期；
- 区分会话、目录、授权、不可用、拒绝、加载失败等状态，并展示安全错误码、`trace_id` 与允许的恢复动作；
- token 不进入 URL 或 `localStorage`，应用退出、过期和边界失败会触发撤权。
- Phase 5A 在会话与目录边界之后内嵌一块临时只读观测面板：按 Human 提供的 Run ID 读取 Data/Universe Snapshot、RulePack、外部 Artifact、manifest/checkpoint、输出血缘和经 Artifact/Ceph required-read 校验的固定收益节点 payload。它显式不具备搜索、推荐、交易或正式研究语义，也不冒充短期授权 iframe WebApp。

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

- 关键期限/多合约曲线风险对冲和组合层优化；Phase 2E 的 Python SDK 首版只提供同步一元调用，不承诺批量、流式或长任务调度。
- 完整 DMQuant 业务 WebApp，包括策略生成、回测、通用 Artifact 浏览、多 run 比较及静态原型中的高级分析页面；当前 UI 仍仅为 Platform Shell 与内嵌的 Phase 5A 临时运行观测面板。
- openGauss 迁移、GeneratedNode/gVisor 业务运行、OMS/EMS 和任何外部交易执行。
- 信用债、ABS、可转债、完整利率互换生命周期、真实询价通讯与清算交割。
- README Phase 5 的正式研究 Lab以及 Phase 6–9 的仿真、AI 基础设施、GeneratedNode 和后续发布流程仍为规划能力，不得把 Phase 5A 临时观测面板描述为当前已完成的业务体验。当前 worker 只装配 CGB 固收 NativeNode 与 Phase 4 证据化执行路径；扩充业务节点 catalog 或提供可用研究 UI 属于后续纵向切片。

## Validity

Valid: long-term until superseded
