# FICANT 迭代 brief

本目录只保存面向 Human 的迭代 brief，不是项目状态台账。每个迭代恰好一份 brief；Agent 交流、Worker 证据、中间候选和子循环 checkpoint 由编排工具承载，不在这里复制。

每份 brief 只使用以下七个部分：目标、验收、非目标、公共契约变化、需 Human 决策、最终真实测试证据、残余风险。最终证据必须来自最终本地候选上的实际命令，包含 exit code 和可得的 test count；未执行的计划不得写成通过。候选身份由 Git commit 与 Pull Request 事实源绑定，不在 tracked brief 中复制自身 Commit SHA。

## 执行边界冻结

- brief 的 §6 允许写路径清单与 execution base 同时冻结。它是权限边界，不是实施进度记录；执行开始后不得就地编辑、补写或以最终事实替换该清单。
- 如确需新增写路径，Root 必须在首次写入前停止并取得 Human 明确授权；扩权只能作为新的 §5 条目记录精确路径、理由与边界，原 §6 清单保持不变。事后发现的越界必须如实记录为偏差，不得用修改 §6 追认。
- 被约束的实施者不得单方改写用来判断其是否越界的约束。这一原则同样适用于允许写路径、guarded 集合、预期值、Oracle、断言与其他自管门禁；需要变更时必须由独立授权或独立可审阅证据承载。

**当前迭代：** [`2026-09-r9e-release-runtime-identity.md`](2026-09-r9e-release-runtime-identity.md)（R9D 已通过 [PR #69](https://github.com/kayz/ficant/pull/69) 线性合入 `main@433d03dd998a1e5829fc7bbc2ec6438e66cbfe00` / tree `1806850afbc1b3ca1690c30cc94a7cc3dd8aa17f`。第四次 clean-main preflight 已通过构建、四镜像扫描、resolved Compose 与空库 migration，随后因 Server 缺少 `FICANT_CODE_COMMIT_SHA` 而 unhealthy。R9E 已补齐完整 commit/tree、Server runtime/environment/bootstrap/input、Worker orphan、部署状态与回滚身份合同；定向真实 Compose 探针、compose security 37 tests（35 passed、2 个显式 live gate skipped）、state contract 9/9、repo-policy、实际镜像摘要 resolved validator 和修复后独立复审 Blocker/Major/Minor `0/0/0` 均已通过；最终 `check-fast.ps1` exit `0`、23/23 步通过，完整 `check.ps1` exit `0`、40/40 步通过。本地候选已完成并可提交/合并；提交/合并与第五次 clean-main preflight 尚待执行，`v0.1.0-alpha.10` 仍未创建，版本 CI 与测试环境交付尚未运行。）

**最近完成迭代：** [`2026-09-r9d-compose-candidate-binding.md`](2026-09-r9d-compose-candidate-binding.md)（[PR #69](https://github.com/kayz/ficant/pull/69) 已线性合入上述 `main`；R9D 的目标回归、快速及标准完整本地检查均已通过。合入后的第四次 preflight 在 tag 前暴露 Server 运行时身份缺口并由 R9E 接续，不把该失败描述为版本交付。）

## 归档说明（2026-07-26）

Phase 0–5A 的 23 篇 brief 已移入 [`../history/iterations/`](../history/iterations/README.md)。

它们记录的是**当时**的证据，不再驱动当前工作。分层重构（见 [`../architecture/layering-refactor.md`](../architecture/layering-refactor.md)）已获 Human 批准做破坏性契约变更，因此其中 Phase 2C 与 Phase 3A/3B 的取证将分别在 R2 与 R4 后失效并重跑；其余 brief 的证据在其声明的候选上仍然成立，但不构成对重构后系统的任何保证。

判定权威位于私有 `kayz/ficant-authority` 仓库中，由该仓库 `authority-manifest.json` 精确绑定公共提交的 `SPEC.md` 与 `ACCEPTANCE.md` 提供；公共仓库根目录的同名本地文件（若存在）不具权威性，判定权威也不在任何 brief。
