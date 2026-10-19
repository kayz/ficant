# FICANT 迭代 brief（历史）

自 2026-10-19 起不再新增 brief，开发以 Pull Request 交付，见 [ADR-0019](../architecture/adr/0019-lightweight-development-keep-release-boundary.md)。本目录中的 brief 保留为当时的设计与证据记录，不再驱动当前工作。

**最后一份 brief：** [`2026-09-r9g-linux-release-parity.md`](2026-09-r9g-linux-release-parity.md)（[PR #73](https://github.com/kayz/ficant/pull/73) 已合入；不可变 `v0.1.0-alpha.12@42e7c323543401567f8d76166d4d13958dd55f7b` 的 clean-main preflight 17/17、[版本 CI](https://github.com/kayz/ficant/actions/runs/33943037740) 11/11、[release-test](https://github.com/kayz/ficant/actions/runs/33943924358) 10/10 均成功。）

## 归档说明（2026-07-26）

Phase 0–5A 的 23 篇 brief 已移入 [`../history/iterations/`](../history/iterations/README.md)。

它们记录的是**当时**的证据，不再驱动当前工作。分层重构（见 [`../architecture/layering-refactor.md`](../architecture/layering-refactor.md)）已获 Human 批准做破坏性契约变更，因此其中 Phase 2C 与 Phase 3A/3B 的取证将分别在 R2 与 R4 后失效并重跑；其余 brief 的证据在其声明的候选上仍然成立，但不构成对重构后系统的任何保证。

判定权威位于私有 `kayz/ficant-authority` 仓库中，由该仓库 `authority-manifest.json` 精确绑定公共提交的 `SPEC.md` 与 `ACCEPTANCE.md` 提供；公共仓库根目录的同名本地文件（若存在）不具权威性，判定权威也不在任何 brief。
