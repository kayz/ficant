# iteration-2 Deviations

No iteration-2 scope deviation is currently accepted.

## D2-IMPL-01 — Initial Cargo member glob correction

- **Observed:** Cargo 1.96 rejects an unmatched `crates/*` workspace member glob before W2 is allowed to create any crate.
- **Correction:** W1 baseline registers only real `binaries/*`; the first W1 lock checkpoint adds `crates/*` immediately after W2 scaffold integration, updates `RustService.Dockerfile` to copy the real crate tree, and reruns the unique Compose command.
- **Reason:** avoids fake/empty production crates while preserving the reviewed single-owner lock handoff.
- **Scope impact:** none; no checklist, domain, dependency direction, worker ownership, or acceptance ID changes.
- **Gate:** Architecture and Review must recheck this clarification before Task 2 production behavior proceeds.

## D2-IMPL-02 — Hybrid local verification cadence

- **Observed:** 本机现有 WSL 是 Ubuntu 26.04，而项目最终兼容性基线是 Ubuntu 24.04；对每个开发红绿循环重复执行完整 Docker 构建会显著拖慢反馈。
- **Correction:** Rust、Python、C++、Node、单元测试及大部分业务测试在 WSL 高频执行；Docker/Compose 仅在阶段验收执行容器专属安全与清理检查。Ubuntu 26.04 只提供快速反馈，最终兼容性证据仍来自固定 Ubuntu 24.04 环境。
- **Reason:** 缩短 TDD 周期，同时不降低 Ubuntu 24.04 与容器运行时的验收标准。
- **Scope impact:** 无产品/目录/接口范围变化；只调整验证执行位置与频率。
- **Gate:** 当前 Task 1 因 D2-D-01/D2-D-02 必须完成一次 Docker/Compose 最终验收；后续阶段不得把 Ubuntu 26.04 结果标记为 Ubuntu 24.04 兼容性通过。测试 VPS 暂不使用。

## D2-TDD-01 — Bootstrap initial RED validity (pending human decision)

- **Observed:** 共享 `ficant-bootstrap` 的首个记录红灯因生产类型尚未定义而在编译阶段失败，没有到达目标业务/行为断言。
- **Correction:** 不把该证据计作关闭对应 `Q2-TDD-*`；后续所有行为任务必须先有可编译 scaffold/lock，再以到达目标断言的失败作为有效 RED。
- **Reason:** 保留真实历史，同时禁止用 setup/compile failure 代替行为 TDD 证据。
- **Scope impact:** Task 1 已完成行为与回归证据，不要求重写提交历史；iteration exit 是否接受此历史偏差仍需人类明确决定。
- **Gate:** iteration exit 前由人类接受或拒绝；当前状态为 `pending`，不得静默视为已接受。

## D2-IMPL-03 — Generated Python runtime lock handoff

- **Observed:** pinned `protocolbuffers/python:v31.1#2` output imports `google.protobuf` and carries runtime guard 6.31.1; governed `uv run --locked pytest` also requires a locked pytest executable. W2 owns generated files/tests but is forbidden to edit shared `python/pyproject.toml`/`uv.lock`.
- **Correction:** W2 stops at the real import failure and requests exact runtime `protobuf==6.31.1` plus dev `pytest==8.4.1`; a dedicated W1 checkpoint edits only the shared Python manifest/lock, verifies exact sync/import/tool versions, and integrates before W2 fast-forwards and reruns the identical import test.
- **Reason:** preserves W1 single ownership of shared locks and prevents `--with`, ambient site-packages or an unlocked test runner from manufacturing GREEN.
- **Scope impact:** no product/interface/directory scope change; this is the Python equivalent of the already-reviewed serial Cargo lock handoff.
- **Gate:** W1 checkpoint task review and integration, followed by W2 identical-command GREEN. W1 cannot claim generated import behavior because generated files remain only in W2's dirty worktree until Task 2d commit.

Potential decisions must be routed here before implementation when they change the confirmed checklist, README technical invariants, directory ownership, TDD evidence, or GitHub publication boundary.

## Validity

Valid: iteration-2 only
