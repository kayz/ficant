# ADR-0003：深模块与显式内部边界

- 状态：Accepted
- 日期：2026-07-13
- 决策者：Human，经 Orchestrator Architecture lens 形成方案

## 背景

ficant 将长期包含领域模型、研究运行时、存储、数值引擎、数据适配、AI 能力和多个 WebApp。仅划分顶层目录不能阻止复杂性扩散：如果内部模块共享供应商类型、隐式状态、错误、表结构或工具细节，系统仍会逐步失去边界感，并使一次局部变化要求跨层协调修改。

## 原则

复杂性必须封装在拥有它的模块内部，而不是弥漫到整个系统。模块应当是“深模块”：以窄、稳定、领域化的接口隐藏较复杂的实现、依赖和失败处理。

## 决策

### 每个模块必须在初始设计中声明

- 单一职责和不负责的事项。
- 对外接口、输入输出、错误和兼容性承诺。
- 拥有的数据、状态、不变量、版本和事务边界。
- 允许依赖、禁止依赖和 composition root。
- 外部供应商或协议的 Anti-Corruption Layer。
- 单元、契约、集成和失败恢复证据。

缺少这些声明的目录、crate、service 或 package 不能作为新模块进入实现。

### 依赖与所有权

- 依赖只能指向更稳定的领域合同或 Application port；高层策略不得依赖具体 adapter、数据库、FFI 或供应商类型。
- Domain 拥有业务语言与不变量；Application 拥有用例和 ports；Adapter 吸收数据库、网络、文件格式、FFI 与第三方库；composition root 只负责显式装配。
- 每个状态和决策只有一个权威所有者。共享数据通过合同传递，不共享内部表、对象或可变全局状态。
- 跨模块错误必须在边界翻译；供应商错误、SQL 错误、C++ 异常和 wire DTO 不得向内层泄漏。
- `utils`、`common`、全局 registry 和跨模块 helper 不是默认共享位置；只有稳定且有明确语义所有者的能力才能共享。

### Provider 与替换

- Application 只定义领域化 provider port，不依赖 provider 实现。
- Provider 使用独立 adapter；供应商类型不得越过 adapter。
- Provider 由 composition root 静态、显式选择，选择和版本写入结果血缘；禁止静默 fallback。
- 替换 provider 不得要求修改 Domain、Application 用例、公共契约或 Artifact schema。
- 本轮不提前建设动态插件框架或空 adapter；只有真实第二 provider 获准时才增加实现。

### 门禁

- CI 从 Cargo metadata、imports 和文件位置检查禁止依赖与 unsafe allowlist。
- 契约测试证明内部实现可替换而调用者行为不变。
- Orchestrator 在 Architecture lens 下检查新增模块的职责、所有权、依赖、错误、状态和测试声明；最终 Audit 只检查文档与实际实现是否一致。
- Architecture 文档必须描述实际依赖；实现变化与 ADR/依赖图在同一候选更新。

## 依据

该原则结合 *A Philosophy of Software Design* 的 Deep Modules、*Clean Architecture* 的依赖反转与 *Domain-Driven Design* 的边界上下文：模块价值不在于文件数量，而在于隐藏复杂性、限制认知负担和缩小变化传播半径。

## 被否决方案

1. **只按目录或语言分层。** 物理隔离不能阻止内部类型和状态泄漏。
2. **建立通用插件/公共工具框架。** 在没有第二个真实实现前会制造 speculative generality。
3. **允许业务层直接调用供应商库。** 短期简单，长期会让领域语义和升级成本扩散到所有调用者。
4. **依靠人工约定、不设门禁。** 约定会随并行开发和时间漂移，必须有自动化证据。

## 后果

- 初始设计比直接编码多一个边界声明步骤，但后续修改与替换更局部。
- 可能产生少量 adapter 和 port；只有真正隐藏复杂性时才允许新增，禁止一函数一模块。
- 新 provider、新数据库或新协议首先表现为 adapter，而不是改写领域与用例。
- 违反边界属于架构缺陷，即使功能测试暂时通过也不能进入最终候选。
