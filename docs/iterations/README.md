# FICANT 迭代 brief

本目录只保存面向 Human 的迭代 brief，不是项目状态台账。每个迭代恰好一份 brief；Agent 交流、Worker 证据、中间候选和子循环 checkpoint 由编排工具承载，不在这里复制。

每份 brief 只使用以下七个部分：目标、验收、非目标、公共契约变化、需 Human 决策、最终真实测试证据、残余风险。最终证据必须来自最终本地候选上的实际命令，包含 exit code 和可得的 test count；未执行的计划不得写成通过。候选身份由 Git commit 与 Pull Request 事实源绑定，不在 tracked brief 中复制自身 Commit SHA。

**当前迭代：** [`2026-08-r1-layer-contract-skeleton.md`](2026-08-r1-layer-contract-skeleton.md)

## 归档说明（2026-07-26）

Phase 0–5A 的 23 篇 brief 已移入 [`../history/iterations/`](../history/iterations/README.md)。

它们记录的是**当时**的证据，不再驱动当前工作。分层重构（见 [`../architecture/layering-refactor.md`](../architecture/layering-refactor.md)）已获 Human 批准做破坏性契约变更，因此其中 Phase 2C 与 Phase 3A/3B 的取证将分别在 R2 与 R4 后失效并重跑；其余 brief 的证据在其声明的候选上仍然成立，但不构成对重构后系统的任何保证。

判定权威在根目录的 `SPEC.md` 与 `ACCEPTANCE.md`，不在任何 brief。
