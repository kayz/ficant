# ficant 产品范围

**状态：** Phase 0–Phase 4 研究运行时内核已完成；对象存储统一为 Ceph RGW + Apache `object_store`

**实现状态：** 当前能力以已合并代码、冻结合同和可重放本地证据为准，不把局部纵向切片扩写为完整 Phase 或最终产品

**来源：** `README.md`、`interface/`、`web-dm/` 与当前生产实现

## 产品定位与终点

ficant 是面向专业投资研究团队的 AI 原生量化研究平台。平台把数据、领域知识、研究方法、运行和结果组织为可版本化、可追踪、可复现的研究资产，并通过统一后台合同向 WebApp、Python SDK 和 Agent 提供能力。

正式产品终点保持为 `ResearchArtifact`、`SimulationResult`、`ReportArtifact`、`SignalSet` 和 `TargetExposure`。平台不拥有订单和外部交易执行，不建设 OMS、EMS、对外报单、清算、结算或投资组合会计。

首个市场仍是中国国债现券与国债期货。当前已完成 Phase 0 仓库/合同基线、Phase 1 领域内核、Phase 2 固定收益参考数值库和 Phase 3 可复现数据链，并交付 Phase 4A 的强类型 ResearchGraph 定义合同。Python SDK 通过同一 Protobuf/gRPC 合同调用真实 Rust/C++ 生产路径，结果与冻结参考一致；文件与 PostgreSQL adapter 把外部报价转换为相同 Canonical Arrow Schema，再发布为可校验、可脱离外源重读的不可变 Parquet `DataSnapshot`。这不表示完整研究产品页面或 Phase 4 运行时已实现。

## Phase 0 已落地边界

- Rust Workspace 是唯一后台实现；Python 只承担生成节点运行时/合同消费，C++20 只保留稳定 C ABI 数值库边界。
- `interface/` 是后台 Protobuf 唯一来源，并生成 Rust、Python、TypeScript consumer；不建立平行 REST/OpenAPI DTO。
- PostgreSQL Migration、Ceph RGW 内容寻址对象、开发 Compose、固定工具链和多语言构建已进入发布候选。
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

当前实现覆盖中金所 `TS`、`TF`、`T`、`TL` 合约参数与可交割券期限资格，并由冻结债券日程推导转换因子、购入/交割应计利息和持有期票息，进而计算交割发票价、基差、融资成本、净基差、未再投资 IRR 与篮子 CTD。CTD 按 IRR 最大、净基差最小、稳定 bond ID 排序。

该切片贯通 Rust 领域校验、C++20、加法式 C ABI、安全 Rust adapter、独立 Decimal Oracle、确定性 Arrow，以及真实 PostgreSQL 16 + Ceph RGW 发布、adapter 重建后重放和篡改检测。中金所规则以带来源和摘要的冻结 fixture 进入测试，不冒充实时交易所数据；保证金、手续费、真实交割流程和外部行情/篮子适配仍未实现。

## 2026-07 / Phase 2D 已落地单合约 CTD DV01 套保比例

当前实现把带符号的现券或组合目标 DV01，结合 Phase 2C 的 CTD、转换因子和 CTD 每百元 DV01，换算为连续期货合约数、推荐整数手数、整数化剩余 DV01 与套保有效性。正目标风险对应卖出期货，负目标风险对应买入期货；整数手数在向下取整、向上取整和零手中最小化绝对剩余风险，并以最小绝对手数和稳定有符号整数打破平局。

结果绑定目标 Risk Artifact、Delivery Artifact、CTD Analytics Artifact、FuturesContract、CTD Bond、RulePack 和 DataSnapshot 七段血缘，并贯通独立 Decimal Oracle、C++20/C ABI/Rust、确定性 Arrow 以及真实 PostgreSQL 16 + Ceph RGW 发布、重启重放和篡改失败关闭。该方法只处理单一 CTD 的平行 1bp 一阶风险，不是关键期限或多合约曲线风险优化，也不包含基差、CTD 切换、凸性、流动性和动态再平衡风险。

## 2026-07 / Phase 2E 已落地 Python SDK 一致性闭环

当前实现新增 `ficant.rates.v1.RatesAnalyticsService` 五个一元 RPC 和可安装的 `ficant-sdk`。Python 只消费由 `interface/` 确定性生成的 Protobuf/gRPC 类型，通过认证后的 `ficant-server` 调用 Phase 2A–2D 的真实 Rust application 与 C++ 数值 provider；它不重写算法，不直连 PostgreSQL、Ceph RGW 或 C ABI。

每个请求显式绑定 owner、DataSnapshot、MarketRulePack、算法/约定/ABI 版本和对应市场对象，服务端要求 `rates:analyze` scope 并在进入 provider 前失败关闭非法身份与输入。真实服务进程上的跨语言 Golden Case 已覆盖现券、曲线、Carry/Roll-down、交割篮子/CTD 和套保五类调用，并证明 Python 结果与冻结参考一致。

## 2026-07 / Phase 3A 已落地双源 Canonical Quote 接入

当前实现新增版本化 `DataSource` 注册和 `ficant-data` 接入边界。文件 NDJSON 与 PostgreSQL adapter 只消费冻结的 raw quote 合同，通过精确 Instrument、Calendar、Unit 版本映射以及 observed/visible 双时间点时选择，生成固定 16 列的 `ficant.market.quote.canonical.v1` Arrow RecordBatch。

质量规则对重复 source record、非法时间、映射缺失/重叠、闭市或会话外数据、空双边、交叉报价、Decimal scale 与 Unit 漂移失败关闭，整批失败而不返回部分结果。真实 PostgreSQL 双源验收已证明 schema ID/hash、字段类型、nullable、metadata、稳定排序与业务列一致。该切片的进程内 RecordBatch 由下述 Phase 3B 固化，不单独冒充研究快照。

## 2026-07 / Phase 3B 已落地不可变 Parquet Snapshot

当前实现把 Canonical Quote RecordBatch 编码为 Apache Arrow/Parquet Rust `59.1.0` 的确定性单 row-group、无压缩、无 dictionary、writer/data page v2 文件，并生成 `ficant.data.snapshot-manifest.v1` canonical JSON。Manifest 精确绑定 owner、schema、Parquet hash/size/rows、点时窗口、DataSource、Instrument mapping、Calendar、Unit、实际 Instrument、质量规则和 writer 参数。

Application 复用既有 `BlobStore`、`VerifiedSnapshotProof::data` 与 `SnapshotRepository` 完成 Parquet/Manifest 双 payload 发布；正式读取复用 `VerifiedReadFacade` required read，再由 `ficant-data` 对 metadata、两个 payload、canonical Manifest、Parquet 元数据、schema、行数与血缘失败关闭。真实 PostgreSQL 16 + Ceph RGW 验收证明外源只在 ingest 时调用一次；销毁 source adapter 并重建存储 adapter 后，仍可只按 `DataSnapshot` ID 取得完全相同的 Canonical RecordBatch。

## 2026-07 / Phase 4A 已落地强类型 ResearchGraph 定义合同

当前领域层新增版本化 `ResearchNodeContract` 与 `ResearchGraph`。节点合同精确绑定输入/输出类型及 schema hash、状态和参数 schema、确定性等级、权限、资源限制和必守不变量；图只接受存在的节点与端口、完全匹配的类型、每个必需输入唯一绑定的有向无环结构。

合同声明、节点和边在构造时规范化；确定性拓扑排序以节点 ID 稳定打破并列关系，图摘要不受调用方 collection 顺序影响。该切片只是不可变 Definition 边界，尚不包含节点执行、Run 状态机扩展、Lease Queue、持久化、恢复、Artifact 节点血缘或实验比较，不得描述为 Phase 4 已完成。

## 2026-07 / Phase 4B 已落地图执行状态机与 checkpoint Journal

当前 runtime 在既有 hash-chained、sequence-contiguous、幂等 append-only `RunJournal` 上新增 node started/succeeded/failed/checkpointed 四类事件，并严格按 ResearchGraph 的确定性拓扑序重放。节点输出只有在 succeeded 事件之后以完全相同 hash 提交 checkpoint 才算完成；中断在 node started 或未 checkpoint 的 succeeded 之后，都从同一节点以递增 attempt 重跑。

图重放会返回已完成节点、最后安全 checkpoint（节点、attempt、输出 hash、Journal sequence/hash）和下一恢复节点；缺失/错序节点、错误 attempt、提前成功、checkpoint 漂移或损坏的 Journal 链全部失败关闭。PostgreSQL 事件类型约束已前向扩展，但 Lease Queue、Worker 认领、租约恢复和真实持久化并发验收属于 Phase 4C。

## 2026-07 / Phase 4C 已落地 PostgreSQL Lease Queue

当前 storage 新增 tenant 隔离的 `execution_tasks` 与 `PostgresLeaseQueue`。任务不可变绑定 run、node、node attempt、graph digest 和稳定 task key；相同 key + 相同业务字段幂等返回，任一字段漂移冲突。claim 使用数据库时钟、`FOR UPDATE SKIP LOCKED` 和稳定排序，使多个 worker 原子取得不同任务。

lease 绑定 worker ULID 与 lease ULID，续租和完成只接受未过期的当前所有者；完成证据 hash 不可变且相同重试幂等。进程中断后无需原位修复旧行，过期 lease 可由另一 worker 原子回收并增加 claim count。真实 PostgreSQL 16 已覆盖并发 claim、错误所有者、完成漂移、过期恢复、tenant 隔离及 11 个 migration 的重复/失败原子性。

## 2026-07 / Phase 4D-E 已落地 NativeNode、节点血缘与实验比较

当前 runtime 新增 `ExecutionIdentity`，精确绑定 DataSnapshot、UniverseSnapshot、ResearchGraph、参数、runtime image、环境摘要、seed 与每个节点实现 digest。NativeNode engine 只按图的确定性拓扑序运行，输入来自已验证上游端口，输出必须逐端口匹配 type ID、version 和 schema hash；缺失/重复实现或任何类型漂移失败关闭。

每个节点生成不可变 `NativeNodeArtifact`，绑定 execution identity、节点/合同/实现、所有上游节点 artifact 和有序输出 hash；最终 result digest 再绑定完整节点 artifact 链。相同冻结身份的重放要求对象级完全一致，实验比较会分别报告 Snapshot、Graph、Parameters、RuntimeImage、Environment、Seed、Implementation 与 Result 差异。至此 README Phase 4 的强类型图、NativeNode、状态/Journal、Lease Queue、安全点恢复、逐节点血缘、环境/seed 和比较已在本地内核与真实 PostgreSQL 协议层闭合。

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

- 关键期限/多合约曲线风险对冲和组合层优化；Phase 2E 的 Python SDK 首版只提供同步一元调用，不承诺批量、流式或长任务调度。
- 完整 DMQuant 业务 WebApp，包括策略生成、回测、Artifact 浏览、多 run 比较及静态原型中的高级分析页面；当前 UI 仍仅为 Platform Shell。
- openGauss 迁移、GeneratedNode/gVisor 业务运行、OMS/EMS 和任何外部交易执行。
- 信用债、ABS、可转债、完整利率互换生命周期、真实询价通讯与清算交割。
- README Phase 5–9 的研究 Lab、仿真、AI 基础设施、GeneratedNode 和后续发布流程仍为规划能力，不得描述为当前已完成。`ficant-worker` 的持续消费循环与业务 NativeNode catalog 将随首个 Phase 5 纵向切片装配；Phase 4 完成不等于已有可用研究 UI。

## Validity

Valid: long-term until superseded
