# Phase 4B 图执行状态机与 RunJournal

## 目标

- 在精确基线 `674c9ac922a0f8c4818ae1e979f89db295e792e6` 上，把 Phase 4A 的确定性 DAG 接入既有 append-only RunJournal。
- 由 Journal 前缀唯一推导已完成节点、最后安全 checkpoint 和中断后必须重跑的节点。

## 验收

- node started/succeeded/failed/checkpointed 事件进入既有 canonical hash、连续 sequence、prev hash 与幂等 append 合同。
- 图状态机严格按拓扑序执行；每个节点从 attempt 1 开始，中断重跑只接受同节点连续递增 attempt。
- succeeded 输出只有被相同 hash 的 checkpoint 提交后才计入 completed；提前 run success、错节点/attempt、重复或漂移 checkpoint 全部失败关闭。
- PostgreSQL RunJournal 事件约束与 codec 可无损承载四类新事件；最终 `./scripts/check-fast.ps1` exit code 0。

## 非目标

- 不实现 PostgreSQL Lease Queue、Worker 认领/续租、进程级恢复、NativeNode 计算、Artifact 发布、实验比较或 GeneratedNode。
- 不改变既有 ExperimentRun run 级状态、旧 Journal canonical code、业务 API、数值算法、Oracle、expected、断言或容差。
- 不创建版本 tag，不 push，不部署，不触发 GitHub CI/CD。

## 公共契约变化

- `JournalEventType` 追加 canonical code `8..11`：`NodeStarted`、`NodeSucceeded`、`NodeFailed`、`NodeCheckpointed`；旧 code `1..7` 不变。
- 新增 `ficant.graph-node-event.v1` 固定二进制 payload，绑定 node ULID、非零 attempt 与 succeeded/failed/checkpoint evidence hash。
- 新增 graph-aware replay；旧 `replay()` 继续只验证 Phase 1 run 级日志，不会把不理解的节点事件宽松解释为合法。
- Migration `0010_graph_journal_events.sql` 仅前向扩展 RunJournal event type check，不修改历史行。

## 需 Human 决策

- 当前无待决业务语义；本迭代不授权版本交付。

## 最终真实测试证据

- `cargo test --offline --locked -p ficant-runtime --test graph_execution`：exit code 0，4/4。
- `cargo test --offline --locked -p ficant-runtime`：exit code 0；graph execution 4/4、既有 journal ordering 7/7、既有 replay determinism 8/8。
- `cargo test --offline --locked -p ficant-storage --lib`：exit code 0，3/3；Journal codec 覆盖四类新增事件往返。
- `cargo clippy --offline --locked -p ficant-runtime -p ficant-storage --all-targets -- -D warnings`：exit code 0。
- `./scripts/check-fast.ps1`：exit code 0；工作区非环境测试与 doc tests 全部通过，storage library 3/3、Phase 3A 5/5、Phase 3B 2/2。

## 残余风险

- Graph replay 当前由调用方同时提供冻结 graph 与 Journal；ExperimentRun 对 graph digest、环境摘要和节点实现 digest 的持久绑定将在 Phase 4D-E 完成，在此之前不得把 graph-aware replay 描述为完整可复现实验。
- Migration 已进入本地 schema 合同，但本迭代不启动 PostgreSQL；真实 migration、并发 lease 和重启恢复由 Phase 4C 的 integration 验收负责。
