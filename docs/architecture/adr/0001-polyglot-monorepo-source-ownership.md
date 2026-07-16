# ADR-0001：多语言 Monorepo 源码所有权

- 状态：Accepted
- 日期：2026-07-13
- 决策者：Human，经 Orchestrator Architecture lens 形成方案

## 背景

ficant 同时包含 Rust 控制平面、C++20 固收数值内核、Python 研究节点运行时、TypeScript/React WebApp 与 Protobuf 契约。根目录曾保留空的 `src/` 和 `result/`，而 README 的仓库树仍以规划中的 `proto/`、crate 和 WebApp 描述当前结构，容易诱发第二套目录和契约源。

## 决策

仓库按语言、构建系统和所有权边界组织，不设置统一的根 `src/`：

- `crates/`：Rust 库；每个 crate 使用自己的 `src/` 与 `tests/`。
- `binaries/`：Rust 可执行程序与 composition root。
- `cpp/`：C++ 数值库；每个库保留 CMake 惯例的 `include/`、`src/` 和 `tests/`。
- `python/`：Python SDK、GeneratedNode 运行时及其生成契约；不放平台控制面和交付门禁。
- `web-dm/`：pnpm workspace、共享 Platform Shell、生成契约和业务 WebApp。
- `interface/`：唯一 Protobuf 契约源；禁止根 `proto/` 或各消费者手写平行 DTO。
- `.github/scripts/`：仓库、供应链、Compose 和发布门禁工具。
- `deploy/dev/`：跨迭代复用的本地运行拓扑；Compose 项目名和本地镜像标签使用稳定的 `ficant-dev`，不得嵌入 iteration 编号。
- `tests/`：跨语言 Golden Case 与系统级验收数据；语言内部测试仍与所属模块共置。
- `docs/architecture/adr/`：架构选择、替代方案、依据和后果。

README 的仓库树必须区分“当前权威结构”和“规划扩展”。尚未创建的 crate、Domain Pack、SDK、WebApp 或部署目标不得表现为已经实现。

生成代码不是第二契约源。`interface/buf.gen.yaml` 机械生成 Rust、Python 与 TypeScript consumer，生成结果只能通过契约生成门禁更新。

## 依据

按构建系统分区保留 Cargo、CMake、uv 和 pnpm 的标准约定，使每个模块能够独立构建、测试和理解。概念完整性由唯一契约、固定依赖方向和目录所有权保证，而不是由一层没有业务含义的根 `src/` 保证。

## 被否决方案

1. **全部移动到根 `src/`。** 增加无语义层级，破坏各工具链惯例，并不能减少跨语言边界。
2. **按部署服务复制多语言目录。** 会把契约和数值算法复制到多个服务，扩大变更传播。
3. **每种语言维护自己的契约。** 会产生字段、错误和版本语义漂移。

## 后果

- 新顶层源码目录属于架构变更，必须通过 ADR 和仓库策略门禁。
- 空的根 `src/`、`result/` 及其发布白名单被删除。
- Python 交付工具迁至 `.github/scripts/`；Python 产品边界保持清晰。
- 当前开发环境资源使用稳定名称，历史 release note 仍保留当时的 iteration 名称作为证据。
- 当前结构变化时，README 和本 ADR 的约束引用必须在同一候选中更新。

## 强制执行

- `.gitignore` 发布 allowlist 控制允许的顶层目录。
- `.github/scripts/verify-repo-policy.sh` 拒绝根 `proto/`、错误语言位置和未批准顶层内容。
- `.github/scripts/verify-contract-generation.sh` 验证三个生成 consumer 与 `interface/` 一致。
