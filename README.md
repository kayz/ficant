# ficant

> **Fixed Income Connected AI-Native Technology**
> 固定收益优先、可复现、可分层扩展的量化研究基础设施。它交付研究结论与已标定的策略，不执行策略。

首个市场是中国银行间国债现券与中金所国债期货。ficant 不自建数据：外部数据源经适配器转换为标准结构，形成不可变的点时快照，再进入计算。

## 分期

| 期 | 目标 | 状态 |
|---|---|---|
| 第一期（v0.1） | 国债与国债期货纵向链路正确：数据接入、点时快照、现金流、曲线、估值与风险、可交割券与 CTD、套保、组合敞口、证据与血缘 | 30 条验收均有命令证据，待 Human 完成批准与 MANUAL 走查 |
| 第二期（v0.2） | 研究闭环：策略定义、bar 点时回放、仿真成交、损益分解、标定，交付已标定策略 | 验收条目已批准（AC38–AC45），待实现 |
| 第三期（v0.3） | 生成式研究节点：AI 按节点契约生成研究方法，经检查、测试与沙箱后产物化 | 方向 |

分期来自已批准的 SPEC v1.4（2026-10-21）。分期与验收以私有 `kayz/ficant-authority` 仓库的 SPEC 与 ACCEPTANCE 为准，本表只是摘要。

## 现在能做什么

- 固定利率与贴现国债的现金流、应计利息、净价与全价、到期收益率、久期、凸性、DV01 和关键期限 DV01。
- 参考曲线、持有期 Carry / Roll-down。
- 国债期货可交割券、转换因子、基差、净基差、隐含回购利率、CTD 与套保手数；市场税前与主体税收调整后两个口径。
- 机构主体（资金档、税收待遇）绑定计算；规则包在计算中被解析而不只是被引用。
- 双时间点时查询、不可变快照、数据源白名单、价格来源可信度、覆盖度声明与数据健康报告。
- 持仓快照、会计分类三态、回购穿透，以及组合只读视图与日度收益序列。
- 正式输出的完整血缘、确定性身份与崩溃恢复。
- React Platform Shell：真实 gRPC-Web 会话与应用启动授权。

## 文档入口

| 想知道 | 看哪里 |
|---|---|
| 什么必须为真、如何验收 | 私有 authority 仓库：SPEC、ACCEPTANCE、MANUAL |
| 怎么协作、交付什么 | [`AGENTS.md`](AGENTS.md)、[`docs/development.md`](docs/development.md) |
| 为什么这样设计 | [`docs/architecture/adr/`](docs/architecture/adr/) |
| 技术选型、架构形态、原始愿景 | [`docs/architecture/baseline.md`](docs/architecture/baseline.md)（非权威） |
| 产品范围现状 | [`docs/product/scope.md`](docs/product/scope.md) |
| 发布与测试环境 | [`docs/delivery/`](docs/delivery/) |
| 后台接口 | [`interface/`](interface/)（Protobuf，唯一来源） |

## 技术栈

后台只用 Rust；数值内核为 C++20，经稳定 C ABI 接入；Python 用于 SDK、契约与独立 Oracle；元数据库为 PostgreSQL 16，对象存储为 Ceph RGW；前端为 React。日常开发在 Windows 11 + PowerShell 7 上进行，发布制品只在 Linux GitHub Runner 上构建。

## 快速开始

```powershell
.\scripts\check-fast.ps1          # 提交前快速检查
.\scripts\check.ps1               # 完整本地回归
.\scripts\dev-up.ps1              # 启动本地开发拓扑（PostgreSQL、Ceph RGW、Server、Worker、UI）
```

各命令都支持 `-ListOnly` 先查看将执行什么。工具版本与离线依赖要求见 [`docs/development.md`](docs/development.md)。

## 发布

普通 push、Pull Request 和 `main` 合并不触发发布。只有 Human 创建指向当前 `main` 的不可变 `v*` tag，才进入中央 `kayz/cicd` 的版本 CI、镜像构建、扫描与测试环境部署；失败的 tag 不移动，修复后用新的版本号。最近一次是 `v0.1.0-alpha.12`，详见 [`docs/delivery/release-notes.md`](docs/delivery/release-notes.md)。

## 许可证

源代码采用 [MIT License](LICENSE)。第三方依赖仍适用各自的许可证，见 [`docs/delivery/third-party-notices.md`](docs/delivery/third-party-notices.md)。
