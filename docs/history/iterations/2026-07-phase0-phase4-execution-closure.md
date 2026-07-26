# Phase 0 环境基线与 Phase 4 持久化执行收口

## 目标

- 从精确基线 `3ffe4fab4ad09cc584d7d685ea9440eec8189a7a` 完成 Phase 0 的一键完整开发环境：真实 PostgreSQL、Ceph RGW、Server、Worker、Web 与 React Platform Shell 使用正式构建边界启动并可实际走通 gRPC-Web。
- 把已有 Phase 4 持久化执行内核收口为可由生产入口提交、由受信身份约束、由 Repository 自行维护状态机、可持久查询/追踪/比较的完整闭环。
- 以真实的两节点 typed dependency graph 验证外部输入、Rust/C++ CGB 分析、下游风险摘要、Ceph Artifact、PostgreSQL lease/fencing、Journal/checkpoint、中断恢复与递归血缘。

## 验收

- `scripts/dev-up.ps1` 在不把 secret 写入仓库或命令行的前提下生成/复用 ignored 本地配置，并以一条命令启动完整开发栈；`scripts/dev-down.ps1` 默认保留 PostgreSQL/Ceph 数据卷。
- React UI 的 `/ficant-api` 由 Nginx 精确反代到 Server；最终 UI 镜像与真实 Server 的 gRPC-Web 功能测试必须成功，不能用 root div 或 `/health` 冒充业务连通。
- README 命名的 ADR 模板存在，repo-policy 对模板、Phase 4 Protobuf 和全部 13 个 migration 的防漂移合同有测试。
- 生产提交入口从已认证 scope、required-read Snapshot/RulePack/Artifact、冻结 graph 和受信 deployment attestation 构造身份；客户端不得自报 runtime、environment 或 implementation digest。
- 提交在一个 PostgreSQL transaction 内创建并启动 Run、写入 Run Journal、发布 graph/identity/bindings，并只 enqueue Journal replay 指定的拓扑首节点；相同幂等请求精确重放，任一漂移冲突。
- enqueue/begin/complete 均由 Repository 加载冻结 graph 与 Journal 校验合法 resume node；worker 不再提供 successor，Repository 自行派生唯一后继或 terminal。
- Ceph promote 产生的 verified blob proof 必须绑定 tenant、owner、hash、size 和 planned Artifact；NodeOutputManifest 的 execution、node、attempt、合同、实现、输入来源、输出类型/hash、Artifact 与 manifest hash 任一漂移都使整个 PostgreSQL transaction 回滚。
- worker 使用部署注入的真实 OCI manifest digest、canonical environment attestation 和构建生成的 source digest 校验持久身份；不匹配在执行前失败关闭。
- 持久查询可读取 graph run、validated node manifest/checkpoint，递归追踪任意输出至全部上游与外部输入，并比较两次持久运行的 Data/Universe/Graph/Parameters/Runtime/Environment/Seed/RulePack/Implementation/ExternalInput/Result 差异。
- 真实 PostgreSQL 16 + Ceph RGW worker SIT 使用 `AnalyzeBondRequest -> AnalyzeBondResult -> RiskSummary` typed edge，覆盖 promote 后/事务前中断、attempt 2 恢复、旧 fence 拒绝、上游篡改失败、下游 Artifact 血缘和 Run `SUCCEEDED`。
- 最终候选通过 `check-fast.ps1`、`check.ps1`、`check.ps1 -IncludeIntegration`、完整 dev Compose smoke 和发布候选预检；版本 CI 必须在 GitHub Runner 执行 lease、Phase 4 repository、真实 worker 与提交/查询集成。

## 非目标

- 不实现 GeneratedNode/gVisor、Phase 5 Rates Research Lab 页面、图编辑器、信号发布、生产集群调度、外部交易执行或生产环境发布。
- 不修改数值 Oracle、Golden Case expected/容差或既有固定收益业务语义来制造通过。
- 不把 Platform Shell 扩展为 Phase 5 业务 UI；本轮 UI 只闭合 Phase 0 的真实静态资源、会话和 gRPC-Web 平台链路。
- 不创建、移动或复用旧版本 tag；新的版本交付必须等待 Human 明确指定精确 `v*` 版本号。

## 公共契约变化

- `ExperimentService` 加法式增加 graph execution 提交、持久状态/manifest/trace 查询与运行比较；既有 Phase 1 RPC 保持兼容。
- Phase 4 application port 增加原子提交、validated persistent read/trace/compare；`CompleteNode` 不再接受调用方提供的后继任务，并强制 verified blob proof 与 canonical manifest。
- 受信执行身份增加 deployment OCI manifest、environment attestation 与 relevant-source digest；worker catalog 增加真实 `CgbBondRiskSummaryNativeNode`。
- dev/test Compose 与 Nginx 增加 React UI、真实 `/ficant-api` 路由和 Server 的 PostgreSQL/S3/attestation 配置；本地一键脚本只写 ignored 用户配置。
- version-tag CI 的真实 PostgreSQL + Ceph job 增加 Phase 4 lease、repository、worker 和提交/查询集成门，普通 branch、Pull Request 与 `main` 合并仍不触发完整 CI。

## 需 Human 决策

- 本地候选、PR 与发布候选预检全部通过后，Human 仍须给出新的精确 `v*` 版本号，才能授权 GitHub version CI、镜像构建/扫描和测试环境部署；不得由 Agent 推断版本号。

## 最终真实测试证据

- `pwsh -NoProfile -File scripts/check-fast.ps1`：exit code 0；workspace check、全部非环境 Rust tests/doc tests、Storage library 3/3、Phase 3A 5/5、Phase 3B 2/2 通过。
- `pwsh -NoProfile -File scripts/check.ps1`：exit code 0；Rust strict Clippy/build/test、生成合同 13/13、C++ 8/8、Q-001..036、Phase 2B 16/16、Phase 2C 18/18 与 Oracle 3/3、Phase 2D 18/18 与 Oracle 3/3、Python live SDK 1/1、Web test files 4/4 与 tests 29/29 全部通过。
- `pwsh -NoProfile -File scripts/check.ps1 -IncludeIntegration`：exit code 0；在本轮专用 PostgreSQL 16 + Ceph RGW 上，migration 4/4、lease 1/1、Phase 4 application submit/query/repository 3/3、真实两节点 Worker 1/1、Phase 1 1/1、负向不变量 13/13、Phase 2B/2C/2D 各 1/1、Phase 3A 2/2、Phase 3B codec 2/2 + publication 1/1 全部通过。
- `pwsh -NoProfile -File scripts/dev-up.ps1`：exit code 0；五个正式候选镜像完成冷构建，七服务健康，Worker runtime/source identity 从实际镜像派生，React UI 经 `/ficant-api` 取得已认证 Session 与 `grpc-status: 0`。`scripts/dev-down.ps1`：exit code 0；本轮容器和网络删除，`ficant-dev_postgres-data` 与 `ficant-dev_ceph-data` 保留。
- `python .github/scripts/tests/test_compose_security_gate.py -v`：exit code 0；33 项通过，其中 2 项需显式 live 标志的 Docker gate 跳过；release Compose 已验证 Server/Worker 身份一致、UI bearer 注入和受管 Ceph 合同。`bash -n` 覆盖全部测试部署脚本、workflow YAML 解析及 `git diff --check` 均为 exit code 0。
- Buf `format`/`lint`/相对基线 breaking 及 Rust/Python/TypeScript 生成均为 exit code 0；最终 descriptor hash 为 `81cede8c016bea5278d13fda68dd96ed8ba84dede1d83caf25e30367d854f6bf`。

## 残余风险

- Release candidate preflight 只允许在 clean `main == origin/main` 上运行，因此须在 PR 合并后执行；当前本地通过不能冒充五镜像 Trivy、Linux Runner、GHCR 或测试环境证据。
- 测试环境新增独立 cursor key 与 UI bootstrap bearer secret，并由部署时实际 Worker 镜像派生 runtime/source identity；在新版本 Action 成功前，远端环境仍不能视为已更新。
- 运行比较的 `Result` 维度当前取冻结拓扑的最后节点，适用于本轮线性两节点图；未来支持多 sink DAG 时应比较全部 terminal outputs。
- Trivy 0.72.0 不支持 Ceph 基础镜像的 CentOS Stream 9 OS family；语言包扫描不能替代完整 OS 包漏洞覆盖。
- Phase 4 退出不等于 Phase 5 产品完成；当前可用界面仍是 Platform Shell，不包含图编辑器、Lab 或 GeneratedNode/gVisor。
