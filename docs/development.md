# FICANT 本地开发与测试

FICANT 的日常开发由 agent 自行组织，以 Pull Request 交付可验证的候选；版本发布只由 Human 创建的版本 tag 触发，发布文件由本仓库拥有（[ADR-0021](architecture/adr/0021-ficant-owns-release-files.md)）。本文只描述本地边界；服务器、GHCR、GitHub Environment、部署和回滚不属于本地入口。开发约定见 [`AGENTS.md`](../AGENTS.md)，决策依据见 [ADR-0019](architecture/adr/0019-lightweight-development-keep-release-boundary.md)。

`docs/iterations/` 下的 brief 是 2026-10-19 之前的历史证据，不再新增。

## 统一命令

快速检查适合提交前反馈：

```powershell
.\scripts\check-fast.ps1 -ListOnly
.\scripts\check-fast.ps1
```

它运行 `cargo fmt`、workspace check、R5D 精确 crate 邻接表与 L1→L2 语法依赖门禁、R7B MANUAL/恢复清单负向 fixture、R8A/R8B Portfolio 契约、本地契约包、18-service 拓扑、R8B domain/application/API focused tests、非环境 Rust 测试和 Storage library 测试；不会连接 PostgreSQL、Ceph RGW、GitHub 或目标服务器。R6A 真实输入平面 SIT、R6B Artifact 生产拓扑 SIT、R7B 真实 source-destroy/fresh-restore 与 R8A/R8B Portfolio PostgreSQL/gRPC SIT 被显式标记为 integration-only；未知 workspace package/依赖边，以及 `research/**` 对 analytics/curves/futures 模块的直接或 façade 引用，都会默认失败。

完整本地回归：

```powershell
.\scripts\check.ps1 -ListOnly
.\scripts\check.ps1
```

它在不依赖目标服务器的前提下运行：严格 Rust format/Clippy/build/test、生成契约与 R5D 结构门禁、冻结 `cgb-futures` 与 R5E `cgb-interest-tax` RulePack 的确定性 payload 漂移检查、C++ Release build 与当前 CMake/CTest catalog 中登记的全部测试、既有 acceptance matrix/独立 Oracle/确定性 Artifact 回归、R5D 独立 Decimal KRD Oracle、R5E 税收调整 Decimal Oracle、R8A Portfolio Decimal Oracle、R8B Portfolio Performance Decimal Oracle、20 个一方包的许可证绑定、Python 契约测试、可重复本地 `@ficant/contracts-generated@0.0.0` 包，以及冻结 Node 22.17.0 下的 Web typecheck/build/Vitest。默认不运行需要持久化服务的测试。CTest 数量由当前 catalog 决定，文档不另行硬编码一个会漂移的计数。

R9A 后，显式运行 `scripts/package-contracts.ps1` 仍会在 ignored `web-dm/packages/contracts-generated/dist/` 留下可消费 `.tgz`；统一 `check-fast.ps1` / `check.ps1` 的内部打包验证则只在受校验的同一路径使用并清理该产物。许可证绑定明确忽略这一个瞬态输出目录，仍精确验证 contracts package 的 tracked source；完整检查另运行连续打包/许可证专测，防止自身产物改变下一次检查输入。

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

脚本不会连接或清理目标服务器，也不会打印这些值。调用者提供的数据库必须可安全重置，不得指向共享、测试发布或生产数据库。集成计划依次覆盖 migration、Phase 1 正向业务闭环、13 项负向不变量、Phase 2B Carry/Roll-down、Phase 2C 国债期货交割、Phase 2D 套保、Phase 3A/3B 数据与快照、R6A 生产输入平面、R6B Artifact 生产拓扑、R8A Portfolio PostgreSQL/生产 gRPC，以及 R8B Portfolio Performance PostgreSQL/生产 native gRPC/gRPC-Web，最后执行 R7B 自建隔离 PostgreSQL/Ceph source 与 restore project 的真实销毁恢复证明。R7B runner 只删除其名称受限的临时 project/volume；细节见 [`operations/recovery.md`](operations/recovery.md)。

仓库内 `deploy/dev/docker-compose.yml` 是当前唯一的本地 Compose 拓扑，提供锁定基础镜像摘要的 PostgreSQL、单节点 Ceph RGW、migration、Server、Worker 和 React Platform Shell；它不是生产 Ceph 部署模板。日常开发只调用下面的包装脚本：

```powershell
.\scripts\dev-up.ps1 -ListOnly
.\scripts\dev-up.ps1
```

首次启动会生成 ignored 的 `deploy/dev/.env.local`；后续启动复用它。该文件包含一次性本地 PostgreSQL、S3、Platform、cursor 和 bootstrap 身份凭据，不进入 Git，也不应复用共享、测试发布或生产凭据。脚本先用正式 Dockerfile 构建完整拓扑，从实际 `ficant/worker:dev` 镜像取得 OCI image ID 和内嵌 native source digest，再以这些受信值启动服务；最后经 UI `/ficant-api` 调用真实 `GetCurrentSession` gRPC-Web，必须同时取得已认证 Session 和成功 trailer。可选端口仍可在启动前通过 `FICANT_POSTGRES_PORT`、`FICANT_S3_PORT`、`FICANT_SERVER_PORT`、`FICANT_WORKER_PORT` 与 `FICANT_UI_PORT` 覆盖。

### R8A/R8B 本地接线

相邻 `ficant-portfolio` WebApp 不进入本仓库构建，R8A/R8B 也未修改或接入该应用。FICANT 当前只提供金证FICC合同管理系统 D01/P01/P02/P03/P04 所需的真实只读 DTO/BFF 合同；下游完成独立接线后必须通过 gRPC-Web 调用本仓库生产 server，其余十九页仍由 WebApp 自己标记 demo/partial。FICANT BFF 永不返回 `demo`。

| 用途 | 地址 | 谁使用 |
|---|---|---|
| Docker 开发 gRPC-Web | `http://127.0.0.1:18080`（`FICANT_SERVER_PORT` 可覆盖） | WebApp、`GetCurrentSession` → `GetPage` |
| 本机直接绑定 native gRPC | `127.0.0.1:50051`（`FICANT_GRPC_BIND`） | 仅本地 native client / focused SIT；WebApp 不得猜测该端口 |
| 允许 origin | 精确包含 `http://127.0.0.1:5173`，以及 Platform Shell `http://127.0.0.1:18083` | CORS；禁止 `*` |
| 本地 TypeScript 包 | `web-dm/packages/contracts-generated/dist/ficant-contracts-generated-0.0.0.tgz` | 相邻 WebApp 安装锁定 artifact，删除对 `web-dm` 源码路径的 alias |

fixture Researcher 必须同时具备精确 scopes：`portfolio:read,positions:read,rates:analyze,facts:read,definitions:read,artifacts:read`。`dev-up.ps1` 会把该集合写入 `.env.local`；拓扑健康后运行：

```powershell
.\scripts\bootstrap-portfolio-p0.ps1
```

它幂等写入 1 Book、1 Group、2 Portfolio、各自 exact PositionSnapshot、中国国债/曲线/市场事实、Benchmark 与 Convention v1，不制造重复可见对象。包本身用 `.\scripts\package-contracts.ps1` 生成，摘要由 `.\scripts\test-contract-package.ps1` 复核。

需要 R8B 日度收益夹具时运行：

```powershell
.\scripts\bootstrap-portfolio-performance.ps1
```

该入口先幂等保证 R8A 基础对象存在，再追加 Calendar v2、1 个 exact PerformanceConvention、两个交易日 × 两个成员的 4 个 PortfolioValuationSnapshot，以及两个 BenchmarkLevelSnapshot；随后从正式 PostgreSQL repository 读回核对。它不修改 R8A 现有记录，也不提供公共写 RPC。

停止容器和网络但保留 `postgres-data`、`ceph-data` 命名卷：

```powershell
.\scripts\dev-down.ps1 -ListOnly
.\scripts\dev-down.ps1
```

`dev-up.ps1` 可能为了冷构建或缺失的锁定基础镜像访问网络。它不会替代 `check.ps1 -IncludeIntegration` 所需的 `FICANT_TEST_*` 变量；测试调用者仍须把本地服务地址和本轮精确 runtime image digest 映射到测试变量。包装脚本不提供删除数据卷的选项；如确需销毁本地数据，必须单独审查精确 Compose project 和卷目标。检查脚本本身仍不自动启动、停止、清理或下载该夹具。

R7B 另提供 clean-candidate 恢复与 private MANUAL 入口：

```powershell
.\scripts\check-recovery.ps1 -ListOnly
.\scripts\check-recovery.ps1

.\scripts\check-manual.ps1 `
    -AuthorityRoot C:\path\to\ficant-authority `
    -ExpectedAuthorityCommit <40-hex> `
    -ExpectedPublicCommit <40-hex>
```

恢复入口绑定 clean public commit/tree 与实际 Worker image config digest；MANUAL runner 另绑定 clean authority checkout、manifest 中三份文档 hash 和 marker 顺序，在临时 detached public checkout 原样执行命令。尖括号只用于本页说明，不能出现在获批 MANUAL literal block 中。

## 本地依赖能力

开发机只确认工具链存在、属于正确的工具族且不低于最低版本，不要求小版本与 CI 一致（[ADR-0020](architecture/adr/0020-dev-toolchain-minimum-versions.md)）。检查脚本不安装工具；依赖按锁文件解析（`--locked`），本机缺少的 crate、wheel、npm 包或 pnpm 会从包源取回（本地契约包测试的 consumer 安装改为 `--prefer-offline`：优先用本机缓存，缺了再联网）。精确版本只在 CI 与发布侧锁定，下表最后一列供对照。

| 能力 | 开发机要求 | CI / 发布锁定 |
|---|---|---|
| PowerShell | PowerShell 7，原生 Windows 路径和参数数组 | — |
| Rust | rustup；`rust-toolchain.toml` 会自动装上锁定版本。脚本接受 cargo/rustc ≥ 1.96 | 1.96.1 |
| C++ | CMake、Ninja、clang++ ≥ 18。依次使用 `FICANT_CXX`、Visual Studio Build Tools 自带 LLVM、`PATH` 上的 `clang++`。Windows 上要在 VS 开发者环境（`vcvars64.bat` / Developer PowerShell）中运行 `check.ps1`，CMake 需要其中 Windows SDK 的 `rc.exe` | LLVM 19.1.5 |
| Cross-Clang | 可选：WSL 发行版 `ficant-ubuntu-24.04` 中的 `/usr/bin/clang++-18`，可用 `FICANT_WSL_DISTRO` / `FICANT_LINUX_CXX` 覆盖。没有时该步打印 SKIPPED 警告并跳过，跳过不是数值一致的证据；设 `FICANT_REQUIRE_CROSS_CLANG=1` 则改为失败 | Ubuntu clang 18.1.8 |
| Python | uv ≥ 0.7；Python 版本与 wheel 由 `python/uv.lock` 决定 | uv 0.7.13 |
| Web | Node ≥ 22（`web-dm/.npmrc` 设 `engine-strict=false`，`engines` 里的 22.17.0 只作提示），Corepack 按 `web-dm/package.json` 的 `packageManager` 取用 pnpm。`web-dm` 的脚本会再调用 `pnpm`，所以 `PATH` 上的 `pnpm` 必须是 Corepack 垫片（`corepack enable`），不能是 `npm -g` 装的其他版本 | Node 22.17.0、pnpm 10.12.4 |
| Protobuf | Buf ≥ 1.56；Windows 可用 `FICANT_BUF` 指定可执行文件 | Buf 1.56.0 |
| 集成测试 | 仅在 `-IncludeIntegration` 下使用调用者提供的本地一次性 PostgreSQL/Ceph RGW 和固定 runtime image digest | — |

工具版本与锁定值不同而检查失败时，先用锁定版本复现，再判断是不是代码问题。`scripts/check-manual.ps1` 是 MANUAL 走查的执行器，仍保持离线安装，不受本节放宽影响。

完整检查对产品 Rust target 执行 `clippy -D warnings`。`ficant-contracts` 的生成文件与 `ficant-contract-tests` 的 descriptor inventory harness 不进入 Clippy；它们由锁定生成器的一致性检查和独立 contract test 覆盖，避免把生成器风格告警误报成手写产品代码缺陷。

## 交接给 CICD

开发以 Pull Request 交给 Human。PR 描述包含真实改动、命令、退出码、测试数量和剩余风险。Human 有两个选择：

1. 在同一候选上运行一次完整本地检查（本地 CI），并完成风险相关的人工复测；
2. 按 PR 描述和已有证据验收并合并。

两个选择都不启动 GitHub 完整 CI。普通 branch push、Pull Request 和 `main` 合并只维护远端源码历史；本地检查是普通迭代的正式证据，但不能冒充 Linux Runner、在线供应链、制品或目标环境证据。

准备建立版本候选时，先更新本机 Trivy 0.72.0 漏洞数据库，然后在干净且与 `origin/main` 精确一致的 `main` 运行：

```powershell
trivy image --download-db-only
.\scripts\check-release-candidate.ps1
```

该入口不创建 tag、不推送镜像、不连接测试服务器，也不安装工具。它使用正式 Dockerfile 和固定基础镜像 digest 构建 Server、Worker、UI 三个应用镜像，验证 `deploy/storage-runtime.lock.json` 后复用并扫描锁定的 Ceph OCI digest；三个应用镜像与 Ceph 运行时共四个运行镜像都以本地 Trivy 数据库执行 `HIGH,CRITICAL`、`ignore-unfixed` 扫描。随后启动 PostgreSQL 与锁定的 Ceph RGW，执行一次性 migration，再启动真实 Worker、Server 和 UI，验证 readiness、UI 与 forward-only migration 兼容。只有该入口通过且 Human 明确给出版本号后，才创建新的不可变版本 tag。

数个迭代后，Human 确定版本号并创建符合版本格式、指向当前 `main` 精确提交的不可变 `v*` tag。创建 tag 即把版本候选交给 CICD，并授权完整 GitHub CI、SHA 镜像构建、扫描、不可变版本标签提升、Linux 测试环境部署、健康/冒烟检查和失败回滚。版本失败后不得移动原 tag；修复走普通 Pull Request 合并到 `main`，再建立 forward-only 版本候选。普通开发工作不得修改 `.github/**`、`cicd.yml` 或 `deploy/**` 来绕过这个交接边界。

本地镜像验证必须使用仓库正式的 `deploy/dev/RustService.Dockerfile` 做冷构建，并保留其精确候选和构建结果。为了排查网络或环境问题而使用的临时 Dockerfile、复制宿主机构建产物后组装的 runtime 镜像，最多只能作为诊断证据，不能冒充正式镜像构建通过。版本制品的最终证据只能来自 Human 创建版本 tag 后触发的 GitHub version Action：它在 Linux Runner 上用正式 Dockerfile 构建并发布 Commit SHA 标识的不可变镜像。普通 branch、Pull Request、`main` 合并以及本地临时镜像均不得被描述为这项最终证据。
