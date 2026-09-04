# R9E 迭代 brief — 发布运行时身份闭环

**面向 Human 的产品名：** 金证FICC合同管理系统 · **平台名：** FICANT · **内部迭代：** R9E · **execution base：** `433d03dd998a1e5829fc7bbc2ec6438e66cbfe00` · **base tree：** `1806850afbc1b3ca1690c30cc94a7cc3dd8aa17f` · **状态：** 本地候选完成，待提交/合并

本 brief 是 R9E 面向 Human 的唯一范围、权限边界与最终本地证据载体。R9D 已通过 PR #69 线性合入同步且干净的 `main`；第四次发布候选预检在创建任何 tag 之前通过构建、四镜像扫描、resolved Compose 校验和空库 migration，随后因 `ficant-server` 在监听前缺少运行时身份配置而正确失败关闭。

## 1. 目标

让发布 Compose、本地 preflight、GitHub 交付入口、测试机部署状态和回滚使用同一份已授权 Code/Runtime 身份：Server 与 Worker 必须接收候选 commit/tree，Server 必须接收实际镜像 config digest 与固定测试环境摘要；测试身份、输入绑定和 Worker 维护周期必须完整，静态校验不得再接受必然无法启动的模型。

**Acceptance sentence：**

> 零 SHA 静态拓扑与 non-zero clean-main 候选必须通过同一发布校验器；Server/Worker 的运行时 Code 必须精确绑定调用方 commit/tree，Server/Worker 实际镜像身份必须由已拉取或已构建镜像派生，测试身份、输入绑定与 Worker orphan 周期必须完整；部署状态和 forward-only 回滚须保留 tree 与两类实际 Runtime 身份；标准本地检查及新的 clean-main 17 步发布预检全部通过后才允许创建 `v0.1.0-alpha.10` tag。

## 2. 验收

| 条目 | R9E 可执行判据 |
|---|---|
| Compose 启动合同 | `ficant-server` 补齐已存在二进制要求的 11 个 Code/Runtime/bootstrap/input 字段；`ficant-worker` 补齐同一 Code 与两个 orphan 周期字段。不得把身份烘焙进 final image 环境或使用空值/可变值绕过。 |
| 候选绑定 | 两个 Rust 服务的运行时 commit 均等于 `FICANT_DEPLOY_SHA`，tree 均等于调用方验证过的 `FICANT_CODE_TREE_SHA`；Server runtime 等于实际 Server image config digest，Worker runtime/source 保持由实际 Worker image 派生。 |
| 环境与测试身份 | Server environment digest 精确对应 `ficant.server.environment.v1`、`amd64`、`linux`、`test`；测试 bearer identity 使用固定 tenant/actor/owner/RESEARCHER，输入连接标识和 Worker orphan 周期为非空、受控值。 |
| 双入口与校验 | GitHub authorize 的零 SHA/零 tree 结构夹具与本地真实 commit/tree 均通过；缺失、非法、错配、单服务漂移或 Runtime 漂移均失败。preflight 的 CORS origin 与随机 UI 端口一致。 |
| 状态与回滚 | `deploy.sh` 接收授权 commit/tree，拉取后派生 Server/Worker runtime；`current.env` / `previous.env` 原子保存 tree、Server runtime、Worker runtime/source。旧状态仅在回滚旧二进制时使用显式零值兼容占位，新候选不得降级。 |
| 发布门 | 修复线性合入后，本地 `main` clean 且精确等于 `origin/main`，当日 Trivy 数据库有效，`check-release-candidate.ps1` 全部 17 步 exit 0；否则不创建 tag。 |

## 3. 非目标

- 不改变 Rust 业务、Protobuf、migration、数据库、数值、金融 Golden/Oracle/expected/容差或依赖。
- 不改变镜像基础层、Ceph 锁、漏洞阈值、扫描参数、端口暴露、安全权限或长期环境 secret 集合。
- 不新增生产发布、UAT、OIDC、gVisor、HA/PITR 或业务 WebApp 功能。
- 不通过删除二进制配置校验、给 final image 写入可伪造身份、跳过 health/readiness 或放宽回滚状态制造通过。
- 本地修复期间不创建、移动或删除版本 tag，不推送镜像，不手工触发 release workflow，不连接测试服务器。

## 4. 公共契约变化

- 业务与外部 API 无变化。
- 测试交付脚本由 `deploy.sh <commit>` 收紧为 `deploy.sh <commit> <tree>`；GitHub authorize 已产生二者，远端调用显式传递。
- `state/current.env` 与 `state/previous.env` 在既有 commit/storage/Worker 字段之外新增 Code tree 与 Server runtime digest；仍使用同目录 `0600` 临时文件和原子 rename。
- 发布 Compose 的调用环境新增 `FICANT_CODE_TREE_SHA` 与 `FICANT_SERVER_RUNTIME_IMAGE_DIGEST`；`FICANT_CODE_COMMIT_SHA` 由既有 `FICANT_DEPLOY_SHA` 唯一派生。

## 5. 需 Human 决策

| 决策 | 已确认选择 | 边界 |
|---|---|---|
| D1 版本号 | 继续使用 Human 已选择的 `v0.1.0-alpha.10`。 | 本地与远端均无该 tag；四次失败全部发生在 tag 前，未发布任何同名不可变候选。 |
| D2 修复后交付 | 沿用“门禁全绿后才创建 tag”的授权，先以独立 PR 合入 R9E，再从新 `main` 重新执行完整 preflight。 | 新 preflight 或 tag 后版本 CI 任一步失败都必须立即停止并保留真实失败事实。 |

## 6. 最终真实测试证据

**实施允许写路径（开始实施前冻结的闭集）：**

- `scripts/check-release-candidate.ps1`
- `deploy/test/compose.test.yml`
- `deploy/test/validate_release.py`
- `deploy/test/bin/deploy.sh`
- `deploy/test/bin/rollback.sh`
- `.github/workflows/release-test.yml`
- `.github/scripts/tests/test_compose_security_gate.py`
- `.github/scripts/tests/test_release_state_contract.py`
- `docs/delivery/test-environment.md`
- `docs/iterations/2026-09-r9e-release-runtime-identity.md`（本文件；实施开始后只在本节追加真实证据并更新第 7 节）
- `docs/iterations/README.md`
- `docs/delivery/release-notes.md`
- `docs/quality/evidence.md`

**受保护事实：** 上述闭集之外的源码、依赖/lock、migration、Dockerfile、镜像锁、金融证据与 ignored private authority 均不得修改。tag、镜像、GitHub workflow 运行和测试环境交付只在本地候选合入并通过 clean-main preflight 后进入 CICD。

本节以下只记录实际命令、exit code、可得 test count、候选身份和必要失败恢复；计划命令不得写成通过。

| 真实命令/检查 | Exit code | 结果 |
|---|---:|---|
| Trivy DB refresh + `scripts/check-release-candidate.ps1`（clean `main@433d03dd998a1e5829fc7bbc2ec6438e66cbfe00`，tree `1806850afbc1b3ca1690c30cc94a7cc3dd8aa17f`） | 0 / 1 | Trivy 0.72.0 DB 当日有效；license/storage、三个正式应用镜像构建、三个应用与锁定 Ceph 的扫描、storage identity 和 resolved Compose 均通过，空库 migration 成功。第 16 步因 Server unhealthy 失败；脚本自动移除本次容器、网络、临时卷和运行根，未创建 tag、未推送镜像。 |
| 隔离 Compose/容器复现、运行时 env 差集与两路独立审查 | 0（Server 预期 exit 1） | Server 日志稳定为 `invalid server configuration: FICANT_CODE_COMMIT_SHA is required`，在监听前退出；解析模型精确缺 11 个 Server 必需字段，Worker 另缺 Code 两字段和 orphan 两字段。诊断容器、网络和卷已清理；只含配置/migration 副本的本机诊断临时目录待最终产物清理处理。审查确认 Dockerfile builder ENV 不会进入 final stage，且当时的静态 validator 存在同一盲区。 |
| 本地/远端 `v0.1.0-alpha.10` 查询 | 0 | 两端均不存在目标 tag；最近既有 tag 仍为 `v0.1.0-alpha.9`。 |
| R9E 定向真实 Compose 探针（既有 `main@433d03dd` 三个预检镜像、真实 commit/tree/runtime/source，空库 migration + 全服务 `--wait` + HTTP smoke） | 0 | migration 完成；Server、Worker、UI 均 healthy；`WORKER=200:ok`、`UI=200`。探针专属容器、网络和卷已通过精确 `docker compose down --volumes --remove-orphans` 清理。 |
| `python -B .github/scripts/tests/test_compose_security_gate.py -v` | 0 | 37 tests：35 passed、2 个显式 live gate skipped、0 failed。覆盖缺失/非法/错配 Code 身份、Server/Worker caller runtime/source 绑定、固定 environment/bootstrap/input/orphan 值、三种 paired drift 与 preflight 清理结构。 |
| `python -B .github/scripts/tests/test_release_state_contract.py -v` | 0 | 9/9 passed。可执行 mock 证明 legacy source 失败会清除继承摘要，新候选不能进入 legacy；成功状态写入后的 record 失败会恢复旧容器并以重新派生的 runtime/source 原子恢复 `current.env`。 |
| `bash .github/scripts/tests/run-repo-policy-tests.sh`、Python `py_compile`、两个部署脚本 `bash -n`、PowerShell parser、resolved Compose + 实际镜像摘要 validator | 0 | repo-policy、语法和 17 步计划均通过；从实际 Server/Worker 镜像派生的 config/source 摘要通过加强后的 `release-compose` 精确绑定校验。 |
| 修复后独立只读复审 | 0 | Blocker 0 / Major 0 / Minor 0。首轮发现的 legacy 摘要污染、record 后状态分叉、Worker paired-drift 绕过和 preflight 临时根清理窗口均已修复并有回归证据。 |
| `scripts/check-fast.ps1`（固定 Node.js `v22.17.0`） | 0 | 23/23 步通过。首次预跑因调用机 PATH 命中非仓库要求的 Node.js `v24.18.0` 而在版本门失败；切回固定 `v22.17.0` 后通过，未修改版本期望、断言或容差。 |
| `scripts/check.ps1`（固定 Node.js `v22.17.0`） | 0 | 40/40 步通过；C++ 9/9、Web 35/35，`FICANT complete local checks passed.`。 |

## 7. 残余风险

- R9E 目标测试、独立复审、最终 `check-fast.ps1` 与完整 `check.ps1` 已完成；本地候选可提交/合并，但提交/合并和合并后 clean-main preflight 尚未完成，当前不得创建版本 tag。
- 定向真实 Compose 已覆盖空库 migration、数据库/S3 依赖和三个应用健康，但远端部署/自动回滚仍由确定性 mock 覆盖；必须以完整 clean-main preflight 和后续不可变版本流水线的实际结果为准。
- 本地 smoke 会把 CORS 配置对齐随机 UI origin，但仍只验证端口、Worker readiness 和 UI 根标记，不执行浏览器内完整 gRPC-Web 业务流；这是后续业务验收边界，不把它描述为本次发布门已经覆盖。
- workflow 的真实 Linux build、GHCR/SBOM/provenance 与测试环境部署只能由后续不可变 tag 运行证明，本地校验不能替代。
- Ceph CentOS Stream 9 OS family 无法由 Trivy 提供 OS 包覆盖；本次只确认其中可识别的 Node/Python 组件为 0 finding，该既有残余风险不因运行时身份修复而消失。
