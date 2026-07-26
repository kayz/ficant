# Phase 4C PostgreSQL Lease Queue

## 目标

- 在精确基线 `bb54cd50b342d788c72072268978aa884db5af0c` 上交付 tenant 隔离、数据库时钟驱动的 PostgreSQL execution lease queue。
- 让多 worker 原子 claim，不重复执行同一 lease；让进程中断后的过期任务可被另一 worker forward-only 回收。

## 验收

- enqueue 不可变绑定 run、node、node attempt、graph digest 和稳定 task key；相同请求幂等，不同内容冲突。
- claim 使用 `FOR UPDATE SKIP LOCKED`，并发 worker 获得不同任务；跨 tenant 不可见。
- renew/complete 同时校验 worker、lease ID 与数据库时钟有效期；完成 hash 不可变，相同完成重试幂等。
- 过期 lease 可被新 worker 回收并增加 claim count；migration 与 queue 行约束拒绝非法中间状态。
- 真实 PostgreSQL integration 与最终 `./scripts/check-fast.ps1` 均 exit code 0。

## 非目标

- 不在 `ficant-worker` 进程中执行 NativeNode，不发布 Artifact/Journal checkpoint，不实现 GeneratedNode、调度优先级、退避或死信队列。
- 不改变 Phase 4A DAG、Phase 4B 图重放语义、数值算法、Oracle、expected、断言或容差。
- 不创建版本 tag，不 push，不部署，不触发 GitHub CI/CD。

## 公共契约变化

- Migration `0011_execution_lease_queue.sql` 新增 `research.execution_tasks`，状态仅为 PENDING/LEASED/COMPLETED，并用行级 CHECK 固定 lease/completion 字段组合。
- `PostgresLeaseQueue` 新增 enqueue、claim、renew、complete；lease 时长限定 `1..=3600` 秒，所有到期判断使用 PostgreSQL `CURRENT_TIMESTAMP`。
- `scripts/check.ps1 -IncludeIntegration` 新增 Phase 4C lease queue SIT，不改变普通快速检查或版本 CI 触发边界。

## 需 Human 决策

- 当前无待决业务语义；本迭代不授权版本交付。

## 最终真实测试证据

- `cargo clippy --offline --locked -p ficant-storage --all-targets -- -D warnings`：exit code 0。
- 一次性本地 PostgreSQL 16 容器 + `cargo test --offline --locked -p ficant-storage --test lease_queue_sit -- --test-threads=1`：exit code 0，1/1；覆盖幂等 enqueue、并发 claim、续租所有权、幂等完成、完成漂移、过期回收与 tenant 隔离。
- 同一 PostgreSQL 16 + `cargo test --offline --locked -p ficant-storage --test migration_acceptance -- --test-threads=1`：exit code 0，4/4；11 个 migration 重复与失败原子性通过。
- `./scripts/check-fast.ps1`：exit code 0；工作区非环境测试与 doc tests 全部通过，storage library 3/3、Phase 3A 5/5、Phase 3B 2/2。

## 残余风险

- 本迭代完成的是 worker-safe 持久协议与真实并发恢复，不是 `ficant-worker` 的业务执行循环；NativeNode 执行、Journal checkpoint 与 Artifact 原子发布将在 Phase 4D-E 装配。
- queue 当前采用 tenant 内 FIFO + task ULID 稳定排序，不承诺优先级、公平配额、退避或死信策略；这些需要真实负载证据后再设计。
