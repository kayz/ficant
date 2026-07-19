# iteration-2 Checklist — Phase 0 + Phase 1 开发轮次

## 状态

**CLOSED：** iteration-2 已于 2026-07-13 完全关闭。最终远程 `main` 为 `80f48706f37e3890224ca106fb763213d0beeb38`；Quality 为 `PASS-WITH-ACCEPTED-RISK`，Review 为 `pass-with-accepted-findings`（C0/I0/M0），main CI run `29201891313` 十项全绿。`RUSTSEC-2025-0052` 按精确 advisory、限时且 fail-closed 的 `accepted-unfixed` 策略受控接受。

## 迭代目标

在一个开发轮次内完成 README 的 Phase 0“仓库和契约基线”与 Phase 1“领域内核”，并以真实业务闭环证明这些基础设施和领域对象能够共同工作，而不是只生成目录、空类型或 CRUD 桩。

## 业务闭环

```text
登记中国国债/国债期货市场事实
→ 创建带版本的 Instrument/Bond/FuturesContract/Calendar/Unit
→ 写入 Quote/Trade/Valuation/CurveSnapshot
→ 固化 UniverseSnapshot 与 DataSnapshot
→ 创建 ExperimentRun
→ 追加 RunJournal 事件
→ 生成不可变 Artifact 与 SignalSet
→ 按 ID、版本、时间和血缘查询
→ 重放并证明历史对象不能被覆盖
```

该闭环使用真实 PostgreSQL 16 和 MinIO 开发容器。测试可以使用确定性的中国国债 golden fixtures，但不得用 mock repository、内存数据库或硬编码成功结果替代真实持久化、版本和血缘行为。

## 明确非目标

- 不实现 Phase 2 的债券定价、到期收益率反解、久期、凸性、DV01 或曲线数值算法。
- 不实现完整 DMQuant 策略生成、回测、仿真或 AI GeneratedNode 功能。
- 不连接测试机、不读取 `C:\git\key`，除非后续获得明确用户名和具体密钥文件授权。
- 不引入第二后台语言、第二契约源、第二数据库体系或 WebApp 独立后台。

## Entry Gate

- [x] iteration-1 已关闭并归档，Review 为 `pass-with-accepted-findings`。
- [x] README Phase 0/1 范围和退出条件已读取。
- [x] 项目是已治理项目，GitHub `main` 已建立干净基线。
- [x] 用户给出中文文档、`web-dm`、根 `interface`、TDD、Quality 多轮、worker 清理和文档收敛要求。
- [x] 用户确认本 checklist 以及业务闭环、目录边界和 GitHub allowlist 扩展。
- [x] Orchestrator 将 `.codex/AGENTS.md` 与 `.claude/CLAUDE.md` 同步切换到 iteration-2。
- [x] Standing roles 完成 iteration-2 设计轮次，冲突已由 Orchestrator 路由。
- [x] Quality round-1 在任何实现前冻结验收用例、测试工具和红灯证据要求。
- [x] Review round-1 对详细设计与实施计划返回 `pass`（仅限计划就绪）。

## A. 治理与执行准备

- [x] 从 `main` 创建 iteration-2 独立分支/worktree；旧本地 `master` 保持只读且不推送。
- [x] Product 确认 Phase 0/1 业务闭环、用户价值和非目标，不扩大到 Phase 2。
- [x] Architecture 冻结 crate、领域对象、契约、数据库和依赖边界。
- [x] Interface 冻结 `web-dm/` 页面/代码布局以及根 `interface/` 后台契约布局。
- [x] Delivery 冻结工具版本、容器、CI、Migration、MinIO、构建和发布证据。
- [x] Quality round-1 选择自动化测试工具，定义业务验收、属性测试、迁移测试、契约测试和失败判定。
- [x] Orchestrator 将角色 outbox 合并成详细实施计划和互斥 worker 写入范围。

## B. Phase 0 — 仓库和契约基线

- [x] 建立 README 规定的 Rust Cargo Workspace、crate/binary 边界、`Cargo.toml`、`Cargo.lock` 和固定 `rust-toolchain.toml`。
- [x] 根目录 `interface/` 成为唯一后台接口与 Protobuf 契约源，按 core/market/research/app 等边界组织；同一契约生成 Rust、Python、TypeScript 类型。
- [x] 建立 PostgreSQL 16 SQLx Migration；能从空库执行、重复验证，并提供前向升级与数据校验入口。
- [x] 建立 MinIO bucket 命名、不可变对象路径、内容哈希和开发环境初始化规范。
- [x] 建立 Docker Compose 开发环境，覆盖 PostgreSQL、MinIO、Rust 服务及本轮所需依赖。
- [x] 建立 Python 3.12 GeneratedNode 基础镜像与依赖锁；只提供运行基线，不实现 AI 节点业务。
- [x] 在根目录 `web-dm/` 建立 React Platform Shell；页面设计与前端代码共置，预留多 WebApp 注册/加载边界。
- [x] 根目录 `interface/` 只承载共享后台契约和接口设计，不放具体 WebApp 页面设计。
- [x] 建立 Rust、Python、C++、TypeScript/Web 的格式化、静态检查、测试和依赖审计命令。
- [x] C++ 仅建立 Clang 18/CMake/Ninja 和稳定 C ABI 的可构建验证骨架，不提供任何伪造的定价/风险实现，也不把工具链自检当作 Phase 2 业务通过。
- [x] 建立 CI，执行契约生成防漂移、构建、测试、Migration 和依赖/许可证/敏感信息检查。
- [x] 在现有中文文档中合并开发、架构、质量和交付事实；仅新增 README 强制的中文 ADR 模板。
- [x] 更新 README 仓库结构：以根 `interface/` 取代原 `proto/` 契约入口，并准确反映 `web-dm/` 页面设计/源码共置规则。
- [x] 更新 `.gitignore`/发布 allowlist，允许根构建/CI 文件以及 `interface/`、`crates/`、`binaries/`、`migrations/`、`deploy/`、`python/`、`cpp/`、`web-dm/`、`tests/`、`domain-packs/` 进入 GitHub；继续排除 `.proqaid/`、工具约束、hidden、旧 `UI-DM/` 和临时 worker 资料。

## C. Phase 1 — 领域内核

- [x] 建立共享领域原语：ULID、版本、SHA-256 内容哈希、Decimal/单位、UTC/市场时区、所有者、状态和血缘引用。
- [x] 实现市场事实对象：`Instrument`、`Bond`、`FuturesContract`、`Cashflow`、`Calendar`、`Unit`、`Quote`、`Trade`、`Valuation`、`CurveSnapshot`、`MarketRulePack`。
- [x] 实现研究资产对象：`DataSnapshot`、`UniverseSnapshot`、`ExperimentRun`、`Artifact`、`SignalSet`、`RunJournal`。
- [x] 每个核心对象具有唯一 Protobuf 契约、Rust domain 类型、验证规则和 PostgreSQL 映射；不手工维护平行跨边界 DTO。
- [x] Definition/Run/Artifact 遵守版本化和不覆盖规则；发布后的 Artifact、Snapshot、SignalSet 与历史 RunJournal 不可原位修改。
- [x] Domain crate 保持纯净，不依赖 SQLx、网络、文件系统、Web、模型服务或容器运行时。
- [x] Repository 在 infrastructure/storage 层实现，并通过真实 PostgreSQL 集成测试验证创建、查询、版本与并发冲突。
- [x] Snapshot/Artifact 大对象引用 MinIO 内容地址；PostgreSQL 仅保存元数据、索引、状态和血缘。
- [x] RunJournal 使用追加语义，能够按 ExperimentRun 顺序读取并支持确定性重放检查。

## D. TDD 与真实业务验证

- [x] 所有生产行为遵守红灯 → 最小实现 → 绿灯 → 重构；worker 报告必须包含对应失败和通过命令证据。
- [x] Rust 单元/领域测试验证构造约束、状态迁移和错误类型；属性测试验证单位、时间、版本和不可变性不变量。
- [x] Protobuf 契约测试验证 lint、breaking policy、三语言生成和生成物防漂移。
- [x] PostgreSQL/MinIO 集成测试使用真实容器，验证空库 Migration、持久化、事务、并发、内容寻址和失败回滚。
- [x] 业务闭环测试使用真实 repository 和对象存储，验证完整市场事实 → 快照 → 实验 → Artifact/SignalSet → RunJournal 血缘。
- [x] 负向业务测试至少证明：非法单位被拒绝、历史版本不可覆盖、快照不随源数据变化、重复发布不破坏幂等性、断裂血缘不能形成正式 SignalSet。
- [x] Web 自动化测试覆盖 Platform Shell 启动、共享契约调用、错误/空状态和基础可访问性；不得把静态页面渲染成功当作业务闭环通过。
- [x] Quality 可在契约完成、领域实现中期和最终集成后再次启动；每轮使用 round-stamped inbox/outbox 和明确 verdict。
- [x] 最终质量报告区分单元、属性、契约、Migration、集成、端到端和业务验收证据，不用覆盖率数字替代闭环证明。

## E. Worker 编排与清理

- [x] Orchestrator 可派发 2–4 个临时 worker；优先按“仓库/工具链”“契约/领域”“存储/Migration”“web-dm/集成验证”分配互斥范围。
- [x] 每个 worker 使用独立 worktree/分支，prompt 明确 checklist 项、写入文件、禁止范围、TDD 命令、报告路径和清理要求。
- [x] 共享根文件（Workspace、锁文件、Compose、CI）由单一 bootstrap/integration owner 串行管理。
- [x] Standing roles 不写生产代码；角色对 worker 结果复核并经 Orchestrator 路由纠正。
- [x] worker 完成后清理 worktree、临时分支、报告草稿、生成缓存和测试数据；保留的证据合并到当前 Quality/Delivery 文档。
- [x] 生产代码中不得残留测试专用入口、fake 实现、硬编码成功数据、TODO 占位、一次性脚本或未使用 mock。

## F. 文档与目录规则

- [x] `docs/` 内本轮新增或更新的自然语言文档使用中文；代码标识、协议字段和必要命令保持原文。
- [x] 优先原位更新 `docs/product/scope.md`、`docs/architecture/data-dictionary.md`、`docs/quality/evidence.md`、`docs/delivery/release-notes.md`、`docs/review/audit-summary.md`。
- [x] 详细页面设计从现有 `docs/interface/ui-reference.md` 合并到对应 `web-dm/<app>/` 共置设计文件；迁移完成后删除重复旧文档或将其收敛为唯一索引，不保留两份事实源。
- [x] 后台接口设计、`.proto` 和契约兼容说明位于根 `interface/`；不得放入具体 WebApp 目录。
- [x] 除中文 ADR 模板、`web-dm` 共置页面设计和 `interface` 契约必需说明外，不新增平行设计/报告文档。

## G. Phase 0/1 Exit Gate

- [x] Ubuntu 24.04 LTS x86_64 环境可用一条命令启动本轮完整开发环境。
- [x] Rust、Python、C++ 和 Web 构建可重放，版本/锁文件/镜像摘要明确。
- [x] Protobuf 可生成 Rust、Python 和 TypeScript 类型，CI 能检测契约或生成物漂移。
- [x] PostgreSQL Migration 能从空库执行并通过数据校验。
- [x] Phase 1 全部核心对象可创建、查询、版本化和追踪。
- [x] Phase 1 全部核心对象具有 Protobuf 契约和 PostgreSQL 映射。
- [x] 历史对象、Snapshot、Artifact、SignalSet 和 RunJournal 不可被后续修改覆盖。
- [x] 真实业务闭环和规定的负向场景在真实 PostgreSQL/MinIO 环境通过。
- [x] Platform Shell 使用共享生成契约且不携带独立后台；`web-dm`/`interface` 边界经 Interface 复核。
- [x] Product 确认 README 和用户文档反映真实可用行为，不把 Phase 2+ 描述为已实现。
- [x] Architecture 确认依赖方向、领域不变量、契约与存储边界无偏离。
- [x] Quality 最终 verdict 证明业务闭环，不仅是测试命令绿色。
- [x] Delivery 确认构建、启动、Migration、CI、发布 allowlist 和可观测影响有证据。
- [x] Review 返回 `pass` 或 `pass-with-accepted-findings`。
- [x] Codex/Claude 约束同步；所有过期 role/worker/规划文件已清理或归档。

## Human Confirmation

确认本清单即表示授权 Orchestrator：

1. 将 iteration-2 切换为正式开发状态；
2. 更新工具约束和 GitHub allowlist 以包含经本清单批准的 Phase 0/1 源码根目录；
3. 运行全部常驻角色，并允许 Quality 多轮运行；
4. 在角色设计和详细实施计划再次完成后，派发 2–4 个临时 worker 实施；
5. 不自动授权测试机或密钥访问，除非另行给出具体连接信息。

## Closure Audit — 2026-07-13

**Operating level：** Full。业务与公共契约不变；本轮只处理依赖维护风险判断、候选证据、Quality/Review 与状态收敛。

- [x] 基线冻结为 GitHub `main` commit `737807302351fe8feee425a89d666caf3d611f96`。
- [x] 用户授权以现状评估 `RUSTSEC-2025-0052` 并完全收束 iteration-2。
- [x] 确认 advisory 类型、修复版本、生产可达链、上游可升级性与实际调用边界。
- [x] 独立 Quality 冻结并评判风险验收合同。
- [x] Delivery 运行候选绑定的依赖图、风险接受 fixture、供应链/仓库策略门禁。
- [x] 中文 Quality/Architecture/Delivery/Review 文档记录风险结论且不伪称已修复。
- [x] 内部 Review 对最终候选返回 `pass` 或 `pass-with-accepted-findings`。
- [x] GitHub CI 对最终候选通过，远程 `main` 与候选一致。
- [x] 清理 closure worktree/branch，并将本清单状态改为 `CLOSED`。

## Validity

Valid: iteration-2 only
