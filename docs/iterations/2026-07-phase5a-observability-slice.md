# Phase 5A 固收研究观测切片

## 目标

- 在既有 DataSnapshot、固定收益分析服务和 Phase 4 持久化执行闭环之上，提供一条可由 Human 直接观察的真实链路。
- 临时界面只展示输入绑定、计算结果、运行状态和血缘证据，不承载搜索、推荐、交易决策或正式 Rates Research Lab 业务语义。
- 通过一份面向 Human 的 brief 收口多个快速子循环，不创建 Agent 状态文档或子任务 brief。

## 验收

- Human 可在 Platform Shell 完成真实会话和目录边界后使用 Phase 5A 观测界面；该内嵌临时面板不冒充 App Registry 的短期 iframe 应用。
- 界面通过真实 gRPC-Web 调用既有生产服务，展示 DataSnapshot/RulePack 绑定、国债现金流、净价、全价、YTM、久期、凸性和 DV01。
- 界面可按 Run ID 展示真实 Phase 4 运行状态、执行身份、节点 manifest/checkpoint 和输出血缘；不存在的数据、权限失败和不完整响应必须失败关闭。
- 页面显式标记为临时观测工具，禁止出现推荐、买卖、目标仓位或正式研究结论。
- 最终候选通过 `scripts/check-fast.ps1`、`scripts/check.ps1`、`scripts/check.ps1 -IncludeIntegration` 及真实本地浏览器/Compose 验收。

## 非目标

- 不实现正式 Rates Research Lab、债券搜索产品、曲线构建 UI、相对价值、报告导出或 CGB Futures Lab。
- 不修改固定收益数值算法、Oracle、Golden Case、expected、断言或容差。
- 不实现 GeneratedNode、gVisor、AI、SignalSet 发布或交易执行。
- 不创建版本 tag，不触发远程 CI/CD，不连接或部署测试环境。

## 公共契约变化

- `ficant.research.v1.ExperimentService` 加法式新增 `ReadNodeOutput`：按 Run/Node ID 经既有 required-read 边界读取不超过 1 MiB 的 canonical output envelope，并把解码结果逐端口与持久化 manifest 的名称、类型和内容 hash 交叉校验后返回。
- 既有市场、固定收益和实验消息语义保持兼容；临时 UI 本身不进入业务领域合同。

## 需 Human 决策

- 无。Human 已明确界面为临时观测面，不具有业务意义。

## 最终真实测试证据

- `scripts/check-fast.ps1`：exit 0；Rust 工作区编译、非环境测试、存储 3/3、Phase 3A 5/5、Phase 3B 2/2 全部通过。
- `scripts/check.ps1`：exit 0；严格 Clippy、工作区构建、契约 13/13、C++ 8/8、Q-001..Q-036 完整性 36/36、Phase 2B/2C/2D 矩阵分别 16/16、18/18、18/18、Python 生成契约 1 passed/1 skipped、Python SDK parity 1/1、Web 类型检查与生产构建、Web 组件测试 35/35 全部通过。
- `scripts/check.ps1 -IncludeIntegration`：exit 0；在隔离的真实 PostgreSQL 与锁定 Ceph RGW 上完成 31 项集成测试。新增生产 Worker → PostgreSQL/Artifact 元数据 → Ceph payload → 生产 `ExperimentGrpcService.ReadNodeOutput` 路径 1/1 通过，并真实触发 size drift 负向完整性事件后失败关闭。
- `cargo test --offline --locked -p ficant-api`：exit 0，19/19；其中新增输出 envelope/manifest 交叉校验 4/4。
- `cargo test --offline --locked -p ficant-contract-tests`：exit 0，13/13；生成契约 descriptor 与服务清单精确。
- 本地真实浏览器：使用实际 `ficant-server`、PostgreSQL、锁定 Ceph RGW 与生产 Web build，按真实 Run ID 展示 SUCCEEDED 运行、两节点 lineage、DataSnapshot/UniverseSnapshot/RulePack、国债现金流与净全价/YTM/久期/凸性/DV01、RiskSummary；浏览器 console 0 error/0 warning；640×900 无水平溢出，标题对比度缺陷经修复后复测通过。
- 许可证 inventory 由权威刷新入口重绑并以 native-LF、完整一方源码校验通过；digest 为 `f6f6ccb6bc17d0b568405c75f83d7d4db8e5f78216f1550f12d9d61731a3e969`。

## 残余风险

- 临时观测面只支持输入已知 Run ID，不提供运行搜索、分页或研究工作流；这是本迭代刻意边界。
- 已知的 `AnalyzeBondResult` 与 `RiskSummary` 可结构化展示；未知 output type 只显示类型、hash 与长度，避免猜测语义。
- `ReadNodeOutput` 是有权限、按 owner scope、required-read 且最大 1 MiB 的原始观测接口；若未来 payload 增长，应新增分页或专用查询合同，而不是提高无界读取上限。
- 浏览器验收使用本地开发身份和隔离基础设施，不等同于版本 CI、测试环境部署或业务 UAT。
- 清洁工作树上的完整契约再生成脚本在调用 Buf 远程插件时被 BSR `resource_exhausted` 限流；限流前已分别证明两次权威生成树 digest 相同，并通过 no-index 对比确认 Rust/Python/TypeScript 生成源码与提交精确一致。后续 CI 仍需在限流恢复后完成该联网门禁。
