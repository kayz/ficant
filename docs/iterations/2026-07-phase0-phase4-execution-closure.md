# Phase 0 环境基线与 Phase 4 持久化执行收口

## 目标

- 在精确基线 `b68f000681391ee9216eebb9b3e8b19b8bb13486` 上收口 Phase 0 的本地 Ceph 开发环境、可重放构建和空库 migration 合同。
- 将 Phase 4 从进程内图执行收口为真实 PostgreSQL + Ceph RGW 的持久化 NativeNode 执行：冻结 graph/identity/RulePack/外部输入，租约领取和 fencing，Journal/checkpoint 恢复，Artifact 原子落账及多节点推进。

## 验收

- 一条明确的 Compose 命令可启动本地 PostgreSQL、Ceph RGW 和开发服务；正式 Rust 服务 Dockerfile 冷构建、多语言构建、契约生成和全部 PostgreSQL migration 可重放。
- graph 外部输入、RulePack 和所有影响计算的执行材料进入可复现身份；运行实例不污染计算或 Artifact digest。
- CGB 固收 NativeNode 解析既有 `AnalyzeBondRequest`，走 API 同一真实 Rust/C++ 计算路径并返回确定性 `AnalyzeBondResult`，没有模拟定价或桩实现。
- worker 以持久化 task/lease/Journals 逐节点执行；中断后按 checkpoint 恢复，过期 worker 被 fencing 拒绝；输出提升至 Ceph 后以一个 PostgreSQL 事务提交 Artifact、Journal、checkpoint 和任务/Run 状态。
- 真实 PostgreSQL 16 + Ceph RGW 故障注入覆盖 NodeStarted 后、对象提升后/事务前的 worker 丢失、重新领取及旧 fencing epoch 拒绝；真实存储覆盖两节点推进，进程内执行测试覆盖强类型依赖边和输入篡改失败。

## 非目标

- 不实现 GeneratedNode/gVisor、Phase 5 Rates Research Lab、DMQuant 业务 UI、信号发布、生产集群调度或外部交易执行。
- 不修改数值 Oracle、Golden Case 的 expected/容差或既有业务语义来制造通过。
- 不创建版本 tag、部署测试环境或将本地检查描述为 GitHub/Linux CI 通过。

## 公共契约变化

- 增加 `ficant.research.v1` 的 graph 与 execution Protobuf 合同及 Rust/Python/TypeScript 生成 consumer；Journal 前向扩展节点生命周期事件。
- ResearchGraph 可声明外部输入并将其绑定到节点端口；可复现身份显式绑定 external input content hash、RulePack ID/version/content hash、节点实现和运行环境，ExecutionInstance 单独绑定 Run ID。
- PostgreSQL 新增 Phase 4 graph、identity、RulePack、external input、node execution 与执行任务的持久化模型；任务冻结计划 Artifact ID，lease claim count 充当 fencing epoch。
- 同一可复现身份下，节点 Artifact ID 由节点身份稳定派生；多个 `GENERIC` Artifact 可以引用同一内容寻址 blob，非 `GENERIC` Artifact 原有 `(tenant, kind, content_hash)` 唯一性保持不变。
- 增加 CGB 固收 NativeNode 和 API 共享的请求解析/计算/结果路径；worker 装配该节点、PostgreSQL 执行存储和 Ceph 对象存储。

## 需 Human 决策

- 是否接受本地候选已经闭合 Phase 4 Rust NativeNode 持久化执行范围；这不授权 Phase 5 或 GeneratedNode。
- 是否指定精确 `v*` 版本号并授权版本交付。正式 Dockerfile 的精确锁定基础镜像冷构建、发布制品和测试环境 Compose 证据只能由该版本 Action 产生；未指定前不得创建 tag。

## 最终真实测试证据

- `pwsh -NoProfile -File scripts/check-fast.ps1`：exit code 0；Rust workspace 非环境测试、storage library 3/3、Phase 3A canonical ingestion 5/5、Phase 3B snapshot codec 2/2 全部通过。
- `cargo clippy --offline --workspace --all-targets --locked --exclude ficant-contracts --exclude ficant-contract-tests --no-deps -- -D warnings` 与 `cargo build --offline --workspace --all-targets --locked`：均 exit code 0。CMake/clang 19 Release 构建后 `ctest --output-on-failure`：exit code 0，8/8。
- 锁定 `buf 1.56.0` 镜像执行 format/lint/descriptor，随后以 Rust `1.96.1` 容器执行生成合同 consumer：均 exit code 0，Rust descriptor tests 13/13。锁定 Node `22.17.0`、pnpm `10.12.4` 离线 frozen install 后 typecheck/build/Vitest：均 exit code 0，4 个 test files、29 项测试通过。
- 锁定 uv `0.7.13`、Python `3.12.11` 容器离线执行 acceptance matrix、独立 Oracle 与 Python consumer：均 exit code 0；Q-001..Q-036、Phase 2B 16/16、Phase 2C 18/18 + Oracle 3/3、Phase 2D 18/18 + Oracle 3/3，Python tests 1 passed/1 skipped。
- 一次性 Compose PostgreSQL 16 + Ceph RGW 环境中顺序执行 migration、lease queue、Phase 4 repository、production worker、Phase 1、13 项负向不变量、Phase 2B/2C/2D、Phase 3A/3B 集成命令：整体 exit code 0。直接计数为 migration 4/4、lease 1/1、Phase 4 repository 1/1、真实 worker 1/1、Phase 1 1/1、负向不变量 13/13、Phase 2B/2C/2D 各 1/1、Phase 3A 2/2、Phase 3B 2/2；环境在完成后连同一次性卷清理。
- 真实 worker 测试使用两个实际 CGB NativeNode。第一次 worker 在 `NodeStarted`、真实 Rust/C++ 计算及 Ceph promote 后、PostgreSQL 完成事务前中断；租约过期后第二个 worker 以 attempt 2 重放 Journal 并完成，旧 fence 的完成请求被拒绝，随后第二节点完成且 Run 进入 `SUCCEEDED`。两个稳定 Artifact ID 可安全指向同一去重 blob，最终 Journal 重放得到两个完成节点。
- `verify-license-inventory.py verify ... --require-first-party`：exit code 0，inventory digest `ccbf68dd98c64f09a777ddc290309ad8b23737bb0578dbb59bad8e6cd71f0201`。`verify-repo-policy.sh --stage final`、supply-chain gate fixtures 与 repo-policy fixtures：均 exit code 0。
- 本机没有同时满足冻结版本的 Windows `uv`、Buf 和 Node，因此没有把 `check.ps1` 的宿主机工具预检失败包装成业务通过；上面的锁定容器命令逐项覆盖其多语言非环境检查，真实集成命令覆盖 `check.ps1 -IncludeIntegration` 的全部 11 个持久化步骤。

## 残余风险

- 本轮只装配一种受信任的 Rust NativeNode；GeneratedNode 的隔离、资源强制和不可信代码执行仍须在后续专门迭代实现。
- Ceph RGW 开发夹具证明本地 S3 兼容行为，不证明生产 Ceph 集群拓扑、Linux version CI、镜像扫描、GHCR 制品或测试环境部署。
- 本机 Docker Hub 对锁定 Rust builder 的缺失大层持续零吞吐，正式 `RustService.Dockerfile` 未在本地完成冷构建，完整开发栈也未以 `up --build --wait` 计为通过；同版本 Linux release 编译和既有 runtime 组装只是诊断证据。精确 digest 冷构建与测试环境交付必须由 Human 指定版本号后的 GitHub version Action 闭合。
- Phase 4 完成也不等于人可用的 Rates Research Lab；业务 UI、图编辑、实验浏览和人工工作流仍属于 Phase 5。
