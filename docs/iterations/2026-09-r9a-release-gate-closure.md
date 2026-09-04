# R9A 迭代 brief — `v0.1.0-alpha.10` 发布门禁收口

**面向 Human 的产品名：** 金证FICC合同管理系统 · **平台名：** FICANT · **内部迭代：** R9A · **execution base：** `1788bcfba8d0609002008043908c8f0013474fce` · **base tree：** `0c255e73bb2ac13ce1e5d1d8c654d6cb7a6d0ac5` · **状态：** 本地候选完成，待 clean `main` 发布候选预检

本 brief 是 R9A 面向 Human 的唯一范围、权限边界与最终本地证据载体。开始实施时，`main == origin/main`，工作树另有一组已由 Human 要求完成的当前状态文档差异，其起始 diff identity 为 `4dd71ddf238625e89fc9f4a72a18288b95d18374`；两者共同构成本轮输入。

## 1. 目标

把当前公共主线收敛为可供 Human 建立 `v0.1.0-alpha.10` 不可变版本 tag 的精确候选：关闭 repo-policy 的两个既有失败，令测试环境状态发布符合原子更新合同，统一不可变版本流水线的取消语义，消除统一本地检查遗留契约包 `dist` 所造成的不可重入，并把 2026-09-04 的 current truth 文档纳入同一候选。

**Acceptance sentence：**

> 在不改变任何业务、数值、Proto、Oracle、expected 或容差语义的前提下，FICANT 的安全契约测试与 repo-policy 必须全绿，版本部署状态只能通过同目录完整临时文件原子替换，已开始的不可变版本流水线不得被后续运行自动取消，统一本地检查必须可连续执行且不把自身 ignored 产物误当发布输入；最终候选须通过规定本地检查，并在合入后以干净且精确同步的 `main` 通过发布候选预检，才允许创建 `v0.1.0-alpha.10` tag。

## 2. 验收

| 条目 | R9A 可执行判据 |
|---|---|
| Repo policy | `.github/scripts/tests/test_compose_security_gate.py -v` 原 34 项中的两项失败关闭，且 CORS、镜像 config/source identity、secret、health、volume 与 gRPC status 安全语义不弱化；完整 `run-repo-policy-tests.sh` exit 0。 |
| CORS | 开发 Compose 保持两个精确 origin：Platform Shell `http://127.0.0.1:18083`（端口可显式覆盖）与相邻 WebApp 开发入口 `http://127.0.0.1:5173`；禁止 wildcard 或其他隐式扩大。 |
| Worker identity | `dev-up.ps1` 继续通过单一 helper 从实际 Worker image config 读取并验证 runtime/source digest；测试验证行为与调用关系，不强迫重复 inline 实现。 |
| 原子状态 | `current.env` 与 `previous.env` 均在目标目录创建完整临时文件、设置正确权限后通过同文件系统 rename 发布；写入失败不得暴露半文件，临时文件须清理。 |
| 并发语义 | `cicd.yml` 与 GitHub workflow 都明确 `false`：已开始的不可变版本 CI/部署不得因另一运行而自动取消。 |
| 可重入检查 | 显式 `package-contracts.ps1` 仍产出可消费包；统一检查拥有内部 `dist` 的确定生命周期，连续执行不会使 license/supply-chain 输入集合漂移。所有修改的 PowerShell 在首次执行前 parser error 为 0。 |
| 一方包策略 | 发布预检精确验证 20 个一方包：18 Cargo、1 PyPI SDK、1 npm generated-contract package；不得通过改 lock、漏计 npm 或虚构已删除 Cargo package 制造通过。 |
| 最终可执行候选 | `check-fast.ps1`、`check.ps1`、`check.ps1 -IncludeIntegration` 与 `git diff --check` 在最终可执行候选上通过；其后的 docs-only 证据后继须重跑所有受文档与仓库状态影响的快速门，不得以计划、旧提交或 Worker 声明替代 Root 最终证据。 |
| 版本候选 | 合入后 `main` 必须 clean 且精确等于 `origin/main`；更新 Trivy 0.72.0 DB 后，完整 `check-release-candidate.ps1` exit 0，随后才进入 CICD 创建不可变 tag。 |

## 3. 非目标

- 不增加产品能力，不改变 R5D–R8B 的业务、数值、Proto、数据库或正式证据语义。
- 不修改任何金融 Oracle、expected、fixture 结果或容差来制造通过；测试变化只能提高对既有安全行为的准确约束。
- 不引入新的镜像、服务、端口、migration、依赖或可变镜像标签。
- 不把本地证据描述为 Linux CI、GHCR、SBOM/provenance、测试环境或生产证据。
- R9A 本地实施期间不创建、移动或删除任何版本 tag，不推送镜像、不连接测试服务器、不执行生产发布或 UAT。
- 不修改 ignored 的私有权威文件，不替 Human 改写 `ACCEPTANCE.md` 状态。

## 4. 公共契约变化

- 业务与 Protobuf 公共契约无变化。
- 开发 Compose 的两个精确 CORS origin 与端口合同不变；门禁只从落后的单 origin 字符串断言更新为验证完整精确 allowlist。
- 测试环境 `current.env` / `previous.env` 的发布从直接覆盖改为同目录原子替换，文件字段与消费方合同不变。
- CICD 元数据与 workflow 统一为不取消已开始的不可变版本运行。
- 契约包命令的外部产出合同不变；只为统一检查增加精确、受边界验证的内部临时产物生命周期。

## 5. 需 Human 决策

| 决策 | Human 已确认选择 | 边界 |
|---|---|---|
| D1 版本号 | `v0.1.0-alpha.10`。 | 不复用或移动 `v0.1.0-alpha.9` 及任何历史 tag。 |
| D2 交付顺序 | 先闭合门禁并提交干净主线，再运行发布候选预检；只有全部通过后创建 tag。 | 失败时停止，不创建“已知会失败”的占位 tag。 |
| D3 自动交付 | 创建该 tag 即授权仓库既有版本 CI、不可变镜像构建/扫描与测试环境交付。 | 不含生产发布；生产仍需独立 Human 授权。 |
| D4 并发取向 | 版本运行不可变且不得自动中止；中央元数据向实际 workflow 的 `cancel-in-progress: false` 收敛。 | 不改变普通开发任务或未来未冻结的调度策略。 |
| D5 一方包断言 | Root 根据冻结 `supply-chain.lock.json` 将基线遗留的 `19 Cargo + 1 PyPI` 断言更正为 `18 Cargo + 1 PyPI + 1 npm`，并增加 npm purl 精确断言。 | 不修改 lock、inventory、package identity 或许可证策略。 |
| D6 CORS validator | Root 在提交前审阅中发现通用 Compose 安全 validator 仍拒绝实际双 origin 合同，因具体失败证据批准 `.github/scripts/compose_security_gate.py` 作为本轮唯一补充写路径，并令 validator 精确验证既有开发 allowlist；原 §6 冻结闭集保持不变。 | 不新增 origin；只接受一个显式端口的 `127.0.0.1` Platform Shell origin 加固定 `http://127.0.0.1:5173`，拒绝单项、第三方/第三项、wildcard、重复、空格与带 path 的条目。 |
| D7 新策略测试路径 | Root 在 tracked-path 预演中确认新 Python 策略测试若提交会被现有精确语言 allowlist 拒绝，批准 `.github/scripts/verify-repo-policy.sh` 作为第二个补充写路径，只加入 `.github/scripts/tests/test_release_state_contract.py` 这一精确条目；原 §6 冻结闭集保持不变。 | 不放宽其他 Python 路径、语言或临时文件策略；必须在 staged/临时 index 使新文件可见的条件下重跑最终 repo-policy。 |
| D8 final path gate | 在新测试已进入 index 后，真实 `verify-repo-policy.sh --stage final` 暴露 Git C-style 转义的两个中文合法路径及已跟踪的四个 Portfolio Oracle 未纳入 allowlist。Root 批准继续在 D7 的同一补充策略文件内关闭基线阻塞：`git ls-files` 显式禁用 `core.quotepath`，Python allowlist 只增加 `tests/oracle/portfolio/*`。 | 不改变根路径、秘密、临时文件或后端语言禁令；夹具须覆盖新测试、Portfolio Oracle 与中文路径，真实 final gate 必须在新测试可见时通过。 |

## 6. 最终真实测试证据

**实施允许写路径（开始实施前冻结的闭集）：**

- `AGENTS.md`
- `README.md`
- `interface/README.md`
- `docs/architecture/data-dictionary.md`
- `docs/architecture/layering-refactor.md`
- `docs/delivery/release-notes.md`
- `docs/delivery/test-environment.md`
- `docs/development.md`
- `docs/interface/ui-reference.md`
- `docs/iterations/2026-08-r8b-portfolio-performance.md`
- `docs/iterations/2026-09-r9a-release-gate-closure.md`（本文件；实施开始后只在本节追加真实证据并更新第 7 节）
- `docs/iterations/README.md`
- `docs/product/scope.md`
- `docs/quality/evidence.md`
- `.github/scripts/tests/test_compose_security_gate.py`
- `.github/scripts/tests/test_license_inventory_bindings.py`
- `.github/scripts/tests/test_release_state_contract.py`（可新建）
- `.github/scripts/tests/run-repo-policy-tests.sh`
- `.github/scripts/verify-supply-chain.sh`
- `.github/scripts/verify-license-inventory.py`
- `.github/workflows/ci.yml`
- `cicd.yml`
- `deploy/dev/docker-compose.yml`
- `deploy/test/bin/deploy.sh`
- `scripts/dev-up.ps1`
- `scripts/package-contracts.ps1`
- `scripts/test-contract-package.ps1`
- `scripts/test-contract-package-reentrancy.ps1`（可新建）
- `scripts/check-fast.ps1`
- `scripts/check.ps1`

**受保护事实：** 以上闭集之外的实现、Proto、migration、Cargo/npm/Python 依赖、供应链 lock、金融 Golden/Oracle/expected/容差、ignored private authority、远端权限与测试机均不得修改。版本 tag、镜像、部署和 GitHub workflow 运行属于 OPAID 完成后的 CICD 活动。

本节以下只追加最终可执行候选上的实际命令、exit code、可得 test count、精确候选身份与必要失败恢复；完成自测后只允许追加 Human 可读证据的 forward-only 后继，并须证明可执行树未变化、相关快速门已重跑。计划命令不得写成通过。

**RED 与局部收口证据：**

| 真实命令/检查 | Exit code | 结果 |
|---|---:|---|
| `python .github/scripts/tests/test_compose_security_gate.py -v`（execution base） | 1 | 34 total：30 passed、2 failed、2 skipped；失败精确为旧单 origin 与旧 inline image-inspect 断言。 |
| license binding → contract package → license binding（execution base） | 0 / 0 / 2 | 打包 6/6 后 ignored `dist` 令第二次 license binding 报 generated-contract first-party binding mismatch，复现不可重入。 |
| `python .github/scripts/tests/test_license_inventory_bindings.py -v`（execution base） | 1 | 11 total：10 passed、1 failed；冻结策略为 20 个一方包，旧断言错误要求 19 Cargo 且漏验 npm。 |
| `python .github/scripts/tests/test_compose_security_gate.py -v` | 0 | 34 total：32 passed、2 个显式 live gate skipped、0 failed；Root 复核保持 `18083` + `5173` 精确 allowlist，仅更新落后夹具与 helper 行为验证。 |
| `python .github/scripts/tests/test_license_inventory_bindings.py -v` | 0 | 11/11；精确断言 18 Cargo、1 PyPI 与 1 npm purl。 |
| `python .github/scripts/tests/test_release_state_contract.py -v`（首次 Windows 动态复核） | 1 | 4 total：2 passed、2 failed；Python 文本模式把传给 Bash stdin 的 LF 转为 CRLF，两个动态判据均在执行被测函数前失败，未作为候选绿灯。改为显式 UTF-8 bytes 并保持原断言后，同命令 4/4、Git Bash 4/4。 |
| `bash .github/scripts/tests/run-repo-policy-tests.sh` | 0 | 原 repo-policy fixtures PASS；新增 release-state contract 4/4。 |
| `bash .github/scripts/verify-repo-policy.sh --stage final`（新测试进入 index 后） | 1 → 0 | 首次真实 final gate 揭示 6 个基线策略偏差：2 个中文 tracked path 被 Git C-style 转义，4 个已跟踪 Portfolio Oracle 不在精确 Python allowlist。按 D7/D8 修复后，同一命令 PASS；新 release-state test、中文文档与 Portfolio Oracle 均由夹具覆盖。 |
| `pwsh -NoLogo -NoProfile -NonInteractive -File scripts/test-contract-package-reentrancy.ps1`（锁定 Node 22.17.0） | 0 | 7 项外层判据；2 次 contract test 共 12 项、2 次 license binding，结束后 `repository_output_removed=true`。 |
| PowerShell Parser（4 个改动/新增脚本）与 `bash -n deploy/test/bin/deploy.sh` | 0 / 0 | PowerShell parse errors 0；Bash syntax valid。 |
| `pwsh -NoLogo -NoProfile -NonInteractive -File scripts/check.ps1`（锁定 Node 22.17.0） | 0 | 首轮在 Web typecheck 前因本 checkout 缺 ignored `node_modules` exit 1；随后 `pnpm@10.12.4 install --offline --frozen-lockfile` 复用 178/178、下载 0，完整入口从头重跑通过：C++ 9/9、Web 35/35、contract reentrancy 7 项及全部 Rust/Python/Oracle/策略检查通过。 |
| 隔离 Compose 上的 `pwsh -NoLogo -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration` | 0 | 绑定可执行候选 commit `20fa5b3fc1d51ef4694745c66577bbf7c8f4bc51`、tree `2a160cb0ab48129b385890f9746e4dd4375ec56b`。Migration 7/7、lease 1/1、execution closure 3/3、production Worker 1/1、Phase 1 正向 1/1 与负向 13/13、Phase 2B/2C/2D 各 1/1、Phase 3A registry/parity 各 1/1、Phase 3B codec 3/3 与 publication 1/1、R6A/R6B 各 1/1、R8A PostgreSQL 5/5 与生产 gRPC/gRPC-Web 1/1、R8B PostgreSQL 1/1 与生产 gRPC/gRPC-Web 1/1，以及 R7B source-destroy/fresh-restore 全部通过；恢复 manifest SHA-256 为 `DDF47CCA00F15A4863273B03C20EC05A65BBDD9CC9A59C338DB98B0737D1AE20`。 |
| 隔离环境退出清理 | 0 | `ficant-r9a-20fa5b3f`、R7B source 与 restore 的容器、网络和数据卷均通过精确 Compose project name 执行 `down --volumes --remove-orphans` 并删除。 |
| `git diff --check`、最终 repo-policy、Markdown 相对链接与 `scripts/check-fast.ps1`（证据后继复核） | 0 | 证据后继只更新本 brief 与证据索引，不改变上述已验证可执行树。首次快速门因调用 shell 的默认 Node 为 24.18.0、与冻结的 22.17.0 不符而在契约打包前正确 exit 1；显式锁定 Node 22.17.0 后从头重跑 exit 0。格式、tracked-path policy 与 2 份改动 Markdown 的 3 个相对链接（0 broken）均通过后才提交。 |

R9A 的自测主体绑定上述可执行候选；随后只追加 Human 可读证据的 forward-only 后继。合并后的精确 `main` 仍须重新通过完整发布候选预检，不能用本节本地证据替代 tag 后的 Linux CI、镜像、SBOM/provenance 或测试环境事实。

## 7. 残余风险

- 完整发布候选预检尚未在合并后的 clean `main == origin/main` 上执行；预检失败时不得创建 tag，且不得把本地自测冒充远端交付证据。
- Ceph CentOS Stream 9 OS family 仍不受 Trivy 0.72.0 完整 OS 包识别，语言包扫描不等价于完整 Ceph OS 漏洞覆盖。
- 测试环境仍使用 rootful Docker；本轮只闭合版本交付合同，不迁移宿主隔离模型。
- `alpha.10` 仍是内部 Alpha：完整业务 WebApp、GeneratedNode/gVisor、OIDC 与业务 UAT 不属于本轮。
