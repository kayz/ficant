# ADR-0007：Windows 主开发环境与阶段特定环境

- 状态：Accepted
- 日期：2026-07-15
- 决策者：Human，经 Orchestrator Architecture lens 形成方案
- 取代：ADR-0006
- 关联：ADR-0002、ADR-0003、ADR-0004、ADR-0005

## 背景

ADR-0006 曾把 Windows 固定为决策面、WSL 固定为普通 Worker 执行面，并把 WSL
runner、缓存、工具链与 Docker 可达性作为统一准入。这些资产及其证据记录了真实历史，
但把开发、SIT、发布和兼容验证的环境责任混在一起。当前用户确认的政策是：Windows 是
普通开发与测试环境；环境能力应在实际进入相应阶段时检查；Orchestrator 的 delivery work 负责
SIT 与发布准备，Human 负责特权操作，二者都不管理普通 Worker 的本地循环。

## 决策

### Windows 普通执行基线

主模型、Development Worker、Test Author 和 Test Executor 使用 Windows 11、PowerShell 7、
Windows Git/worktree 和 Windows 路径。普通工作不得解析通用
`C:\Windows\System32\bash.exe`、调用 `wsl`，或经 `/mnt/c` 路由。仅当合同明确指定 Git Bash
时，才可用它解释仓库中的 `.sh` 兼容脚本。

只有至少两个独立、有界任务可并行且收益高于协调成本时，Orchestrator 才唤起 Development Worker，
Quality 才唤起 Test Worker。Worker 使用预建的隔离 Windows worktree 和同一套精确 base、allowlist、
冻结合同、自测、回归、修正预算及清理规则。单一顺序任务由对应 HOQA 参与者直接完成，避免把
外部 CLI、模型服务和 sandbox 生命周期变成默认切换成本。内部子 agent 继承当前主模型能力，
Fast/Medium/Strong 首先表示任务风险与验证强度，不强制对应一次外部模型切换。

Test Executor 继续由 runner 执行确定性的 read-only 命令；内部子 agent 可整理证据和测试 brief，
但不能代替 runner 的退出码、计数或权限判定。Spark、Claude 与 Strong Codex 外部 CLI Profile
保留为显式 fallback、容量/兼容验证或用户指定执行器，使用前仍须通过相应 preflight 并记录实际
模型。Test Executor 为 read-only；Test Author 与 Development Worker 为 isolated
workspace-write。Worker 声明只是候选；Development 结果由 Orchestrator 判断，Test 结果由 Quality
汇总为 test report，最终业务接受仍由 Human 决定。

### 模块与 runner 封装边界

业务模块只依赖任务合同、测试入口和结构化结果，不依赖具体 shell、宿主路径、缓存或 runner
实现。普通 Windows runner 负责精确 base、allowlist、权限、实际模型、命令/退出码、测试计数、
证据和清理。任务本地工具缺失由 Worker/runner 报告；共享 runner 损坏由 Orchestrator 处理。
不得把这些责任转交给交付流程，也不得通过业务模块调用 WSL。

ADR-0006 所述 WSL runner/config/schema/evidence 保留为历史来源，当前不删除、不扩展，也不作为
普通 Windows 工作的准入条件。`I3-D-RUNNER-005` 与 `I3-D-RUNNER-006` 只保留为历史 WSL
发现；不为其实施 mode-only integration。

### 阶段特定环境所有权

- **Development/Test：** Windows PowerShell 7；只检查当前任务需要的工具和能力。普通缓存、
  scratch、本地工具及测试不属于交付工作。
- **SIT：** Test/Operate 阶段需要 SIT 时，由 Orchestrator 的 delivery work 从 Windows 管理 Docker
  Desktop、Compose、PostgreSQL、MinIO、服务/数据隔离、健康检查和清理；Human 负责 Docker Desktop
  GUI、启动、管理员和 WSL Integration 操作，WSL Integration 不是前置条件。
- **Release/Migration/Runtime/UAT：** Orchestrator 准备制品、迁移/回滚和运行合同；Human 授权并执行
  凭证相关或低可观测操作。目标保持 `47.100.66.40`、`greatquant.com`、应用 `dm`；普通 Worker 不得读取或复制凭证。
- **Cross-OS：** Linux 只在明确命名的 compatibility、CI/container、release 或 UAT 门禁执行，
  不作为 ACTIVE 或普通开发的普遍准入条件。

## 选择依据

该决策使日常开发环境与当前宿主、权限和路径一致，并按阶段隔离环境责任。业务与测试合同仍由
稳定 runner 边界封装；SIT 和发布保留更严格的 Orchestrator/Human 操作边界。它避免因未进入阶段的工具缺失而
阻塞开发，同时保持跨平台、部署和运行证据可审计。

## 风险与缓解

- **Windows C++ 工具不可用。** 先提供任务本地 Windows CMake/Clang/Ninja 能力，再运行 CTest；
  未执行前不得声称 CTest 4/4。
- **Windows 与 Linux 行为差异。** 在明确的跨 OS 门禁使用同一冻结输入、容差和制品身份验证，
  不把该门禁扩散到每次普通 Worker 调用。
- **历史 WSL 文档被误读为当前政策。** ADR-0006 显式标记被本 ADR 取代；checklist 仅把 WSL
  资产和发现列为历史证据。
- **交付权限扩散。** Orchestrator 的 delivery work 只准备 SIT、release、migration、rollback、runtime
  与 UAT 合同；Human 保留凭证和低可观测操作，不接管普通 Worker 工具、缓存、scratch 或测试。
- **凭证暴露。** UAT 凭证不进入普通 Worker 合同、输出或项目文件，仅在 Human 授权的操作中使用。

## 被否决方案

1. **继续把 WSL 作为所有普通 Worker 的执行面。** 与确认的 Windows 开发政策冲突，并把跨 OS
   路径、Git mode 和 sandbox 差异变成日常阻断。
2. **把 WSL 或完整 SIT 能力作为 ACTIVE 的统一准入。** 检查了当前阶段未使用的能力，混淆开发
   与交付门禁。
3. **由交付工作管理所有 Worker 缓存、工具和测试。** 扩大权限并使正常开发循环依赖
   不相关角色。
4. **立即删除 WSL 资产与证据。** 会破坏历史来源；退役应在单独、明确授权的任务中完成。

## 证据复用与迁移条件

已接受且输入、实现、Oracle、容差及候选身份未改变的证据继续有效。当前 Windows Oracle 自测为
`15 + 15 + 1 = 31/31`，绑定 base `ddbc0cc`。当前 Windows 已验证 CMake 3.31.6、Ninja 1.12.1、
standalone LLVM 18.1.8、Visual Studio LLVM 19.1.5 和 Rust 1.96.1。未集成 runner 候选已有真实
VS LLVM 19.1.5 CMake/configure/build/CTest `0/0/0`、4/4 证据，但其完整 runner suite 仍在 linked-worktree
registration 处失败且根因未证，因此该候选和证据尚未被当前基线接受。

WSL runner/config/evidence 不迁移为当前普通 runner，也不在本 ADR 中删除。未来只有明确的
compatibility、CI/container、release 或 UAT 合同可调用跨 OS 资产；其合同必须冻结目标、路径、
制品身份、命令、退出码和清理。另行退役 WSL 资产时必须先保留所引用证据并更新所有当前链接。
