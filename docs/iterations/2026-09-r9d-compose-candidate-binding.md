# R9D 迭代 brief — 发布 Compose 候选身份绑定

**面向 Human 的产品名：** 金证FICC合同管理系统 · **平台名：** FICANT · **内部迭代：** R9D · **execution base：** `eb09b2e12f2ed8d4237c235eb638d0da1db07b38` · **base tree：** `cf4db0f9386a4b3735f6c5d076c9dcc6f02fa2d0` · **状态：** 本地候选完成，待 PR 与 clean-main preflight

本 brief 是 R9D 面向 Human 的唯一范围、权限边界与最终本地证据载体。R9C 已通过 PR #68 线性合入同步且干净的 `main`；第三次发布候选预检在创建任何 tag 之前通过三个应用镜像的构建，以及三个应用镜像与锁定 Ceph 的扫描，随后暴露发布 Compose 校验器仍把零 SHA 夹具写死为唯一允许镜像身份，因此对真实候选正确失败关闭。

## 1. 目标

让发布 Compose 校验器把调用边界已注入的 `FICANT_DEPLOY_SHA` 作为唯一候选身份，严格验证其为 40 位小写 Git SHA，并要求 Server、Worker、UI 三个解析后镜像逐一精确绑定该值；保留远端零 SHA 静态夹具，同时让本地非零真实候选可被严格校验。

**Acceptance sentence：**

> `validate_release.py` 必须拒绝缺失、非法、大小写漂移或与任一应用镜像不一致的 `FICANT_DEPLOY_SHA`，只接受三个应用镜像均精确为 `ghcr.io/kayz/ficant-{server|worker|ui}:sha-{已验证候选}` 的解析模型；非零候选、错配与缺失/非法身份的行为回归、标准本地检查及新的 clean-main 完整发布候选预检全部通过后才允许创建 `v0.1.0-alpha.10` tag。

## 2. 验收

| 条目 | R9D 可执行判据 |
|---|---|
| 候选输入 | 校验器从进程环境读取 `FICANT_DEPLOY_SHA`，只接受精确 `[0-9a-f]{40}`；不猜测 HEAD、tag、镜像中的任意一项，也不接受缺失、空值或大写。 |
| 镜像绑定 | `ficant-server`、`ficant-worker`、`ficant-ui` 的解析后 `image` 必须分别完全等于固定 GHCR prefix、服务 suffix 与同一候选 SHA 的组合；前后缀匹配或“任意合法 SHA”均不足以通过。 |
| 双入口兼容 | 远端 authorize 静态拓扑使用的 40 个零继续作为合法结构夹具；本地 preflight 注入的真实 non-zero candidate SHA 必须通过同一校验器，不新增第二套判断逻辑。 |
| 失败关闭 | 行为测试覆盖 non-zero 正例、候选错配、单服务漂移、缺失/空值/大写/长度非法；不得放宽 registry、镜像名、SHA 格式或现有 Compose 安全约束。 |
| 发布门 | 修复线性合入后，本地 `main` clean 且精确等于 `origin/main`，当日 Trivy 数据库有效，`check-release-candidate.ps1` 全部 17 步 exit 0；否则不创建 tag。 |

## 3. 非目标

- 不改变业务、Protobuf、migration、数据库、数值、金融 Golden/Oracle/expected/容差或依赖。
- 不改变 Compose 模板、workflow、preflight 脚本、Dockerfile、基础镜像、Ceph 锁、漏洞阈值或扫描参数。
- 不通过恢复写死零 SHA、只校验正则前后缀、忽略调用方候选或跳过模型校验制造通过。
- 不新增 secret、测试环境变量、生产部署或 UAT。
- 本地修复期间不创建、移动或删除版本 tag，不推送镜像，不手工触发 release workflow，不连接测试服务器。

## 4. 公共契约变化

- 业务和外部接口契约无变化。
- 发布校验 CLI 的输入合同被收紧：除了 stdin 中的 resolved Compose JSON，还必须从环境接收一个合法的 `FICANT_DEPLOY_SHA`，且三个应用镜像必须精确绑定它。
- 远端 workflow 与本地 preflight 已分别注入零 SHA 结构夹具和真实候选 SHA，因此调用形状不变；`v0.1.0-alpha.10`、镜像命名和部署状态格式不变。

## 5. 需 Human 决策

| 决策 | 已确认选择 | 边界 |
|---|---|---|
| D1 版本号 | 继续使用 Human 已选择的 `v0.1.0-alpha.10`。 | 本地与远端均无该 tag；三次失败全部发生在 tag 前，未发布任何同名不可变候选。 |
| D2 修复后交付 | 沿用“门禁全绿后才创建 tag”的授权，先以独立 PR 合入 R9D，再从新 `main` 重新执行完整 preflight。 | 新 preflight 或 tag 后版本 CI 任一步失败都必须立即停止并保留真实失败事实。 |

## 6. 最终真实测试证据

**实施允许写路径（开始实施前冻结的闭集）：**

- `deploy/test/validate_release.py`
- `.github/scripts/tests/test_compose_security_gate.py`
- `docs/iterations/2026-09-r9d-compose-candidate-binding.md`（本文件；实施开始后只在本节追加真实证据并更新第 7 节）
- `docs/iterations/README.md`
- `docs/delivery/release-notes.md`
- `docs/quality/evidence.md`

**受保护事实：** 上述闭集之外的源码、依赖/lock、Compose、workflow、preflight、Dockerfile、镜像锁、金融证据与 ignored private authority 均不得修改。tag、镜像、GitHub workflow 运行和测试环境交付只在本地候选合入并通过 clean-main preflight 后进入 CICD。

本节以下只记录实际命令、exit code、可得 test count、候选身份和必要失败恢复；计划命令不得写成通过。

| 真实命令/检查 | Exit code | 结果 |
|---|---:|---|
| Trivy DB update + `scripts/check-release-candidate.ps1`（clean `main@eb09b2e12f2ed8d4237c235eb638d0da1db07b38`，tree `cf4db0f9386a4b3735f6c5d076c9dcc6f02fa2d0`） | 0 / 1 | Trivy 0.72.0 DB 为当日有效；license/storage gates 通过，Server、Worker、UI 正式镜像构建通过，三个应用镜像与锁定 Ceph 的 HIGH/CRITICAL 扫描均为 0，storage config/RepoDigest 验证通过。第 15 步 resolved Compose 校验因实际 Server 镜像为 `sha-eb09b2e...` 而校验器写死只允许 `sha-000...` 失败；未创建运行临时根、未启动 Compose、未创建 tag、未推送镜像。 |
| 本地/远端 `v0.1.0-alpha.10` 与 preflight 资源查询 | 0 | 两端均不存在目标 tag；不存在 `ficant-release-preflight-*` 容器，工作区仍 clean，历史 tag 未修改。 |
| `python .github/scripts/tests/test_compose_security_gate.py -v` | 0 | 36 total：34 passed、2 个显式 live gate skipped。新增行为门证明零 SHA 与 non-zero SHA 正例通过，并拒绝缺失、空值、大写、正确长度非十六进制、长短漂移、校验器候选错配、三个服务各自的 wrong SHA 及两类前后缀镜像名称欺骗。 |
| 以真实 `eb09b2e12f2ed8d4237c235eb638d0da1db07b38` 解析 `compose.test.yml` 并管道输入 `validate_release.py` | 0 | `release-compose: PASS`；同一个候选同时驱动三个镜像引用和 validator，没有使用零 SHA 替代真实本地身份。 |
| `bash .github/scripts/tests/run-repo-policy-tests.sh` 与 `git diff --check` | 0 / 0 | release-state 4/4 与完整 repo-policy fixtures PASS；无 whitespace error。 |
| `scripts/check-fast.ps1`（Node 22.17.0） | 0 | 23 步统一快速检查从头通过。 |
| 独立代码与发布合同审查 | 0 | 最终均为 blocker 0 / major 0 / minor 0；12+ 项独立 mutation 覆盖零/真实 SHA、registry/owner/name/SHA/latest、安全模型、缺失/大写身份，且旧 `startswith/endswith` 可接受的欺骗形状均被当前实现与持久测试拒绝。 |
| `scripts/check.ps1`（Node 22.17.0） | 0 | 40 步标准完整本地检查从头通过；C++ 9/9、Web 35/35，Rust/Python/独立 Decimal Oracle、契约与生成物检查均为 0 failed。 |
| R9D current-truth 文档、相对链接与闭集审查 | 0 | blocker 0 / major 0 / minor 0；4 个文档的 22 个相对链接 0 broken，允许 6 / 实际 6 / 越界 0。审查发现的 Ceph“构建/扫描”歧义已改为三个应用镜像构建、四个镜像扫描的精确事实。 |

## 7. 残余风险

- 最终候选提交/合并和合并后 clean-main preflight 尚未完成；当前不得创建版本 tag。
- workflow 的真实 Linux build、GHCR/SBOM/provenance 与测试环境部署只能由后续不可变 tag 运行证明，本地校验不能替代。
- Ceph CentOS Stream 9 OS family 无法由 Trivy 提供 OS 包覆盖；本次只确认其中可识别的 Node/Python 组件为 0 finding，该既有残余风险不因 Compose 身份修复而消失。
