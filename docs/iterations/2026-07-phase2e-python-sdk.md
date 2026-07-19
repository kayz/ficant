# Phase 2E：Python SDK 一致性与 Phase 2 正式退出

## 目标

- 在精确基线 `6fcc0d2dd968417ae5c01598dbcda009d792fe66` 上交付可安装、可认证的 Python SDK，使 Python 调用真实 Rust application/native provider，而不是在 Python 中重写 Phase 2 算法。
- 通过加法式 `ficant.rates.v1.RatesAnalyticsService` 暴露 Phase 2A–2D 的现券分析、收益率曲线插值、Carry/Roll-down、国债期货交割分析和 DV01 套保分析。
- 证明相同冻结输入通过 Python SDK、Rust/C++ 生产路径与独立 Oracle 得到一致结果，并在完成后正式关闭 README 的 Phase 2 退出条件。
- 使用快速子循环交付：公共合同与生成树；Rust transport/composition；Python client；跨语言 Golden Case；最终门禁与状态收口。

## 验收

- Python SDK 只通过 gRPC 调用 `ficant-server`，不直接加载 C ABI、不重写定价算法、不访问 PostgreSQL/Ceph RGW、不持有服务端密钥。
- SDK 覆盖 Phase 2A–2D 五类调用，并使用 `interface/` 生成的唯一 Protobuf DTO 与 gRPC stub；不得手写平行 Decimal、时间、错误或业务结果传输类型。
- 每个请求显式绑定 owner、DataSnapshot、MarketRulePack、算法/约定/ABI 版本以及对应 Bond、Curve 或 FuturesContract 身份；缺失、空值、非法 Decimal、日期、枚举、版本或非有限结果均失败关闭。
- 服务端必须复用现有身份边界，要求认证主体具备 `rates:analyze` scope；未认证、scope 缺失和无效 bearer 不得进入数值 provider。
- 五类调用对冻结 Golden Case 的结果与现有 Rust/C++ acceptance 和独立 Python Oracle 一致；精确 Decimal 字段使用规范 `coefficient + scale` 比较，不通过放宽容差、修改 expected 或从 expected 反向生成生产结果制造通过。
- gRPC 状态只表达传输失败；业务校验使用稳定、安全、无内部细节泄漏的结构化错误。既有 `ficant.app.v1.PlatformService` 七个 RPC、公共字段编号和 Phase 2A–2D C ABI 保持不变。
- `./scripts/check-fast.ps1`、`./scripts/check.ps1` 与带真实服务进程的 Python SDK 集成测试在同一最终候选上 exit 0；完整合同生成树确定且无漂移。

## 非目标

- 不实现 Phase 3 的外部数据源、Canonical RecordBatch、数据质量规则或 Parquet Snapshot。
- 不新增 UI、ResearchGraph、异步任务、批量投资组合接口、多合约曲线对冲、GeneratedNode 或交易执行。
- 不允许 Python SDK 作为新的控制平面，也不提供数据库、对象存储、原始凭据或任意 C++ 函数访问。
- 不修改 Phase 2A–2D expected、Oracle、数值公式、断言或容差。

## 公共契约变化

- `interface/` 新增加法式 `ficant.rates.v1` 包及 `RatesAnalyticsService` 五个一元 RPC；共享身份、Decimal、时间和血缘类型继续引用 `ficant.core.v1`，不在 rates 包重复定义。
- Rust、Python 和 TypeScript 生成树由同一 descriptor 确定性再生成；Python 同时生成固定版本的 gRPC client stub。
- `ficant-server` 在同一监听器装配 PlatformService 与 RatesAnalyticsService；不新增进程、端口、后台语言或数据库。

## 需 Human 决策

- 当前无待决项。若实现必须改变既有 Phase 2 数值语义、C ABI、公共字段编号、身份模型，或必须让 Python 直接访问存储/FFI，停止并返回 Human 决策。

## 最终真实测试证据

- 待最终候选形成后填写；中间子循环命令由编排运行状态承载，不在仓库新增状态文档。

## 残余风险

- 待最终候选形成后填写。SDK 首版是同步一元调用，不承诺批量吞吐、流式结果或长任务调度；这些能力属于后续 ResearchGraph/Worker 迭代。
