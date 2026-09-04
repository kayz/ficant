# FICANT 迭代 brief

本目录只保存面向 Human 的迭代 brief，不是项目状态台账。每个迭代恰好一份 brief；Agent 交流、Worker 证据、中间候选和子循环 checkpoint 由编排工具承载，不在这里复制。

每份 brief 只使用以下七个部分：目标、验收、非目标、公共契约变化、需 Human 决策、最终真实测试证据、残余风险。最终证据必须来自最终本地候选上的实际命令，包含 exit code 和可得的 test count；未执行的计划不得写成通过。候选身份由 Git commit 与 Pull Request 事实源绑定，不在 tracked brief 中复制自身 Commit SHA。

## 执行边界冻结

- brief 的 §6 允许写路径清单与 execution base 同时冻结。它是权限边界，不是实施进度记录；执行开始后不得就地编辑、补写或以最终事实替换该清单。
- 如确需新增写路径，Root 必须在首次写入前停止并取得 Human 明确授权；扩权只能作为新的 §5 条目记录精确路径、理由与边界，原 §6 清单保持不变。事后发现的越界必须如实记录为偏差，不得用修改 §6 追认。
- 被约束的实施者不得单方改写用来判断其是否越界的约束。这一原则同样适用于允许写路径、guarded 集合、预期值、Oracle、断言与其他自管门禁；需要变更时必须由独立授权或独立可审阅证据承载。

**当前迭代：** [`2026-09-r9f-ci-source-identity.md`](2026-09-r9f-ci-source-identity.md)（R9E 已通过 [PR #70](https://github.com/kayz/ficant/pull/70) 合入 `main@6b194996cce06d8fefee91b130e28869a3ae5293` / tree `2f5f73381c0701e061802a56f34c7aa4f7e8a3ff`，随后第五次 clean-main 发布预检 17/17 步全部通过。不可变 tag `v0.1.0-alpha.10` 已创建并推送；[版本 CI run 33889960292](https://github.com/kayz/ficant/actions/runs/33889960292) 的 authorize、Python、migration、repo-policy、C++、supply-chain 通过，Rust、contract、Web、reproducibility、business-loop 失败，[release-test run 33890473662](https://github.com/kayz/ficant/actions/runs/33890473662) 因上游失败而 skipped，未构建发布镜像、未部署测试环境。R9F 正在以 forward-only 候选补齐已授权 commit/tree 到所有 Server/Worker 编译入口的传播，并把 contract baseline 改绑到公共 `main` 可达的内容等价祖先；新的版本号仍待 Human 在本地候选完成并合入后确认。）

**最近完成迭代：** [`2026-09-r9e-release-runtime-identity.md`](2026-09-r9e-release-runtime-identity.md)（[PR #70](https://github.com/kayz/ficant/pull/70) 已合入上述 `main`；R9E 的目标回归、快速及标准完整本地检查与第五次 clean-main preflight 均已通过。`alpha.10` 随后的远端失败作为 R9F 的输入保留，不回写成 R9E 的通过结论。）

## 归档说明（2026-07-26）

Phase 0–5A 的 23 篇 brief 已移入 [`../history/iterations/`](../history/iterations/README.md)。

它们记录的是**当时**的证据，不再驱动当前工作。分层重构（见 [`../architecture/layering-refactor.md`](../architecture/layering-refactor.md)）已获 Human 批准做破坏性契约变更，因此其中 Phase 2C 与 Phase 3A/3B 的取证将分别在 R2 与 R4 后失效并重跑；其余 brief 的证据在其声明的候选上仍然成立，但不构成对重构后系统的任何保证。

判定权威位于私有 `kayz/ficant-authority` 仓库中，由该仓库 `authority-manifest.json` 精确绑定公共提交的 `SPEC.md` 与 `ACCEPTANCE.md` 提供；公共仓库根目录的同名本地文件（若存在）不具权威性，判定权威也不在任何 brief。
