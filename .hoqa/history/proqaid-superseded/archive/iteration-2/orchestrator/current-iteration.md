# Current Iteration: iteration-2

## Status

- **Lifecycle:** `CLOSED`。最终 GitHub `main` 为单提交收束候选 `80f48706f37e3890224ca106fb763213d0beeb38`，tree 为 `9dd6a136d453872dc37085bc55903eb90978fdf9`，唯一 parent 为此前已发布基线 `737807302351fe8feee425a89d666caf3d611f96`；候选 CI run `29201419136` 与 main CI run `29201891313` 均十项全绿。
- **PROQAID operating level:** `Full`（用户未指定 Lite；2026-07-11 切换到新版 PROQAID 后继续沿用七个常驻角色，Design Freeze 不重复）。
- **Current implementation state:** Phase 0/Phase 1、真实 PostgreSQL/MinIO 业务闭环、Migration、四类构建、Web、契约、供应链、可重放构建与唯一 Compose 专项均已通过。`RUSTSEC-2025-0052` 已完成证据化评估并以精确 advisory、限时、fail-closed 的 `accepted-unfixed` 策略受控接受；最早在 iteration-3 Entry Gate、首次对外发布或 2026-10-13 重新评估。独立 Quality 结论为 `PASS-WITH-ACCEPTED-RISK`；最终定向 Review 为 `pass-with-accepted-findings`（C0/I0/M0），仅保留已接受的 D-026。临时 worker、worktree、分支、缓存、测试数据和 Compose 资源均在关闭阶段清理。
- **Previous iteration:** iteration-1 closed with `pass-with-accepted-findings` and archived under `.proqaid/archive/iteration-1/`.
- **Project classification:** existing governed project with current governance and an initial GitHub baseline.

## Project Goal

ficant 是面向专业投资研究团队的固定收益优先、AI 原生、领域驱动、可复现量化研究操作系统。总体生产目标仍按 README Phase 0 至 Phase 9 推进，平台输出止于研究产物、仿真结果、报告、`SignalSet` 和 `TargetExposure`。

## iteration-2 Proposed Objective

把 README Phase 0 与 Phase 1 合并为一个开发轮次：建立唯一技术栈、可复现仓库/契约/开发环境，并实现核心市场事实与研究资产模型，使一条真实的市场事实 → 快照 → 实验 → 产物/信号 → RunJournal 血缘闭环可在真实 PostgreSQL/MinIO 环境中执行、查询、版本化和重放。

## Human Requirements

- `docs/` 中本轮产出的文档使用中文。
- 页面设计和 WebApp 代码位于根目录 `web-dm/`；后台接口与 Protobuf 唯一契约位于根目录 `interface/`，为未来多 WebApp 提供共享接口边界。
- Quality 在设计、开发中间检查和最终验证阶段可多次启动；开发使用 TDD，测试必须验证真实业务和领域不变量。
- Orchestrator 可派发多个临时 worker；worker 使用隔离工作区和互斥写入范围，完成后清理。
- 优先更新已有文档；仅在 Phase 0 强制交付或目录共置要求无法由现有文档承担时新增文档。

## Confirmed Constraints

- iteration-2 从 GitHub/local `main` 建立独立分支和 worktree；不得在旧本地 `master` 上开发。
- Phase 2 的定价、收益率、曲线和风险数值算法不提前伪实现。
- 现有 GitHub allowlist 必须扩展到经确认的 Phase 0/1 源码根目录；PROQAID、工具约束、hidden 和旧 UI-DM 仍保持本地 ignored。
- 测试机与 `C:\git\key` 不因 checklist 确认而自动授权访问；远程验证仍需明确用户名与具体密钥文件路径。
- Standing roles 不写生产代码；只有 Orchestrator-dispatched workers 可在分配范围内实施。

## Expected Planning Deliverable

1. 人类确认的 `iteration-2-checklist.md`。
2. 常驻角色设计轮次与 Quality 测试设计轮次。
3. 详细实施计划、worker 切分和业务验收证据矩阵。

以上三项及完整 Phase 0/1、真实 PostgreSQL/MinIO 业务闭环和发布就绪均已完成。

## Approval Record

- Human confirmed `iteration-2-checklist.md` on 2026-07-11.
- Human additionally required full archival of iteration-1 `.proqaid` artifacts; completed under `.proqaid/archive/iteration-1/`.
- `docs/` natural-language outputs must be Chinese.
- Human approved hybrid verification cadence on 2026-07-11: WSL for high-frequency language/business tests, Docker/Compose once per stage for container-specific acceptance; Ubuntu 26.04 cannot substitute for Ubuntu 24.04 evidence, and the test VPS remains unused.
- Human approved migration to the updated PROQAID skill on 2026-07-11: apply at the next safe Task 4 event boundary; preserve the existing Design Freeze; use unique test ownership, risk/wave Reviews, reader-driven memory, parallel safe slots, milestone-only reporting, and no GitHub `main` integration before iteration exit.
- Human clarified on 2026-07-12 that PROQAID and Superpowers are overlapping governance systems and must not run together. PROQAID remains the sole iteration governor; inherited `.superpowers/` artifacts are read-only historical evidence until iteration-2 archival, while TDD/worktree/debugging/verification remain implementation techniques only.

## Validity

Valid: iteration-2 only
