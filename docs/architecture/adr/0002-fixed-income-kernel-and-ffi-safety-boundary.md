# ADR-0002：固定收益数值内核与 FFI 安全边界

- 状态：Accepted
- 日期：2026-07-13
- 决策者：Human，经 Orchestrator Architecture lens 形成方案

## 背景

Phase 2 要求 C++20 提供权威固定收益参考算法，并由 Rust 通过稳定 C ABI 调用。当前 workspace 对所有既有 crate 设置 `unsafe_code = "forbid"`，而 FFI 调用必然需要少量 unsafe。现有 `Cashflow` 和 `Valuation` 契约又明确表示外部输入事实，不能直接承载平台生成的现金流和估值结果。

## 决策

### 模块边界

- `cpp/fixed-income-kernel` 只实现确定性数值算法，不处理身份、权限、持久化、血缘、任务和 API。
- 新建 `ficant-kernel-sys`，作为整个 workspace 唯一允许 FFI unsafe 的 crate。它不继承 workspace 的 `unsafe_code = "forbid"`，但必须显式标注每个 unsafe 操作及其安全前提，并拒绝在其他 crate 中新增 unsafe。
- Application 定义领域化 `BondAnalyticsEngine` port，只使用 Domain 的 `BondAnalyticsInput/Result` 与稳定 `AnalyticsError`；它不依赖任何计算 provider、FFI 或供应商类型。
- 新建 `ficant-fixed-income-native` adapter，实现 Application port，负责输入校验、单位、错误映射和 C ABI 安全封装；它依赖 `ficant-kernel-sys`，自身继续禁止 unsafe。
- `ficant-worker` 是 composition root，静态、显式注入 `native-reference` provider；禁止运行时自动发现或静默 fallback。
- QuantLib 本轮只作为独立 Golden Case Oracle，不进入生产依赖。未来若获准进入生产，必须作为独立 `ficant-fixed-income-quantlib` Anti-Corruption Adapter 实现同一 port；在真实第二 provider 获准前不创建空 crate 或插件框架。
- 模块遵守 [ADR-0003](0003-deep-modules-and-explicit-internal-boundaries.md)；本小迭代不新增公共 gRPC、Python SDK 或 Web 页面。

### ABI 合同

- ABI 使用固定宽度整数、显式长度和稳定状态码；不跨边界传递 Rust/C++ 容器、异常或所有权不清晰的指针。
- C++ 捕获全部异常并转换为状态码；异常不得跨越 `extern "C"`。
- 调用方拥有输入和输出缓冲区；所有长度、空指针、枚举、数值范围和 ABI 版本在进入算法前验证。
- 算法必须确定性、可重入；不得读取系统时钟、环境变量、网络或隐式全局状态。
- 每次调用先验证 ABI version，版本不匹配 fail closed。

### 结果语义

- 平台生成的现金流、净价、全价、收益率、久期、修正久期、凸性和 DV01 使用内部 `BondAnalyticsResult`，不写成外部 `Cashflow` 或 `Valuation` 事实。
- 正式结果以内容寻址 Artifact 保存，绑定 Bond 精确版本、MarketRulePack 精确版本、估值时点、输入快照、算法版本、ABI 版本和单位。
- 相同输入与版本必须产生相同规范结果和内容哈希；版本或输入变化必须产生可解释、可追踪的差异。
- Artifact 记录 `engine_id`、`engine_version`、`algorithm_id`、`algorithm_version`、`convention_profile` 和 `abi_version`；provider 不得在失败时悄悄替换。

## 依据

唯一 sys crate 把不可避免的 unsafe 压缩到最小审计面，native adapter 吸收 ABI、单位和错误处理复杂度，Application port 阻止 provider 细节向业务层扩散。将计算结果与外部市场事实分离，并把 provider 身份写入血缘，维持 Phase 1 已冻结的来源语义和历史可重放性。

## 被否决方案

1. **全 workspace 放宽 unsafe。** 审计面不可控，并破坏既有安全承诺。
2. **在 Application 或 Worker 内直接写 FFI。** 让数值边界、业务编排和部署细节耦合。
3. **用现有 Cashflow/Valuation 保存计算结果。** 混淆外部事实与平台派生物。
4. **本轮新增公共 RPC、Python SDK 或 Web 页面。** 会使小迭代跨越更多高风险边界，削弱数值正确性验证。
5. **改用纯 Rust 重写 C++ 目标。** 与已确认的 v0.1 技术基线冲突，且不能验证既定跨语言参考边界。
6. **Application 直接依赖统一计算门面。** 会把具体 provider 变成高层依赖，未来引入 QuantLib 时扩大变化传播。
7. **现在建设动态插件系统。** 当前只有一个生产 provider，提前建设属于 speculative generality。

## 验证要求

- C++ Golden Case、边界值、错误和不收敛测试。
- C ABI 版本、空指针、长度、异常转码、内存与重复调用测试。
- Rust 安全门面与 C++ 结果一致性、单位和误差测试。
- `cargo geiger` 或等价确定性检查证明 unsafe 只存在于批准的 sys crate。
- 依赖门禁证明 Domain/Application 不依赖 native/sys/QuantLib，供应商类型不出 adapter。
- Provider 契约测试证明失败不触发静默 fallback，结果完整记录 provider 身份。
- Application 业务验收证明结果绑定完整且相同输入可重放。
