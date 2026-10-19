# FICANT 协作边界

本仓库分两条链路：日常开发由 agent 自行组织，按下面的约定交付可验证的候选；版本发布只由 Human 创建的版本 tag 触发，交给中央 `cicd` 平台。两条链路不得互相代替。决策依据见 [ADR-0019](docs/architecture/adr/0019-lightweight-development-keep-release-boundary.md)。

## 权威与边界

- 什么必须为真、如何判定为真，以私有 `kayz/ficant-authority` 仓库中的 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md` 为准，由该仓库的 `authority-manifest.json` 精确绑定公共提交。本仓库的 README、ADR、brief 和代码与它们冲突时，以它们为准。拿不到 authority 时直接说明，不要猜测其内容。
- 分层法则（依赖只能向下、定价原语不得内嵌具体市场规则数值、规则包必须被解析而不只是被引用、新增市场不改 L0/L1/L2 源码）见 [ADR-0013](docs/architecture/adr/0013-layering-law-shape-in-core-content-in-rulepack.md) 与 [分层重构说明](docs/architecture/layering-refactor.md)。`scripts/check-layering.ps1` 会机械检查其中一部分。
- ACCEPTANCE 条目只由 Human 在 authority 仓库中逐条批准点亮。agent 的测试通过只是证据，不能写成"已点亮"或"已验收"。

## 开发约定

- 如何拆分任务、是否使用子 agent、模块内部怎么写，由你自行判断。产品或金融语义不清楚时先问；实现细节自己决定。
- 不得通过修改 expected、Oracle、golden case、断言或容差制造通过。认为它们有误时，单独提出并说明理由，由 Human 决定。
- 公共契约（`interface/proto/**`）、数据库 migration、RulePack 内容（`domain-packs/**`）的语义变化需要 Human 同意；纯实现重构不需要。
- 完成前运行 `.\scripts\check-fast.ps1`；涉及契约、持久化、RulePack 或主流程时运行 `.\scripts\check.ps1`，需要 PostgreSQL/Ceph 的改动再加 `-IncludeIntegration`。先用 `-ListOnly` 查看将执行的命令。没跑或跑不了就直接说明，不要写成已通过。
- 交付形式是 Pull Request。PR 描述写清：改了什么、对应哪些 AC 或 SPEC 条款、实际运行的命令与 exit code 和可得的 test count、仍有的风险。不再为每个迭代单独写 brief，也不在仓库里建状态台账、任务清单或进度副本。
- 脚本保持 PowerShell 7 原生语法、参数数组、失败即退出和离线依赖行为；不得加入 SSH、GitHub、发布、部署或联网安装行为。

统一入口：

```powershell
.\scripts\check-fast.ps1
.\scripts\check.ps1
.\scripts\check.ps1 -IncludeIntegration
```

详细命令与本地依赖见 `docs/development.md`。

## CICD 发布管理

- `.github/**`、`cicd.yml`、`deploy/**` 和中央 `kayz/cicd` 平台定义版本候选的正式 CI/CD 与发布合同。除非 Human 明确要求，不得修改这些路径。
- 普通 branch push、Pull Request 和 `main` 合并不得触发完整 GitHub CI、镜像构建或部署。只有 Human 明确确定版本号并创建指向当前 `main` 精确提交的 `v*` tag，才进入 CICD；创建 tag 即授权自动完成版本 CI、构建、扫描和测试环境交付。
- 发布制品只能由 Linux GitHub Runner 构建并以 Commit SHA 标识；测试服务器只拉取不可变镜像并部署，不现场编译。
- 创建版本 tag 前，在已同步且干净的 `main` 上更新本机 Trivy 数据库并运行 `.\scripts\check-release-candidate.ps1`；该入口必须用正式 Dockerfile 构建并扫描与最终拓扑对应的 Server、Worker、UI 三个本地预检应用镜像，复核并扫描锁定的 Ceph RGW 运行时，启动 PostgreSQL、Ceph RGW，串行执行一次性 migration，再启动真实 Worker、Server 与 UI。它不创建 tag、不推送镜像、不连接目标服务器。
- GitHub Secrets、Environment、GHCR、SSH、Nginx、目标环境健康检查、部署记录和回滚只在获授权的 CICD/运维工作中处理。
- 未获得 Human 的版本号与版本交付授权时，不得创建、移动或删除版本 tag。版本 CI 失败后不得移动原 tag；修复走普通 Pull Request 合并到 `main`，再由 Human 创建新的 forward-only 版本候选。
- 本地检查结果不能写成远端 CI 结论，远端 CI 结果也不能写成本地通过结论。
