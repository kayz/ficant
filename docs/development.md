# FICANT 本地开发与测试

FICANT 使用 OPAID 组织本地开发、测试候选和 Human brief，使用中央 `cicd` 平台处理 Human 建立的版本候选。本文只描述本地边界；服务器、GHCR、GitHub Environment、部署和回滚不属于本地入口。

## OPAID round contract

每一轮开始前由 Root Orchestrator 冻结：

1. 一个明确的代码结果和一句可验证的 acceptance sentence；
2. 非目标以及不得修改的业务语义、接口、Oracle、expected 和容差；
3. 精确 base Commit SHA 和最终候选身份；
4. 规定的本地自测命令与风险相关回归命令；
5. 有界任务的依赖关系、允许和禁止写路径；
6. 只有 Root/Human 能改变的公共契约和业务决定。

Worker 只承担一个有界任务，返回 changed files、实际命令、exit code、可得的 test count、blocker 和 residual risk。Root 检查真实 diff，在所有修改完成后对精确集成候选重新运行规定测试。OPAID 到“本地自测候选与完成的 Human brief”即结束，不等待远端 CI，也不创建仓库内状态 ledger、agent registry、mailbox 或治理 checklist。

## 单一迭代 brief

每个迭代恰好有一份面向 Human 的 brief，位于 [`docs/iterations/`](iterations/README.md)。它是 Human 冻结范围、处理必要决策和阅读最终证据的唯一迭代文档，只能包含：

1. 目标；
2. 验收；
3. 非目标；
4. 公共契约变化；
5. 需 Human 决策；
6. 最终真实测试证据；
7. 残余风险。

Agent 交流、Worker 证据、失败诊断和中间候选关系保留在编排工具与实际命令输出中，不要求 Human 阅读。不得为这些信息生成仓库内状态页、子任务 brief、治理 checklist 或每日进度副本。最终证据必须在同一 brief 中记录实际命令、exit code 和可得的 test count；精确候选身份由最终 Git commit 与 Pull Request 事实源绑定，不复制进会参与候选哈希的 tracked brief。计划命令、`-ListOnly` 输出和 Worker 文字声明都不能冒充测试通过。

## 快速子循环与 forward-only checkpoint

一个迭代可以拆成多个快速子循环，每个子循环必须同时具备：

- 一个可交付结果；
- 与该结果风险匹配的针对性测试；
- 一个 forward-only checkpoint，明确已验证且可保留的候选事实，以及下一循环只能继续推进的边界。

子循环不产生独立治理文档，不重新定义或削弱 Human 冻结的迭代验收。局部失败从最近兼容 checkpoint 继续 forward-only 修复，保留其他已经验证的结果；只有公共契约或业务语义需要变化时才返回 Root/Human 决策。

OPAID 是这套候选关系的默认表达方式。只有真实失败记录能够证明现有 OPAID 无法表达依赖、兼容性或 forward-only 恢复关系时，才讨论修改治理；不得因为偏好、预防性设计或方法论整理而改写流程。

## 统一命令

快速检查适合提交前反馈：

```powershell
.\scripts\check-fast.ps1 -ListOnly
.\scripts\check-fast.ps1
```

它运行 `cargo fmt`、离线 workspace check、R5D 精确 crate 邻接表与 L1→L2 语法依赖门禁、非环境 Rust 测试和 Storage library 测试；不会连接 PostgreSQL、Ceph RGW、GitHub 或目标服务器。R6A 真实输入平面 SIT 被显式标记为 integration-only，普通 workspace test 只报告 ignored；未知 workspace package/依赖边，以及 `research/**` 对 analytics/curves/futures 模块的直接或 façade 引用，都会默认失败。

完整本地回归：

```powershell
.\scripts\check.ps1 -ListOnly
.\scripts\check.ps1
```

它在不依赖目标服务器的前提下运行：严格 Rust format/Clippy/build/test、生成契约与 R5D 结构门禁、冻结 `cgb-futures` 与 R5E `cgb-interest-tax` RulePack 的确定性 payload 漂移检查、C++ Release build 与当前 CMake/CTest catalog 中登记的全部测试、既有 acceptance matrix/独立 Oracle/确定性 Artifact 回归、R5D 独立 Decimal KRD Oracle、R5E 税收调整 Decimal Oracle、20 个一方包的许可证绑定、Python 契约测试，以及 Web typecheck/build/Vitest。默认不运行需要持久化服务的测试。CTest 数量由当前 catalog 决定，文档不另行硬编码一个会漂移的计数。

可选本地集成回归：

```powershell
.\scripts\check.ps1 -IncludeIntegration -ListOnly
.\scripts\check.ps1 -IncludeIntegration
```

调用者必须先提供一次性的本地 PostgreSQL 与 Ceph RGW，设置以下环境变量：

- `FICANT_TEST_DATABASE_URL`
- `FICANT_TEST_S3_ENDPOINT`
- `FICANT_TEST_S3_BUCKET`
- `FICANT_TEST_S3_ACCESS_KEY`
- `FICANT_TEST_S3_SECRET_KEY`
- `FICANT_TEST_RUNTIME_IMAGE_DIGEST`

脚本不会创建、部署或清理服务器，也不会打印这些值。数据库必须可以安全地被测试 migration 重置；不得指向共享、测试发布或生产数据库。集成计划依次覆盖 migration、Phase 1 正向业务闭环、13 项负向不变量、Phase 2B Carry/Roll-down、Phase 2C 国债期货交割、Phase 2D 套保、Phase 3A/3B 数据与快照，以及 R6A 生产 Definition/Fact/Snapshot 输入平面的授权导入、零副作用拒绝、重启重读和治理证据。

仓库内 `deploy/dev/docker-compose.yml` 是当前唯一的本地 Compose 拓扑，提供锁定基础镜像摘要的 PostgreSQL、单节点 Ceph RGW、migration、三个 Rust 服务和 React Platform Shell；它不是生产 Ceph 部署模板。日常开发只调用下面的包装脚本：

```powershell
.\scripts\dev-up.ps1 -ListOnly
.\scripts\dev-up.ps1
```

首次启动会生成 ignored 的 `deploy/dev/.env.local`；后续启动复用它。该文件包含一次性本地 PostgreSQL、S3、Platform、cursor 和 bootstrap 身份凭据，不进入 Git，也不应复用共享、测试发布或生产凭据。脚本先用正式 Dockerfile 构建完整拓扑，从实际 `ficant/worker:dev` 镜像取得 OCI image ID 和内嵌 native source digest，再以这些受信值启动服务；最后经 UI `/ficant-api` 调用真实 `GetCurrentSession` gRPC-Web，必须同时取得已认证 Session 和成功 trailer。可选端口仍可在启动前通过 `FICANT_POSTGRES_PORT`、`FICANT_S3_PORT`、`FICANT_SERVER_PORT`、`FICANT_WORKER_PORT`、`FICANT_WEB_PORT` 与 `FICANT_UI_PORT` 覆盖。

停止容器和网络但保留 `postgres-data`、`ceph-data` 命名卷：

```powershell
.\scripts\dev-down.ps1 -ListOnly
.\scripts\dev-down.ps1
```

`dev-up.ps1` 可能为了冷构建或缺失的锁定基础镜像访问网络。它不会替代 `check.ps1 -IncludeIntegration` 所需的 `FICANT_TEST_*` 变量；测试调用者仍须把本地服务地址和本轮精确 runtime image digest 映射到测试变量。包装脚本不提供删除数据卷的选项；如确需销毁本地数据，必须单独审查精确 Compose project 和卷目标。检查脚本本身仍不自动启动、停止、清理或下载该夹具。

## 本地依赖能力

仓库不通过检查脚本安装任何依赖。缺少工具、缓存或锁定版本时会失败并说明原因。

| 能力 | 仓库合同 |
|---|---|
| PowerShell | PowerShell 7，原生 Windows 路径和参数数组 |
| Rust | `rust-toolchain.toml` 锁定 Cargo/Rust 1.96.1，crate 必须已在本机 Cargo cache；命令使用 `--offline --locked` |
| C++ | CMake 3.31.6、Ninja、Visual Studio Build Tools LLVM 19.1.5 x64 `clang++`；不回退到 standalone Clang 18 |
| Python | uv 0.7.13、Python 3.12 与 `python/uv.lock` 所需 wheel 必须已缓存；命令使用 `--offline --locked` |
| Web | Node 22.17.0、Corepack 缓存中的 pnpm 10.12.4，以及已完成 frozen-lockfile 安装的 `web-dm/node_modules` |
| Protobuf | Buf 1.56.0；Windows 可用 `FICANT_BUF` 显式提供已核验的可执行文件路径 |
| 集成测试 | 仅在 `-IncludeIntegration` 下使用调用者提供的本地一次性 PostgreSQL/Ceph RGW 和固定 runtime image digest |

`COREPACK_ENABLE_NETWORK=0` 只在完整检查的执行范围内设置并恢复。Cargo 和 uv 同样使用离线模式，因此脚本不会因为缺少缓存而静默联网安装。

完整检查对产品 Rust target 执行 `clippy -D warnings`。`ficant-contracts` 的生成文件与 `ficant-contract-tests` 的 descriptor inventory harness 不进入 Clippy；它们由锁定生成器的一致性检查和独立 contract test 覆盖，避免把生成器风格告警误报成手写产品代码缺陷。

## 交接给 CICD

OPAID 先把精确 Commit SHA 的本地自测候选和唯一 brief 交给 Human。brief 包含真实 changed files、命令、退出码、测试数量和剩余风险。Human 有两个迭代验收选择：

1. 在同一精确候选上运行一次完整本地检查（本地 CI），并完成风险相关的人工复测；
2. 按 brief 和已有证据验收，合并并直接进入下一迭代。

两个选择都不启动 GitHub 完整 CI。普通 branch push、Pull Request 和 `main` 合并只维护远端源码历史；本地检查是普通迭代的正式证据，但不能冒充 Linux Runner、在线供应链、制品或目标环境证据。

准备建立版本候选时，先更新本机 Trivy 0.72.0 漏洞数据库，然后在干净且与 `origin/main` 精确一致的 `main` 运行：

```powershell
trivy image --download-db-only
.\scripts\check-release-candidate.ps1
```

该入口不创建 tag、不推送镜像、不连接测试服务器，也不安装工具。它使用正式 Dockerfile 和固定基础镜像 digest 构建 Server、Worker、Web、UI 四个应用镜像，验证 `deploy/storage-runtime.lock.json` 后复用并扫描锁定的 Ceph OCI digest；所有五个运行镜像都以本地 Trivy 数据库执行 `HIGH,CRITICAL`、`ignore-unfixed` 扫描。随后启动 PostgreSQL、锁定的 Ceph RGW、真实 Worker、Server、Web 和 UI，验证 readiness、UI 与 forward-only migration 兼容。只有该入口通过且 Human 明确给出版本号后，才创建新的不可变版本 tag。

数个迭代后，Human 确定版本号并创建符合版本格式、指向当前 `main` 精确提交的不可变 `v*` tag。创建 tag 即把版本候选交给 CICD，并授权完整 GitHub CI、SHA 镜像构建、扫描、不可变版本标签提升、Linux 测试环境部署、健康/冒烟检查和失败回滚。版本失败后不得移动原 tag；修复进入新的 OPAID 迭代，再建立 forward-only 版本候选。普通 OPAID 工作不得修改 `.github/**`、`cicd.yml` 或 `deploy/**` 来绕过这个交接边界。

本地镜像验证必须使用仓库正式的 `deploy/dev/RustService.Dockerfile` 做冷构建，并保留其精确候选和构建结果。为了排查网络或环境问题而使用的临时 Dockerfile、复制宿主机构建产物后组装的 runtime 镜像，最多只能作为诊断证据，不能冒充正式镜像构建通过。版本制品的最终证据只能来自 Human 创建版本 tag 后触发的 GitHub version Action：它在 Linux Runner 上用正式 Dockerfile 构建并发布 Commit SHA 标识的不可变镜像。普通 branch、Pull Request、`main` 合并以及本地临时镜像均不得被描述为这项最终证据。
