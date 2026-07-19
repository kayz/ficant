# ADR-0006：Windows 决策面、WSL 执行面与能力分级 Worker Pool

- 状态：Superseded by ADR-0007
- 日期：2026-07-13
- 决策者：Architecture authority，经用户明确确认
- 历史关联：ADR-0003、ADR-0005；当前配置不再实现本 ADR，见 ADR-0007 与 ADR-0009

> 本文仅保留为 superseded 历史决策。其 runner 与合同资产已归档到 `docs/history/hoqa/deploy-execution/`；文中的七角色、WSL 默认执行面、Delivery、Review、Profile Pool 和 runner 规则不得驱动当前 OPAID 工作或中央 CICD 发布。

## 背景

iteration-3 同时使用 Windows 主模型、Windows Git 工作区、WSL 编译/SIT 工具链、Codex/Claude CLI 和远程 UAT。早期执行把模型、权限、宿主和工具链隐式绑定在临时命令中，导致 Windows sandbox 用户看不到 WSL、名义 `workspace-write` 被误判为模型写入能力、同一测试在不同身份下重复验证，以及环境故障被错误归到 Worker。若角色继续直接拼接 `wsl`、CLI、路径和权限参数，执行复杂性会扩散到每个角色合同和业务模块，违反 ADR-0003 的深模块原则。

## 决策

### 三个执行区域

1. **Windows 决策面。** Orchestrator、Product、Architecture、Interface、Delivery、Quality、Review 在 Windows 主模型侧完成判断、合同、验收和文档维护。决策面只生成有界合同、选择 Profile、调用统一 runner、读取结构化结果/diff/证据、作出 Mentor verdict 和决定是否集成；不得用临时命令进入 WSL 修改代码、调试或安装依赖。
2. **WSL 执行面。** Development Worker 和 Test Worker 默认由 `ficant-ubuntu-24.04` 中的 CLI 执行。编译、测试、依赖、构建缓存和开发工具链归 WSL 管理。Windows Git 工作区现阶段可从 `/mnt/c` 使用，但 worktree、构建目录、数据集和服务实例必须按 Development、Test、SIT 隔离；构建输出、依赖缓存和临时证据优先位于 Linux 文件系统。
3. **UAT/发布目标。** VPS `47.100.66.40`、`greatquant.com`、应用名 `dm` 仍由 Delivery 管理。机械发布可以使用 Delivery 临时 Release Executor；普通 Development/Test Worker 无远程发布权限，不能获得 VPS/root key，密钥不得进入项目目录或执行合同。

当时的 Windows 稳定入口现归档为 `docs/history/hoqa/deploy-execution/invoke-wsl.ps1`；它使用参数数组完成 Windows/WSL 路径映射并调用同目录的 `run.sh`，不拼接 `bash -lc` 或临时命令。该 WSL runner 只解释历史执行证据，不是当前本地开发入口。

### Worker Profile Pool

Worker 是临时执行资源，不是第八个角色。路由维度固定为：

```text
能力等级 × 权限 × 执行环境 × 任务风险
```

| Profile | WSL Executor / 模型 | 默认范围 | 禁止降级的风险 |
|---|---|---|---|
| Strong | Codex CLI / `gpt-5.6-sol` | 数值算法、未知根因、高风险或跨模块修改 | YTM/久期/凸性/DV01、C ABI、unsafe、内存、并发、安全、事务、恢复 |
| Medium | Claude CLI / requested alias；结果必须记录 provider 实际模型 | 边界冻结的常规开发、根因明确缺陷、有限跨文件实现 | 未知根因、跨模块扩散或两次有界修正仍未解决时升 Strong |
| Fast | Codex CLI / `gpt-5.3-codex-spark` | fixture、manifest、mapping、adapter、lint/format、报告、确定性接口测试、机械小修 | 业务语义、Oracle/expected/容差及所有 Strong 风险 |

模型与权限正交。`Test Executor=read-only`，`Test Author=workspace-write`，`Development Worker=workspace-write`，`Quality/Review=read-only`，`Release Executor=Delivery-only`。Spark 可以写入，是否写入只由权限 Profile 和合同路径决定。

### 路由、升级与 Mentor

- 有界、机械且验证命令确定时优先 Fast；常规实现且边界冻结时可用 Medium；高风险、语义变化、未知根因或跨模块复杂性直接使用 Strong。
- Fast 只有 1 次合同内机械修正预算；首次非机械失败、根因不显然、扩大范围、弱化断言或触碰 expected/Oracle/容差，立即停止并升 Strong。
- Medium 与 Strong 各有 2 次合同内修正预算。Medium 出现未知根因或跨模块扩散时不消耗预算，立即升 Strong；两次有界修正仍未解决才返回阻断。Strong 在预算耗尽后返回可审计阻断，不通过扩大范围继续试错。
- Worker 请求改变冻结边界时停止并返回 Windows 决策面；Architecture/Quality 按权威边界决定是否修改合同。
- 环境、sandbox 或 runner 失败由 Delivery 修复环境或切换同等级执行器，不改变风险等级或伪装成模型 fallback。
- Worker 完成声明只是候选。Quality 在测试合同冻结和完整测试批次完成两个节点判断测试语义与证据；Orchestrator 依据已冻结合同处理批次内部的 Development/Test Worker 技术循环，并拥有 Git 集成与清理。

### 有界自恢复与角色介入节奏

一次 Worker 调用可以在同一 base、同一隔离 worktree、同一 allowlist 和冻结语义内完成“失败 → 诊断 → 允许范围内调整 → 重跑”。Fast 的最大修正次数为 1，Medium/Strong 为 2。完全相同状态下的盲目重试被禁止；每次修正必须记录失败分类、所作调整、重跑命令和结果。环境或证据打包的机械问题可使用预算，Fast 一旦发现非机械问题立即升级 Strong。

改变架构、接口、业务语义、expected、Oracle、容差、权限、路径、凭证、UAT 或合同范围属于立即停止条件，不计入修正次数。Worker 必须返回 Windows 决策面；它不能把扩大边界包装成“自恢复”。

可恢复阻断发生时，runner 不再无条件回滚到 base。只要差异位于 allowlist、未写 Git metadata、未违反权限/冻结边界，runner 就固化 patch/tree 并返回 `blocked-with-candidate`；该状态仅表示候选可被后续确定性合同接续，不表示可集成。路径、权限、凭证、Git metadata 或冻结边界违规属于不可信状态，必须恢复 base 后再派发。

Quality 的正常介入点只有 `test_contract_freeze` 与 `completed_test_batch`。中间命令错误、证据路径、环境打包和逐个缺陷循环由 Orchestrator 对照冻结合同处理；只有 expected/Oracle/容差/测试合同或业务验收需要变化时才重新进入 Quality。Review 只在 Design Freeze 和迭代退出介入；高风险边界变化先返回 Architecture，形成修订后的 Design Freeze 时再由 Review 判断，不设置临时逐轮 Review。

### Test Executor 的托管执行边界

`Test Executor=read-only` 的“只读”限定原始候选源码和 Git worktree，而不是禁止确定性测试产生任何临时输出。对于 `permission_profile=test-executor`，统一 runner 在 WSL Linux state 目录创建精确 base 的隔离 source snapshot，在其中执行合同冻结的结构化 argv；原 worktree 在执行前后必须保持同一 HEAD 和零 diff。命令、cwd、timeout 与 `expected_tests` 由合同冻结，runner 记录真实退出码、持续时间和 stdout/stderr 哈希，不能由 Spark 的文本声明覆盖。

确定性命令结束后，runner 才以真实 `gpt-5.3-codex-spark`、read-only sandbox 调用 Fast Test Worker整理 candidate brief。结果分别记录 `executor=codex` 与 `command_executor=runner-managed`；Spark brief 固定为候选信息，不构成命令证据、测试计数或 Mentor verdict。模型 admission 使用 Test Executor 专属 invocation revision，runner 变更只作废受影响的 Fast/read-only admission，不联动 Strong/Medium/Test Author。

runner evidence 持久化在 `${XDG_STATE_HOME:-$HOME/.local/state}/ficant/evidence` 并返回 SHA-256；scratch/build/source snapshot 位于 WSL state build root，证据捕获后删除。真实 managed canary 必须同时证明：原 worktree clean、scratch 可写、命令库存计数正确、实际 Spark 身份可验证、brief 不能改变 runner 事实、证据哈希可复算以及临时目录已清理。

### 合同与结果

所有新任务通过 `ficant WSL execution contract v4` 进入 runner。合同包含 checklist/task/case/acceptance/defect ID、Profile、requested model、环境准入时验证的 actual model、独立 `model_admission_fingerprint`、权限、环境 fingerprint、精确 base SHA、由 Orchestrator 通过 `PrepareWorktree` 预先创建的隔离 worktree WSL POSIX 路径、允许/禁止路径、冻结合同、expected/Oracle、最小上下文、RED/GREEN/regression、超时、结果位置、清理、Mentor、升级、fallback，以及与 Profile 一致的修正预算、候选保留规则和立即停止条件。Test Executor 的每条命令还必须以结构化 argv/cwd/timeout/`expected_tests` 表示；运行时 actual model、model-admission key 或 environment fingerprint 与合同不一致时 fail closed，只重新准入受影响的 Profile/permission，不联动重跑无关 Docker/toolchain 检查。

Worker 不能自行创建 worktree、切换分支、获取凭证、扩大范围，也不能 stage、commit 或写入 `.git`/Git metadata。写入型 Worker 的成功交付物是允许路径内的未提交工作树差异；runner 使用位于证据目录的临时 alternate index/object store，从精确 base 机械构造 `candidate_tree`、完整 binary `candidate.patch` 及其 SHA-256，不扩大 Worker 的 Git 权限。Quality/Orchestrator 验证候选后，只有 Orchestrator 可以把完全相同的候选树写成提交并执行集成；Worker 返回的 `candidate_sha` 在此之前仍等于 base SHA，集成 SHA 作为后续 Mentor/集成记录补充。

隔离 worktree 及候选提交都属于 runner 封装边界：Windows 决策面不得用 Windows Git 创建或直接提交供 WSL Worker 使用的 linked worktree。Orchestrator 只能调用稳定入口 `PrepareWorktree`，由 WSL Git 在固定 `worktrees/` 根下从精确 base 创建 `codex/` 分支；Mentor verdict 后只能调用 `IntegrateCandidate`。后者必须从结构化 result 重新验证 status、base、changed-files、candidate tree 与 binary patch SHA，提交后再次验证 parent/tree/clean postcondition。Worker 无这两个动作权限。

runner 验证 worktree/base、调用非交互 CLI、记录 provider 可验证的实际模型、检查 diff 路径并生成 `ficant WSL execution result v4`。结果至少绑定 base/candidate SHA、candidate state/tree/diff SHA、环境与模型准入 fingerprint、actual model、模型 executor 与命令 executor、权限、命令/退出码/持续时间/冻结测试库存、测试计数、Spark candidate brief、修正预算/已用次数/事件、模型执行与 runner 验证耗时、证据摘要、升级原因和清理；`verified-diff` 与 `blocked-with-candidate` 都只是 runner 固化的候选状态，schema 合法不等于 Mentor 通过。排队、环境等待和 Mentor 耗时不由 Worker 猜测，未知时记录 `null`，由拥有该阶段的权威补充。

### 迭代环境能力准入

当时每次迭代在 `ACTIVE` 前由 Product、Architecture、Quality、Delivery 声明所需能力，并与现已归档的 `docs/history/hoqa/deploy-execution/environment-capabilities.toml` 及持久化工具链 lock 比较。该准入流程仅说明历史证据，不是当前 OPAID round contract 或本地能力预检。

`fingerprint` 记录 distribution、runner identity、工具路径/版本/哈希、runner/config/toolchain lock 哈希、组件 fingerprint 与验证时间。身份哈希明确排除 `captured_at`，所以同一能力状态可稳定复用；时间仍作为审计事实保留。模型准入另以选定 CLI binary、实际模型、Profile、permission/sandbox 和显式 `model_invocation_revision` 计算 key。文档、测试数据、toolchain、Docker 或 SIT 服务状态变化不使模型 key 失效；CLI/model/permission 或模型调用逻辑变化才重跑对应 Profile。

模型与权限预检使用 WSL Linux 文件系统中的最小 Git canary，而不是检出完整 ficant 项目。canary 只证明 CLI、模型身份、read/write sandbox 和清理；业务 Worker 仍必须使用精确项目 base SHA 和隔离 worktree。Delivery 通过 `PrepareCaches` 幂等准备 `$HOME/.cache/ficant`、`$HOME/.local/state/ficant/build` 和 canary；共享仅限依赖缓存/canary，worktree、build/target、测试数据和服务实例仍按合同隔离。`ACTIVE` 后 runner/config 应冻结；确需变更时只作废其声明的 admission scope。

Worker 只引用已准入 fingerprint，不重复平台预检。未改变合同、dependency、fixture、migration、toolchain、runtime image 或测试的 SHA-bound 确定性证据允许复用；不同角色不得为了“再次确认”重跑相同全量套件。`ACTIVE` 后才发现的基础能力缺失登记为 `environment-admission-omission`，不计为 Worker 或模型失败。

### 容器运行时的所有权与生命周期

本机容器运行时固定为 **Windows Docker Desktop + WSL2 Linux engine**；`ficant-ubuntu-24.04` 只通过 WSL Integration 使用 Docker client/Compose，不另装或维护第二套 WSL Docker daemon。这样保留单一 daemon、镜像库和端口治理面，同时把服务部署命令留在 WSL POSIX 执行面。

- Delivery 对容器环境能力承担最终责任：Docker Desktop 可达性、WSL Integration、client/server/Compose 兼容、PostgreSQL/MinIO 等 SIT 服务的隔离部署、健康检查、fingerprint 和清理。
- Docker Desktop GUI 启动、管理员操作和启用 WSL Integration 属于 Human Operator 边界。Delivery 输出最小操作要求并在操作后验收；Human Operator 不是核心角色，也不获得环境 verdict。
- 可重复的部署、健康检查和清理由 Delivery 管理的临时 `Environment/SIT Executor` 执行。它是无模型的确定性 Worker Profile，不是第八个角色；只允许管理带 ficant 标签且属于合同指定 Compose project 的资源。
- 普通 Development/Test Worker 不获得 Docker socket，也不能启动/停止 daemon、切换全局 Docker context 或清理其他任务资源。测试合同只引用 Delivery 已准备好的隔离服务端点；正常 Test↔Development 缺陷循环不需要 Delivery 参与，只有环境事故返回 Delivery。
- Development、Test、SIT 使用不同 Compose project、端口、volume、network、bucket/database namespace 和生命周期标签。默认 project 名为 `ficant-<iteration>-<purpose>-<contract-id>`；清理必须验证 owner/iteration/contract 标签后执行并记录结果。
- 稳定入口 `invoke-wsl.ps1 -Action ContainerPreflight` 只做可达性和能力探测，不自动启动 Docker Desktop、不改变宿主配置、不读取 UAT/VPS/root key。环境部署必须使用 `environment-sit` 权限 Profile，发布仍使用 Delivery 专用 `release-executor`。

仅在 Docker Desktop 长期不稳定、许可/资源约束不可接受、CI 需要原生 Linux daemon，或隔离/性能指标持续不能满足时，才评估迁移到 WSL 原生 daemon 或远程容器运行时；迁移仍须保持同一能力合同、标签、证据和 Delivery 所有权。

### 跨平台保护

- 合同只使用 WSL POSIX 路径；Windows 路径由 PowerShell 入口映射，WSL runner 不接受临时 shell 字符串。
- 仓库执行配置固定 UTF-8/LF、大小写敏感语义；shell 入口由显式 `bash` 调用，Git tree 中需要直接执行的脚本必须真实跟踪 `100755`，不得以 Windows/DrvFS 的表观可执行性代替 Git mode 证据。
- `/mnt/c` 只承载源码/worktree；构建、依赖缓存和临时证据优先写入 WSL Linux 文件系统。
- Windows 决策面不直接编辑 WSL worktree。Quality/Review 只读候选 diff、结构化结果和证据；Orchestrator 在 Mentor verdict 后通过统一入口 `IntegrateCandidate` 提交已验证的同一候选树，该动作 fail closed，且不得改变候选文件内容。
- runner 不实现 UAT 发布或密钥读取；Release Executor 由 Delivery 单独管理。

## 选择依据

该拓扑把“如何执行”的平台差异封装进一个窄 runner，把“执行什么”和“是否接受”保留在七角色权威中。它消除临时命令的身份漂移，使同一合同能在 Fast/Medium/Strong 间升级而不改变业务边界，并让环境失败、模型失败、实现失败和 Mentor 拒绝成为可审计的不同事件。

## 风险与缓解

- **`/mnt/c` I/O、文件名大小写和 executable bit 差异。** runner 固定 POSIX 路径、LF/UTF-8 和显式 bash；构建缓存留在 Linux。若性能或语义问题持续，迁移执行 worktree 到 WSL ext4。
- **Windows Git linked-worktree 指针在 WSL 中不可解析。** 供 WSL Worker 使用的 worktree 统一由 `PrepareWorktree` 通过 WSL Git 创建并验证 POSIX gitdir；属性文件自身固定 LF。Windows Git 创建的旧 worktree 只保留历史证据，不再派发。
- **CLI alias 与实际模型不同。** Codex 使用精确 model slug；Claude 结果必须从 provider `modelUsage` 取得实际标识，未取得即 preflight/结果失败。
- **fingerprint 随身份或工具更新漂移。** 每次准入重新生成并绑定候选；Worker 不复用不匹配的 fingerprint。
- **显式 revision 未随模型调用逻辑更新。** `model_invocation_revision` 是受 Architecture/Delivery 审查的机器配置；任何 CLI argv、身份解析或 sandbox 映射变化必须同步递增，确定性测试验证字段存在。
- **canary 与业务仓库被混同。** canary 只用于环境/模型能力准入，结果使用独立 canary SHA；业务合同仍要求项目精确 base SHA、路径白名单和业务测试证据。
- **统一 runner 成为关键模块。** 配置、合同、结果和路径边界有确定性测试；runner fail closed，原始证据留在 WSL 执行系统，Mentor 不信任 Worker 自报绿灯。
- **workspace-write sandbox 可改源码却不能安全写 Git metadata。** 不扩大 `.git` 权限；runner 用临时 alternate index 固化候选 tree/patch，Quality/Orchestrator 审核后由 Orchestrator 提交。提交后的 tree 必须等于 runner 记录的 `candidate_tree`，否则拒绝集成。
- **Windows/WSL Git 对 linked worktree 的路径解释不同。** `PrepareWorktree` 与 `IntegrateCandidate` 都在 WSL Git 中执行；Windows 侧只传递规范化路径和 result identity，不直接改 WSL worktree/index/ref。
- **无 `.git` 的 archive snapshot 不能承载 Git 身份测试。** iteration-3 的首轮 managed Wave 1 证明 Oracle 会读取 `HEAD:<path>` blob identity；仅解压 `git archive` 会丢失该能力。后续 snapshot 必须是精确 base 的隔离 Git checkout/clone，同时保留原 worktree 零 diff。当前还发现 `fetch_quantlib.sh` 在 Git tree 中为 `100644`，Windows/DrvFS 曾掩盖该事实；Worker 不得越权写 Git metadata，mode-only 修复必须先由 Windows 决策面确认 Orchestrator 稳定入口方案。
- **Docker socket 等价于高权限宿主能力。** 普通 Worker 不获得 socket；只有 Delivery 管理的 Environment/SIT Executor 在受标签、project 和动作白名单约束时使用。宿主 GUI/管理员动作升级给 Human Operator。
- **共享 daemon 产生跨任务可变状态。** Compose project、端口、volume、network、数据 namespace 和生命周期标签必须按合同隔离；清理只能命中已验证归属的资源。
- **保留阻断候选可能被误当成通过。** `blocked-with-candidate` 必须带非空 blocker、base/tree/patch 哈希和未清洁 worktree 事实；它不能被集成，只有后续确定性合同完成缺失证据并取得 Mentor verdict 后才能转为 `verified-diff`。
- **远程发布权限误扩散。** release 权限 Profile 只属于 Delivery，通用 runner 明确拒绝 release 执行。

## 被否决方案

1. **Windows 直接执行全部 Worker。** Windows sandbox 身份与 WSL 工具链割裂，已导致重复验证和错误归因。
2. **每个角色自行拼接 `wsl`/CLI 命令。** 路径、权限、模型和证据逻辑会向七角色扩散，无法统一审计。
3. **所有任务一律 Strong。** 浪费低延迟机械执行能力，也不能替代合同和确定性工具。
4. **按代码行数选模型。** 小改动也可能触及 ABI/安全/业务语义；风险与不确定性比行数更重要。
5. **把 UAT 凭证交给 WSL Worker。** 扩大攻击面并破坏 Delivery 的最终责任。
6. **在 WSL 再安装一套 Docker daemon。** 会形成双 daemon、双镜像缓存、端口和故障归属分裂；当前没有足够收益抵消治理复杂度。
7. **让普通 Development/Test Worker 直接管理 Docker。** Docker socket 权限和全局可变状态超出其合同边界，也会让环境事故混入实现/测试结果。
8. **让 Worker 自行 stage/commit。** 这会把 Git metadata 权限、分支状态和集成责任扩散进每个 Worker sandbox，并使 Mentor 审核前后候选身份难以稳定；候选 diff 与 Git 集成因此被刻意分离。
9. **任一错误都回滚到 base 并重新派发。** 这会丢失已经通过核心测试的 allowlisted 候选，把证据打包或环境错误放大为整轮模型重建；有界自恢复和 `blocked-with-candidate` 在不放宽边界的前提下保留进度。

## 后果与迁移条件

- 后续 Worker 必须从 Profile Pool 和统一 runner 派发；旧 Windows CLI 记录只保留为历史证据，不再作为默认执行入口。
- 当前允许 `/mnt/c` worktree。出现可重复的 I/O 性能问题、大小写冲突、权限/锁冲突或构建不可重现时，Delivery 将执行 worktree 迁到 WSL ext4；合同和 schema 不变，仅路径映射与 fingerprint 变化。
- 若采用 CI/cloud executor，必须实现同一合同、权限、实际模型、fingerprint、证据和清理语义；不能绕过 Mentor 或七角色边界。
- QuantLib 只需在受控测试环境中可执行，用于独立验证冻结业务合同；不要求形成部署模式，不进入生产依赖、运行镜像、Artifact schema 或 UAT 发布包。未来更换 Oracle provider 时仍须保持同一输入/expected/容差合同与独立性证据。
- 当时的执行复杂性已归档到 `docs/history/hoqa/deploy-execution/`；当前业务模块仍不得引入 WSL、CLI、sandbox、provider 或历史 runner 路径类型。
