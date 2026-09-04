# R9B 迭代 brief — 发布镜像源码身份绑定

**面向 Human 的产品名：** 金证FICC合同管理系统 · **平台名：** FICANT · **内部迭代：** R9B · **execution base：** `542d91582da926be3bc3ef2adffb3bdb9d00f39d` · **base tree：** `9814163f34f8315afcb3a54d211624e3786a759b` · **状态：** 本地候选完成，待 PR 与 clean-main preflight

本 brief 是 R9B 面向 Human 的唯一范围、权限边界与最终本地证据载体。R9A 已通过 PR #66 线性合入同步且干净的 `main`；首次发布候选预检在创建任何 tag 之前暴露发布镜像缺少源码身份 build arguments，因此正确失败关闭。

## 1. 目标

让本地发布候选预检与远端不可变版本构建都把已授权候选的精确 Git commit/tree 作为 Docker build arguments 传入 Rust Server/Worker，并增加防回归断言；在新的干净主线候选上重跑完整发布预检后，继续使用尚未占用的 `v0.1.0-alpha.10` 授权。

**Acceptance sentence：**

> 在不改变业务、Proto、数值、Oracle、expected、容差、依赖、基础镜像或部署权限的前提下，本地 preflight 和 release workflow 必须从各自已验证的候选身份派生同一组 40 位小写 commit/tree SHA，显式传给两种 Rust 镜像构建；缺失、非法或漂移身份继续失败关闭，相关策略测试、标准本地检查和 clean-main 完整发布候选预检必须通过后才允许创建 `v0.1.0-alpha.10` tag。

## 2. 验收

| 条目 | R9B 可执行判据 |
|---|---|
| 本地身份 | `check-release-candidate.ps1` 先冻结 canonical commit，再从该 `$candidateSha^{tree}` 派生 tree；在 binding 前后、每个 build 前后、scan 后及最终成功前重复要求 clean `main == origin/main` 且 commit/tree 未漂移，并为 Server/Worker Docker build 显式传入两个身份。 |
| 远端身份 | `release-test.yml` 的 authorize job 输出已验证 tag commit 及其 tree；两个 Rust matrix build 都只消费这些 authorize outputs，不从可变分支或调用者输入猜测。 |
| 失败关闭 | Rust `build.rs` 的现有 40 位小写 SHA 校验保持不变；不传、传空、非法或非候选值不得被占位值、标签或 Docker metadata 代替。 |
| 回归门 | Compose/release policy test 精确覆盖本地两次 build 和远端 matrix build 的 commit/tree 传递；PowerShell parser、workflow/repo-policy、`check-fast.ps1`、`check.ps1` 与 `git diff --check` 通过。 |
| 发布门 | 修复线性合入后，本地 `main` clean 且精确等于 `origin/main`，Trivy 0.72.0 DB 保持当日有效，`check-release-candidate.ps1` 全部步骤 exit 0；否则不创建 tag。 |

## 3. 非目标

- 不改变业务、Protobuf、migration、数据库、数值、金融 Golden/Oracle/expected/容差或一方包集合。
- 不改变 Rust `build.rs`、Dockerfile、基础镜像、Cargo cache、运行时镜像集合、漏洞阈值或 Ceph 锁。
- 不新增 workflow 触发条件、权限、secret、测试环境变量、生产部署或 UAT。
- 本地修复期间不创建、移动或删除版本 tag，不推送镜像，不手工触发 release workflow，不连接测试服务器。
- 不通过移除源码身份校验、写死零值或绕过 clean-main/Trivy 门禁制造通过。

## 4. 公共契约变化

- 业务和外部接口契约无变化。
- 构建来源证明合同被补齐：Rust Server/Worker 编译时身份必须与 authorize/preflight 已验证的候选 commit/tree 一致。
- `v0.1.0-alpha.10`、镜像命名、OCI labels、Compose 与部署状态文件格式不变。

## 5. 需 Human 决策

| 决策 | 已确认选择 | 边界 |
|---|---|---|
| D1 版本号 | 继续使用 Human 已选择的 `v0.1.0-alpha.10`。 | 本地与远端均无该 tag；未发生不可变版本发布，因此不需要递增版本或移动历史 tag。 |
| D2 修复后交付 | 沿用“门禁全绿后才创建 tag”的授权，先以独立 PR 合入 R9B，再从新 `main` 重新执行完整 preflight。 | 首次失败证据不得删除或描述为通过；新预检失败仍立即停止。 |

## 6. 最终真实测试证据

**实施允许写路径（开始实施前冻结的闭集）：**

- `scripts/check-release-candidate.ps1`
- `.github/workflows/release-test.yml`
- `.github/scripts/tests/test_compose_security_gate.py`
- `docs/iterations/2026-09-r9b-release-identity-binding.md`（本文件；实施开始后只在本节追加真实证据并更新第 7 节）
- `docs/iterations/README.md`
- `docs/delivery/release-notes.md`
- `docs/quality/evidence.md`

**受保护事实：** 上述闭集之外的源码、Dockerfile、依赖/lock、供应链策略、Compose、部署脚本、金融证据与 ignored private authority 均不得修改。tag、镜像、GitHub workflow 运行和测试环境交付只在本地候选重新合入并通过 clean-main preflight 后进入 CICD。

本节以下只记录实际命令、exit code、可得 test count、候选身份和必要失败恢复；计划命令不得写成通过。

| 真实命令/检查 | Exit code | 结果 |
|---|---:|---|
| `trivy image --download-db-only` | 0 | Trivy 0.72.0 DB 更新为 `UpdatedAt 2026-09-04 01:11:59Z`、`DownloadedAt 2026-09-04 06:57:15Z`。 |
| `pwsh -NoLogo -NoProfile -NonInteractive -File scripts/check-release-candidate.ps1`（execution base，Node 22.17.0） | 1 | 前五步 license/storage binding 全部通过；第 6 步 Server release image build 中，`build.rs` 收到空的 `FICANT_CODE_COMMIT_SHA`，以“compiled Git commit must be one 40-character lowercase SHA”失败。未创建 tag、未推送镜像、未启动发布 Compose。 |
| 本地/远端 `v0.1.0-alpha.10` 查询 | 0 | 两端均不存在目标 tag，历史 tag 未修改。 |
| PowerShell Parser + `python .github/scripts/tests/test_compose_security_gate.py -v` | 0 / 0 | Parser errors 0；35 total：33 passed、2 个显式 live gate skipped。新增判据结构化锁定 authorize 的 tag commit/tree 单次派生与输出、唯一且以 40 位 SHA 固定的 Rust build action、其 `with.build-args`，以及本地 `$candidateSha^{tree}` 派生和各阶段身份重验调用点。 |
| `pwsh -NoLogo -NoProfile -NonInteractive -File scripts/check-release-candidate.ps1 -ListOnly` | 0 | 17 步计划完整；两种 Rust 镜像均显示 execution base 的精确 commit `542d91582da926be3bc3ef2adffb3bdb9d00f39d` 与 tree `9814163f34f8315afcb3a54d211624e3786a759b`，UI 与 Ceph 步骤不伪造 Rust 身份。 |
| 以同一 commit/tree build arguments 直接构建正式 Dockerfile 的 Server、Worker | 1 → 0 | 首次调用前 Docker Desktop Linux engine 管道意外消失，未读取构建上下文；隐藏启动 Docker Desktop 后确认 `27.5.1 linux/x86_64`，从头重跑两种 release build 均通过。镜像 config digest 分别为 `sha256:5b65ebc7602a5a8ec61803027bc2be9d39834e7e9e40a6c769842a9190a0ef12` 与 `sha256:f84a6c7da6234b1fd8f519c4634606ba038e6f6044e7bb866bcee188563a336c`。 |
| `bash .github/scripts/tests/run-repo-policy-tests.sh` 与 `git diff --check` | 0 / 0 | release-state 4/4 与完整 repo-policy fixtures PASS；无 whitespace error。 |
| `scripts/check-fast.ps1`、`scripts/check.ps1`（Node 22.17.0） | 0 / 0 | 统一入口从头通过；可得 test count 与详细子门输出由命令记录承载，未运行 `-IncludeIntegration`，因为本轮不改应用、数据库、Compose 或业务行为。 |

## 7. 残余风险

- 合并后 clean-main preflight 尚未完成；当前不得创建版本 tag。
- workflow 的真实 Linux build、GHCR/SBOM/provenance 与测试环境部署只能由后续不可变 tag 运行证明，本地静态测试不能替代。
- Ceph CentOS Stream 9 OS family、rootful 测试 Docker 和 Alpha 产品范围等 R9A 残余风险不因本轮构建参数修复而消失。
