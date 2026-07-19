# iteration-2 PROQAID 退出摘要

> **历史记录（superseded）：** 本文保留 iteration-2 在旧治理下的关闭事实。PROQAID、HOQA、Review、角色路径和 verdict 不得用于驱动当前 OPAID 工作；后续治理边界见 ADR-0009，旧状态与 checklist 分别归档于 `docs/history/hoqa/governance/state.toml` 和 `docs/history/hoqa/iteration-3-checklist.md`。
>
> **后续处置：** 2026-07-19 的 Ceph RGW 迁移候选已从活动 Cargo/Compose/CI 合同移除 `minio` 与 `async-std`，并把风险接受集合收敛为空。下文仍只描述 iteration-2 当时的接受事实，当前决策见 ADR-0010。

**迭代状态：** `CLOSED`

**Review 状态：** `pass-with-accepted-findings`（C0/I0/M1）

**权威治理记录：** `.proqaid/orchestrator/decisions.md` 的 D-027

## 结论

用户于 2026-07-13 要求以现状评估 `RUSTSEC-2025-0052` 并完全关闭 iteration-2。closure audit 已完成风险复核、独立 Quality、内部 Review 与最终状态收敛，不改变 Phase 0/1 业务行为。历史 `Review skipped by explicit human authorization` 只保留为过程事实，已被本次正式 Review 取代，不再是最终偏差状态。

iteration-2 已取得以下确定性退出证据：

- Ubuntu 24.04 GitHub Actions 十项 gate 全部成功；
- 真实 PostgreSQL/MinIO Phase 1 业务闭环、Migration、required published-content read、完整性事件与确定性重放通过；
- Rust、Python、C++、Web、Contract 与可重放构建通过；
- Supply inventory 为 607 个第三方包与 13 个精确一方包，secret 扫描为零；
- 七服务 Docker/Compose runtime、安全、重启持久性与零残留清理通过；
- D-025 的发布候选为可信 `main` 基线的唯一单提交子节点，禁止 merge commit 与 force-push。

## 已接受风险

`async-std 1.13.2` 保持 `accepted-unfixed`，不得标记为已修复或忽略。2026-07-13 复核确认该 RustSec 项为 `INFO / unmaintained`、没有 patched version；发布 Workspace/生产 storage adapter 的依赖链与实际调用均可达，但当前 server/worker 尚未直接装配该 adapter。`minio 0.4.0` 已是 crates.io 最新版且上游 `master` 仍依赖 `async-std`，所以没有安全的小版本消除路径。当前安全风险评为低、维护风险评为中，接受仅覆盖既有 Phase 0/1 内部开发切片；iteration-3 Entry Gate、首次外部发布或 2026-10-13 前（最早者）必须验证替代方案。

## Closure Review

- Quality final verdict：`PASS-WITH-ACCEPTED-RISK`；`QRS-01..07` 全部满足，无 blocker。
- Review exit verdict：`pass-with-accepted-findings`，C0/I0/M1；M-01 的旧 deviation 措辞已在本摘要纠正。
- 唯一 accepted finding 是 D-026 的限时维护风险；它不等于 fixed/ignored，也不是开放 blocker。
- 候选 `f492eefb...` / tree `5debcd4b...` 的 GitHub CI `29200796715` 为 10/10 success，Supply artifact 只接受目标 advisory。
- 最终状态 successor 仅更新中文状态文档；fast-forward `main` 前仍须通过 required CI 和 targeted final Review。

## Validity

Valid: iteration-2 only
