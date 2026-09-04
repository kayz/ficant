# R9F 迭代 brief — CI 源码身份闭环

**面向 Human 的产品名：** 金证FICC合同管理系统 · **平台名：** FICANT · **内部迭代：** R9F · **execution base：** `6b194996cce06d8fefee91b130e28869a3ae5293` · **base tree：** `2f5f73381c0701e061802a56f34c7aa4f7e8a3ff` · **状态：** 本地候选完成，待合入

本 brief 是 R9F 面向 Human 的唯一范围、权限边界与最终本地证据载体。`v0.1.0-alpha.10` 已不可变地绑定上述 base；其 [CI run 33889960292](https://github.com/kayz/ficant/actions/runs/33889960292) 在本地 17 步 preflight 通过后暴露了干净 Linux checkout / 无 `.git` archive 中的源码身份传播缺口，因此版本 CI 失败；后续 [release-test run 33890473662](https://github.com/kayz/ficant/actions/runs/33890473662) 的 7 个 job 全部 skipped，未构建或推送版本应用镜像，也未部署测试环境。

## 1. 目标

让版本 CI 的每条 Server/Worker 编译路径显式继承已授权 tag 的 commit/tree，让 reproducibility 的两份无 `.git` archive 仍构建同一源码身份，并把 contract breaking baseline 改绑到公共 `main` 中内容等价且可由常规 fetch 获得的祖先提交。

## 2. 验收

一句话验收：在精确 R9F 候选上，CI 静态合同拒绝任一 Rust 编译入口缺失 commit/tree，reproducibility 两份 archive 以同一经 Git 校验的身份完成可复现构建，contract gate 使用 `main` 可达且 `interface/` 内容不变的 baseline；目标 gate、`check-fast.ps1` 与标准 `check.ps1` 全部 exit `0`。

| 条目 | 可执行判据 |
|---|---|
| 授权身份 | `authorize-version` 在确认 tag 精确位于当前 `main` 后输出 40 位小写 commit/tree；下游不得自行猜测或使用零值。 |
| CI 容器 | `rust`、Web 的 Worker/Server、`business-loop` 所有可能编译 Server/Worker 的 Rust 容器都同时显式传入两个身份变量；删除任一注入的夹具必须失败。 |
| 可复现归档 | `verify-reproducibility.sh` 在真实 worktree 一次性解析并校验 HEAD commit/tree，拒绝 caller drift，并把二者导出给两份无 `.git` archive 的 Rust build。 |
| 契约基线 | breaking baseline 使用 `01123c02291310bbe6fb90071b2512ec444a8a3d`；它必须是候选祖先，且与旧悬空对象 `6c805930f201b3d82bbcbee9030b791e48fb08e7` 的 `interface/` 内容无差异。 |
| 本地证据 | `run-gates-tests.sh`、`run-repo-policy-tests.sh`、Compose security tests、真实 contract/reproducibility gates、Linux Docker 身份探针、最终 23 步快速检查与 40 步标准检查均 exit `0`。 |
| 版本边界 | `v0.1.0-alpha.10` 保持原位且不重跑；R9F 合入后也不得在 Human 明确选择新的 forward-only 版本号前创建 tag。 |

## 3. 非目标

- 不修改 `binaries/ficant-server/build.rs`、`binaries/ficant-worker/build.rs`，不放宽缺失/非法源码身份时的 fail-closed 行为。
- 不修改业务域、API、Web 功能、migration、依赖/lock、生成契约、descriptor hash、Dockerfile 或镜像锁。
- 不重跑、移动、复用或删除 `v0.1.0-alpha.10`；承认其 tag 已创建并推送，但不得描述为 CI 通过、应用镜像发布或测试环境交付成功的版本。
- 不在本轮执行测试环境部署、回滚或服务器管理；这些仍由新的不可变版本流水线证明。

## 4. 公共契约变化

- 业务公共契约：无变化。
- 交付合同：版本授权 job 公开精确 `sha` / `tree` 输出；所有相关 Rust 编译消费者必须显式接收二者。
- 证据合同：contract baseline 必须是当前候选祖先；reproducibility archive 的二进制身份必须等于来源 worktree，而不是依赖 archive 内不存在的 Git 元数据。

## 5. 需 Human 决策

- R9F 候选完成并合入后，需 Human 明确选择新的 forward-only 版本号；建议届时使用 `v0.1.0-alpha.11`。
- 本轮不需要更改业务语义、Oracle、expected、断言或容差；若实施发现必须如此，立即停止并返回 Human。

## 6. 最终真实测试证据

以下只记录最终候选上已实际执行的命令、exit code、可得 test count 与失败/跳过事实。

| 真实命令/检查 | Exit / Conclusion | 结果 |
|---|---:|---|
| `v0.1.0-alpha.10` / CI run `33889960292` | `failure` | authorize、Python、migration、repo-policy、C++、supply-chain 通过；Rust、contract、Web、reproducibility、business-loop 失败。 |
| release-test run `33890473662` | `skipped` | authorize、build、build-ui、scan、promote、deploy 等 7 个 job 全部 skipped；无版本应用镜像和测试环境部署。 |
| 五个失败 job 的独立只读日志诊断 | 0 | 四个 job 共因源码身份未传入容器/archive；contract 独立因 baseline 为不可 fetch 的悬空对象。未修改或重跑旧 tag。 |
| `bash .github/scripts/tests/run-gates-tests.sh` | 0 | 源码身份的未设置、匹配、空值、半设置、漂移及子进程继承矩阵全部通过；删除生产 baseline 调用等变异均被拒绝。 |
| `bash .github/scripts/tests/run-repo-policy-tests.sh` | 0 | release-state 9/9、repo policy 通过；21 个 CI 身份传播变异全部被拒绝。 |
| `python -m pytest .github/scripts/tests/test_compose_security_gate.py -q` | 0 | 37 个测试：35 passed，2 个需真实 registry 的 live 测试显式 skipped；另执行 79 个 subtests。 |
| CI YAML 解析与 job 清单检查 | 0 | PyYAML 成功解析 11 个 job；本机未安装 `actionlint`，未把该项写成通过。 |
| 固定 Rust Linux 镜像内的 Server/Worker 身份探针 | 0 | bind-mounted checkout 显式注入候选 commit/tree 后，`cargo check --locked -p ficant-server -p ficant-worker` 通过。 |
| 真实 `verify-contract-generation.sh` | 0 | 可达 baseline `01123c0...` 与旧 baseline 的 `interface/` tree 相同；descriptor `0de1176...`；Rust 34、Python 1、TypeScript 1 个 consumer 测试及类型检查全部通过。 |
| 真实 `verify-reproducibility.sh` | 0 | 两份不含 `.git` 的 archive 均完成 Rust、Python、C++、Web 构建；四类制品逐类哈希一致。 |
| `.\scripts\check-fast.ps1` | 0 | 23/23 个快速本地步骤通过。 |
| `.\scripts\check.ps1` | 0 | 40/40 个标准本地步骤通过。 |
| 独立增量复审 | 0 | blocker / major / minor = 0 / 0 / 0；冻结合同、失败闭合及变异覆盖未发现遗漏。 |

## 7. 残余风险

- R9F 只能准备新的本地候选；远端 Linux CI、镜像发布和测试环境交付必须由 Human 另行确认的新版本 tag 提供证据。
- 本地 Docker 探针可覆盖 bind mount 与显式身份注入，但不能替代 GitHub runner 的完整矩阵。
- 本机没有 `actionlint`；GitHub workflow 语法仅由 PyYAML、静态夹具和后续远端 runner 共同覆盖。
- GitHub Actions 当前仍提示部分固定 action 使用 Node.js 20 runtime 并被 runner 强制到 Node.js 24；这是上游 action runtime 提示，不是本次五个 job 的失败根因，后续应独立处理。

### 冻结写路径

- `.github/workflows/ci.yml`
- `.github/scripts/verify-contract-generation.sh`
- `.github/scripts/verify-reproducibility.sh`
- `.github/scripts/tests/run-gates-tests.sh`
- `.github/scripts/tests/run-repo-policy-tests.sh`
- `docs/iterations/2026-09-r9f-ci-source-identity.md`（本文件）
- `docs/iterations/README.md`
- `docs/delivery/release-notes.md`
- `docs/delivery/test-environment.md`
- `docs/quality/evidence.md`

上述闭集之外的源码、构建脚本、lock、migration、镜像与私有 authority 均不得修改。
