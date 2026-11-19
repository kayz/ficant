# FICANT 协作边界

本仓库分两条链路：日常开发由 agent 自行组织，按下面的约定交付可验证的候选；版本发布只由 Human 创建的版本 tag 触发，由本仓库的发布工作流执行。两条链路不得互相代替。决策依据见 [ADR-0019](docs/architecture/adr/0019-lightweight-development-keep-release-boundary.md) 与 [ADR-0030](docs/architecture/adr/0030-core-reset-and-trading-desk-ibor.md)。

## 权威与边界

- **产品与架构定位**：`ficant` 是开源的中国本币市场固收及衍生品「实时估值定价 + 轻量级实时 IBOR（投资记账簿）」内核，面向机构交易内网交付「投资经理与交易台盘中副屏」（见 [ADR-0030](docs/architecture/adr/0030-core-reset-and-trading-desk-ibor.md)）。凡 T-1 盘后即可静态算清的工作、柜台报单（OMS/EMS）、清算结算与中后台会计总账均不在本仓库范围内。原外围仓库（`ficant-authority`、`ficant-fixin`、`ficant-portfolio`、`ficant-research`）已归档，本仓库为唯一活跃权威源。
- **分层法则**：依赖只能向下、定价原语不得内嵌具体市场规则数值、规则包必须被解析而不只是被引用、新增市场不改 L0/L1/L2 源码，见 [ADR-0013](docs/architecture/adr/0013-layering-law-shape-in-core-content-in-rulepack.md)。`scripts/check-layering.ps1` 会机械检查其中关键不变量。

## 开发约定

- 如何拆分任务、是否使用子 agent、模块内部怎么写，由你自行判断。产品或金融语义不清楚时先问；实现细节自己决定。
- 不得通过修改 `expected`、`Oracle`、`golden case`、断言或容差制造通过。认为它们有误时，单独提出并说明理由，由 Human 决定。
- 公共契约（`interface/proto/**`）与 RulePack 内容（`domain-packs/**`）的语义变化需要 Human 同意；纯实现重构不需要。
- 完成前运行 `.\scripts\check-fast.ps1`；涉及契约、RulePack、C++ 内核或主流程时运行 `.\scripts\check.ps1`。先用 `-ListOnly` 查看将执行的命令。没跑或跑不了就直接说明，不要写成已通过。
- 交付说明写清：改了什么、实际运行的命令与 exit code 和可得的 test count、仍有的风险。不在仓库里建状态台账、任务清单或进度副本。
- 脚本保持 PowerShell 7 原生语法、参数数组、失败即退出，依赖按锁文件解析（`--locked`）；不得加入 SSH、GitHub、发布、部署或安装工具的行为。开发机只检查工具族与最低版本，精确版本只在 CI 与发布侧锁定，见 [ADR-0020](docs/architecture/adr/0020-dev-toolchain-minimum-versions.md)。

统一入口：

```powershell
.\scripts\check-fast.ps1
.\scripts\check.ps1
```

详细命令与本地依赖见 [docs/development.md](docs/development.md)。

## CICD 发布管理

- `.github/**`、`cicd.yml`、`deploy/**` 定义版本候选的正式 CI/CD 与发布合同，由本仓库自己拥有（[ADR-0021](docs/architecture/adr/0021-ficant-owns-release-files.md)）；中央 `kayz/cicd` 只保留测试机初始化、Nginx 与 Secrets/Environment 说明。除非 Human 明确要求，不得修改这些路径。
- 普通 branch push、Pull Request 和 `main` 合并不得触发完整 GitHub CI、镜像构建或部署。只有 Human 明确确定版本号并创建指向当前 `main` 精确提交的 `v*` tag，才进入 CICD；创建 tag 即授权自动完成版本 CI、构建与扫描验证。
- 发布制品只能由 Linux GitHub Runner 构建并以 Commit SHA 标识；测试服务器只拉取不可变镜像并部署，不现场编译（见 [docs/delivery/test-environment.md](docs/delivery/test-environment.md)）。
- GitHub Secrets、Environment、GHCR、SSH、Nginx、目标环境健康检查、部署记录和回滚只在获授权的 CICD/运维工作中处理。
- 未获得 Human 的版本号与版本交付授权时，不得创建、移动或删除版本 tag。版本 CI 失败后不得移动原 tag；修复合并到 `main` 后，再由 Human 创建新的 forward-only 版本候选。
- 本地检查结果不能写成远端 CI 结论，远端 CI 结果也不能写成本地通过结论。
