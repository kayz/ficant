# FICANT 协作边界

本仓库使用 OPAID 管理从任务冻结到本地自测候选及 Human brief 的开发工作，使用中央 `cicd` 平台管理 Human 明确建立的版本候选所触发的持续集成、镜像构建、发布、测试环境部署和回滚。两条链路不得互相代替。

## OPAID 本地开发与测试

- 在开始修改前冻结一个代码结果、验收句、非目标、精确 base、本地自测命令、写路径和公共契约。
- Root Orchestrator 对范围、语义、集成候选和最终本地验证负责；仅把边界互斥且确有并行收益的任务交给直接 Worker，Worker 不得再创建子 Agent。
- Worker 必须返回实际 changed files、命令、exit code、可得的 test count、blocker 和 residual risk。文字声明不能替代命令证据。
- 不得为 OPAID 创建状态 ledger、agent registry、mailbox、治理 checklist 或平行状态机。编排工具是本轮运行状态的唯一来源。
- 不得通过修改 expected、Oracle、断言或容差制造通过；此类语义变更必须返回 Root/Human 决策。
- OPAID 在精确集成候选通过规定的本地自测并完成唯一 Human brief 时结束。CI/CD、SIT、UAT、发布、服务器管理和回滚不是 OPAID 退出门。

### 迭代 brief 与快速子循环

- 每个迭代恰好维护一份面向 Human 的 brief，统一放在 `docs/iterations/`。brief 只记录目标、验收、非目标、公共契约变化、需 Human 决策、最终真实测试证据和残余风险。
- Agent 交流、Worker 命令证据和中间实现材料由编排工具承载，不要求 Human 阅读，也不得据此生成仓库内状态文档、子任务 brief 或平行治理记录。
- 一个迭代可以包含多个快速子循环；每个子循环只交付一个结果，运行针对性测试并形成 forward-only checkpoint。子循环不创建独立治理文档，不改变 Human 已冻结的迭代验收。
- 只有具体失败证据证明 OPAID 无法表达候选依赖、兼容性或 forward-only 恢复关系时，才允许提出治理修改；偏好、预防性抽象或方法论整理本身不是修改理由。
- OPAID 结束后把 brief 交给 Human。Human 可以要求在同一候选上运行一次完整本地检查（本地 CI）并人工复测，也可以按文档和现有证据验收、合并并进入下一迭代；两个选择都不触发 GitHub 完整 CI。

统一入口：

```powershell
.\scripts\check-fast.ps1
.\scripts\check.ps1
.\scripts\check.ps1 -IncludeIntegration
```

先用 `-ListOnly` 查看将执行的命令。脚本必须保持 PowerShell 7 原生语法、参数数组、失败即退出和离线依赖行为；不得加入 SSH、GitHub、发布、部署或联网安装行为。

## CICD 发布管理

- `.github/**`、`cicd.yml`、`deploy/**` 和中央 `kayz/cicd` 平台定义版本候选的正式 CI/CD 与发布合同。
- 普通 branch push、Pull Request 和 `main` 合并不得触发完整 GitHub CI、镜像构建或部署。数个迭代后，只有 Human 明确确定版本号并创建指向当前 `main` 精确提交的 `v*` tag，才进入 CICD；创建 tag 即授权自动完成版本 CI、构建、扫描和测试环境交付。
- 发布制品只能由 Linux GitHub Runner 构建并以 Commit SHA 标识；测试服务器只拉取不可变镜像并部署，不现场编译。
- 创建版本 tag 前，在已同步且干净的 `main` 上更新本机 Trivy 数据库并运行 `.\scripts\check-release-candidate.ps1`；该入口必须用正式 Dockerfile 构建和扫描全部最终镜像，并启动 PostgreSQL、Ceph RGW、真实 Worker、Server、Web 与 UI 的本地发布拓扑。它不创建 tag、不推送镜像、不连接目标服务器。
- GitHub Secrets、Environment、GHCR、SSH、Nginx、目标环境健康检查、部署记录和回滚只在获授权的 CICD/运维工作中处理。
- 未获得 Human 的版本号与版本交付授权时，不得创建、移动或删除版本 tag。版本 CI 失败后不得移动原 tag；修复必须进入新的 OPAID 迭代，再创建 forward-only 版本候选。
- OPAID 交接的是精确 Commit SHA 的本地自测候选、brief 及真实测试证据，不直接触发发布，也不把远端 CI 结果写成本地通过结论。版本交付任务使用 `$cicd` skill；普通开发任务使用 `$opaid` skill。

详细开发合同见 `docs/development.md`，治理决策见 `docs/architecture/adr/0009-opaid-local-development-and-cicd-release-boundary.md`。
