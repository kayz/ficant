# R9C 迭代 brief — UI 运行时漏洞收口

**面向 Human 的产品名：** 金证FICC合同管理系统 · **平台名：** FICANT · **内部迭代：** R9C · **execution base：** `43e52c4e6831a68fd1fbfa0ede4dc59504bcbe83` · **base tree：** `c1da896dbb47a914438641f6a1d8725642a3f9f3` · **状态：** 本地候选完成，待 PR 与 clean-main preflight

本 brief 是 R9C 面向 Human 的唯一范围、权限边界与最终本地证据载体。R9B 已补齐发布镜像源码身份绑定；第二次 clean-main 发布预检在创建任何 tag 之前发现 UI 固定运行时中的 OpenSSL 高危漏洞并正确失败关闭。

## 1. 目标

把 UI 最终运行时从存在已修复高危漏洞的官方 `nginx 1.31.3-alpine-slim` 不可变摘要前移到已验证的官方 `nginx 1.31.5-alpine-slim` 不可变摘要，同时增加精确摘要回归约束；不改变 UI 构建、Nginx 配置、非 root 身份、业务或发布拓扑。

**Acceptance sentence：**

> UI 正式 Dockerfile 必须只使用官方 `nginx 1.31.5-alpine-slim@sha256:3b171d7224b669faa3cc2137fea0a65301791df1ec1f271ebd2a2b7461f7fade` 作为最终运行时；本地正式镜像的 HIGH/CRITICAL 已修复漏洞扫描必须为 0，UID 101、Nginx 配置与 HTTP smoke 合同保持成立，并在标准本地检查及新的 clean-main 完整发布候选预检全部通过后才允许创建 `v0.1.0-alpha.10` tag。

## 2. 验收

| 条目 | R9C 可执行判据 |
|---|---|
| 运行时身份 | `deploy/test/FicantUi.Dockerfile` 的最终 stage 精确固定为官方 multi-arch index `nginx 1.31.5-alpine-slim@sha256:3b171d7224b669faa3cc2137fea0a65301791df1ec1f271ebd2a2b7461f7fade`，不得退回已失败摘要或可变 tag。 |
| 漏洞门 | 使用当日 Trivy 0.72.0 数据库扫描正式 UI 镜像，`HIGH,CRITICAL` 且 `--ignore-unfixed` 的结果为 0；不新增 ignore、例外或降低阈值。 |
| 运行合同 | 最终镜像继续以 UID 101 运行，现有模板、静态资源、健康检查与 `/ficant/` HTTP smoke 成功；Node build stage、应用代码和 Nginx 配置不变。 |
| 回归门 | Compose/release policy test 精确锁定新摘要并拒绝旧摘要；PowerShell/parser（若适用）、repo-policy、`check-fast.ps1`、`check.ps1` 与 `git diff --check` 通过。 |
| 发布门 | 修复线性合入后，本地 `main` clean 且精确等于 `origin/main`，Trivy 数据库保持当日有效，`check-release-candidate.ps1` 全部步骤 exit 0；否则不创建 tag。 |

## 3. 非目标

- 不改变业务、Protobuf、migration、数据库、数值、金融 Golden/Oracle/expected/容差或一方包集合。
- 不改变 Node build stage、前端源码或依赖、Nginx 配置、Compose、workflow、Rust 镜像、Ceph 锁、漏洞阈值或扫描参数。
- 不新增漏洞忽略、allowlist、secret、测试环境变量、生产部署或 UAT。
- 本地修复期间不创建、移动或删除版本 tag，不推送镜像，不手工触发 release workflow，不连接测试服务器。
- 不把 R9B 后失败的发布预检描述为通过；新预检任一步失败仍立即停止。

## 4. 公共契约变化

- 业务和外部接口契约无变化。
- UI 容器的固定运行时从官方 Nginx `1.31.3-alpine-slim` 前移为 `1.31.5-alpine-slim`；UID 101、文件布局、健康检查、端口与反向代理合同不变。
- `v0.1.0-alpha.10`、应用镜像集合、OCI identity、Compose 与部署状态文件格式不变。

## 5. 需 Human 决策

| 决策 | 已确认选择 | 边界 |
|---|---|---|
| D1 版本号 | 继续使用 Human 已选择的 `v0.1.0-alpha.10`。 | 本地与远端均无该 tag；两次失败都发生在 tag 前，因此不需要递增版本或移动历史 tag。 |
| D2 修复后交付 | 沿用“门禁全绿后才创建 tag”的授权，先以独立 PR 合入 R9C，再从新 `main` 重新执行完整 preflight。 | 若新预检或后续不可变版本 CI 失败，立即停止并保留真实失败事实。 |

## 6. 最终真实测试证据

**实施允许写路径（开始实施前冻结的闭集）：**

- `deploy/test/FicantUi.Dockerfile`
- `.github/scripts/tests/test_compose_security_gate.py`
- `docs/iterations/2026-09-r9c-ui-runtime-cve.md`（本文件；实施开始后只在本节追加真实证据并更新第 7 节）
- `docs/iterations/README.md`
- `docs/delivery/release-notes.md`
- `docs/quality/evidence.md`

**受保护事实：** 上述闭集之外的源码、依赖/lock、Compose、workflow、发布脚本、Rust/Ceph 镜像、金融证据与 ignored private authority 均不得修改。tag、镜像、GitHub workflow 运行和测试环境交付只在本地候选合入并通过 clean-main preflight 后进入 CICD。

本节以下只记录实际命令、exit code、可得 test count、候选身份和必要失败恢复；计划命令不得写成通过。

| 真实命令/检查 | Exit code | 结果 |
|---|---:|---|
| `pwsh -NoLogo -NoProfile -NonInteractive -File scripts/check-release-candidate.ps1`（R9B 合并后的 clean `main@43e52c4e6831a68fd1fbfa0ede4dc59504bcbe83`） | 1 | 前五步通过；Server、Worker 正式镜像构建及 HIGH/CRITICAL 扫描均通过且为 0；第 9 步已 pull 锁定 Ceph。第 12 步 UI 扫描发现 `libcrypto3`、`libssl3` 的 `CVE-2026-14456`（HIGH），安装版 `3.5.7-r0`、修复版 `3.5.8-r0`；未进入 Ceph scan/config verification 与 Compose 阶段，未创建 tag、未推送镜像。 |
| 本地/远端 `v0.1.0-alpha.10` 查询 | 0 | 两端均不存在目标 tag，历史 tag 未修改。 |
| 官方 `nginx@sha256:3b171d7224b669faa3cc2137fea0a65301791df1ec1f271ebd2a2b7461f7fade` pull + Trivy 预验证 | 0 | 官方 multi-arch index 对应 `nginx 1.31.5-alpine-slim`（2026-09-02），本机 linux/amd64 config digest 为 `sha256:ea186b7c7ac205bfc4e095b9db7bda1ebab289b138d16f3c4a85a9a7a2b63e9e`；相同漏洞门结果为 0。 |
| `python .github/scripts/tests/test_compose_security_gate.py -v` | 0 | 35 total：33 passed、2 个显式 live gate skipped。断言精确固定全部 `FROM` 行为 Node builder 与 Nginx final；独立 mutation review 证明追加 `nginx:latest` 或 `alpine:latest` 均被拒绝，最终结论 blocker 0 / major 0 / minor 0。 |
| 正式 `FicantUi.Dockerfile` build + Trivy + 包身份检查 | 0 | `ficant-r9c/ficant-ui:preflight` 构建为 `sha256:6e1f972fb84c7a86e62b8643f5964895315832f67443309d4f94e04e0a8f4de0`；Alpine 3.24.1、Nginx `1.31.5-r1`、`libcrypto3/libssl3 3.5.8-r0`，HIGH/CRITICAL finding 为 0，config user 保持 `101:101`。 |
| UID 101 + Nginx/HTTP smoke | 1 → 0 | 首次孤立启动因未提供 Compose DNS 名 `ficant-server` 正确拒绝配置；加入仅用于解析的 loopback host fixture 后，容器保持运行、实际 UID 101，`/health` 返回 `200 ok`，`/ficant/` 返回 200 与非空页面。独立只读审查另在 read-only root 下验证模板替换与 `nginx -t`，并确认新旧运行时入口、模板依赖与文件权限兼容。 |
| `bash .github/scripts/tests/run-repo-policy-tests.sh` 与 `git diff --check` | 0 / 0 | release-state 4/4 与完整 repo-policy fixtures PASS；无 whitespace error。 |
| `scripts/check-fast.ps1` | 1 → 0 | 首次因调用机 PATH 优先 Node 24.18.0 而在精确版本门失败，未放宽门禁；显式恢复仓库既有 Node 22.17.0 后从头重跑，23 步统一快速检查全部通过。 |
| `scripts/check.ps1`（Node 22.17.0） | 0 | 40 步统一完整本地检查通过：分层夹具 51 assertions、覆盖 inventory 69/7/62、完整 Rust build/test/Clippy、C++ 9/9、Cross-Clang 71 rows、独立 Decimal Oracles、Python 合约、contract package reentrancy、live SDK parity，以及 Web 5 files / 35 tests 全部通过。未运行 `-IncludeIntegration`，因为本轮不改应用、数据库、Compose 或业务行为；发布拓扑由合并后的正式 preflight 负责。 |
| 独立代码/运行时/文档审查 | 0 | 代码 mutation review 为 blocker 0 / major 0 / minor 0；运行时合同审查为 blocker 0 / major 0 / minor 0；文档复核的唯一 minor（Ceph pull 与后续 scan 阶段措辞）已修正，4 文件 20 个相对链接无 broken target，6 个实际变更路径与冻结闭集完全一致。 |

## 7. 残余风险

- 最终候选提交/合并和合并后 clean-main preflight 尚未完成；当前不得创建版本 tag。
- workflow 的真实 Linux build、GHCR/SBOM/provenance 与测试环境部署只能由后续不可变 tag 运行证明，本地扫描不能替代。
- Ceph CentOS Stream 9 OS family、rootful 测试 Docker 和 Alpha 产品范围等既有残余风险不因本轮 UI 运行时前移而消失。
