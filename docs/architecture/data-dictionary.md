# ficant 架构与数据字典

**状态：** 目标架构字典；当前无实现 Schema  
**来源：** `README.md`、`UI-DM/`、iteration-1 Architecture 评审

## 状态标记

| 标记 | 含义 |
|---|---|
| 约束 | README 已冻结且不得无 ADR 偏离的系统事实 |
| 设计 | 目标语义已经描述，但没有生产实现或 Schema 证据 |
| 待契约 | Phase 0 必须冻结的字段、枚举或协议 |
| 已验证实现 | 有源码、生成物和测试证据；iteration-1 中没有此类条目 |

## 系统边界与依赖方向

```text
Static React WebApps / Python SDK / Agent Tools
                 ↓ Protobuf contracts + gRPC-Web/gRPC
Rust API / Application / Domain / Infrastructure
                 ↓ stable C ABI only when numerical kernels require it
C++20 numerical library

Generated Python 3.12 nodes
                 ↓ typed Arrow input + Protobuf output in gVisor
Rust deterministic runtime validates and records facts
```

Rust `domain` 不依赖数据库、网络、文件系统、模型服务或 Web 框架。WebApp 不携带独立后台。C++ 不承载业务编排；Python 不进入平台主进程或直接访问数据库、密钥、Artifact Store、RunJournal。

## 三层领域知识

| 层 | 定义 | 生命周期 |
|---|---|---|
| 领域不变量 | 时间、单位、现金流、事件顺序、点时性、血缘 | Rust 固化 |
| `MarketRulePack` | 合约、时段、交割、节假日等带来源与生效日期的规则 | 版本化 |
| ResearchNode 研究方法 | 曲线、因子、模型、组合、成本、撮合与归因方法 | 可生成、替换、验证和发布 |

## Definition / Run / Artifact

- **Definition：** 方法和参数的版本化定义；修改产生新版本。
- **Run：** 某次确定性执行；重跑产生新 Run。
- **Artifact：** 不可变结果；发布后不覆盖。

任何正式 `SignalSet` 必须追溯到策略、ResearchGraph、因子/模型、Universe、DataSnapshot、规则包、能力版本、运行镜像和 ExperimentRun。

## 核心对象

| 对象 | 类别 | 当前定义 | 关键规则 |
|---|---|---|---|
| `DataSnapshot` | Artifact | 某研究时点可见数据的不可变快照 | 来源、版本、点时性、Manifest、内容哈希 |
| `UniverseSnapshot` | Artifact | 某次研究使用的证券集合 | 不随外部证券池变化 |
| `DomainPack` | Definition | 对象、契约、规则、参考算法和不变量测试的领域交付单元 | Protobuf 描述、内容哈希、有效期 |
| `MarketRulePack` | Definition | 带来源和生效区间的市场规则 | 历史实验绑定当时版本 |
| `ResearchGraph` | Definition | 强类型研究 DAG | 修改创建新版本，不覆盖原图 |
| `ResearchNodeContract` | Definition | 节点 I/O、状态、参数、确定性、权限和资源契约 | Protobuf 唯一边界契约 |
| `ResearchPatchSpec` | Definition | 对研究图的结构化变更意图 | 生成新图版本 |
| `CapabilityArtifact` | Artifact | 已产物化的生成式研究能力 | 源码/依赖/模型/提示/测试/权限可追踪 |
| `ExperimentRun` | Run | ResearchGraph 的一次受控执行 | 固定输入、规则、环境和种子 |
| `RunJournal` | Evidence | 运行事件与模型/工具调用账本 | 追加、可审计、可重放 |
| `SimulationResult` | Artifact | 仿真输出 | 正式成交事实由 Rust 引擎验证生成 |
| `ReportArtifact` | Artifact | 研究报告 | 绑定完整运行血缘 |
| `SignalSet` | Artifact | 受治理的研究信号 | 平台正式输出，不是订单 |
| `TargetExposure` | Artifact | 目标风险或敞口 | 由下游决定如何转为订单 |

## DMQuant 临时名称到平台概念

| DMQuant/UI 名称 | 平台目标概念 | 状态 |
|---|---|---|
| strategy / version | `StrategySpec` 版本 | 待契约 |
| task / `TaskInfo` | 异步 Job/Task 状态投影 | 待契约 |
| run / `RunInfo` | `ExperimentRun` + 结果摘要 | 待契约 |
| series(nav/signals) | 运行产物的类型化序列投影 | 待契约 |
| strategy file | `CapabilityArtifact` 或策略源码包引用 | 待确认生命周期和权限 |
| backtest artifact | `SimulationResult` / `ReportArtifact` / 其他 Artifact | 待契约 |
| `SubmitAck` | 幂等提交确认与缓存身份 | 待契约 |
| `ApiError` | Protobuf 错误信封、错误码与 `trace_id` | 待契约 |
| fingerprint | 数据、策略、环境、引擎等复现标识 | 待冻结组成与哈希规则 |

UI-DM 中的 OpenAPI 生成类型和 SSE 事件名只能作为界面抽象或临时别名。正式跨边界契约必须由 Protobuf 产生，浏览器传输必须与 gRPC-Web 基线一致。

## 统一数据规则

- ID、版本、内容哈希和租户/所有者字段必须明确。
- 价格、收益率、金额和风险量使用 Decimal/明确单位，禁止隐式 float 语义。
- 持久化时间使用 UTC，展示层按市场/用户时区呈现；交易日历和估值时点必须显式。
- 状态枚举只能单向进入终态；删除、弃用、引用保护和审计语义必须冻结。
- 错误必须携带稳定业务码和 `trace_id`，不能用 UI 文案替代协议。
- 权限由平台 RBAC + ABAC 执行，客户端禁用按钮不是授权证据。

## Phase 0 必须关闭的契约缺口

1. Protobuf package、兼容策略和 Rust/Python/TypeScript 生成检查。
2. AI 草稿流式消息顺序、断线恢复、完成和错误语义。
3. 策略版本、幂等提交、任务阶段、取消、缓存身份和失败结果保留。
4. 结果序列、文件/Artifact、校验报告、复现指纹和错误信封。
5. 租户与对象授权、导出/下载/删除审计。
6. 时间、单位、分页、未成交原因、排队原因与 Domain Pack 兼容。

这些条目是设计缺口，不是已存在 Schema。

## Validity

Valid: long-term until superseded
