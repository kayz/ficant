# Phase 4D-E NativeNode 与运行时收口

## 目标

- 在精确基线 `84402a21405629ed8db2ac290fdb05fbad9d5e1a` 上交付 NativeNode 执行、冻结执行身份、逐节点 Artifact 血缘、确定性重放校验与实验比较。
- 以一个本地收口候选满足 README Phase 4 的三个退出条件。

## 验收

- ExecutionIdentity 绑定 Data/Universe Snapshot、Graph、Parameters、Runtime Image、Environment、Seed 和每节点 Implementation digest。
- NativeNode 只按确定性拓扑序执行；每条输入来自已验证上游输出，输出端口及 type ID/version/schema hash 精确匹配合同。
- 每节点 Artifact 绑定身份、合同、实现、上游 Artifact 和输出 hash；任一输出可反向追踪完整节点链。
- 相同身份重放对象级完全一致；实验比较精确区分输入/代码/环境/seed/result 差异。
- 最终 `./scripts/check-fast.ps1` exit code 0。

## 非目标

- 不实现 GeneratedNode/gVisor、Phase 5 Rates Lab、UI、业务 NativeNode catalog、持续 worker daemon、优先级/死信或集群调度。
- 不修改数值算法、Oracle、expected、断言、容差、Protobuf 或发布合同。
- 不创建版本 tag，不 push，不部署，不触发 GitHub CI/CD。

## 公共契约变化

- `ficant-runtime` 新增 `ExecutionIdentity`、`NativeNode`、`NativePortValue`、`NativeNodeArtifact`、`NativeExecutionResult`、重放校验和 `ExperimentComparison`。
- canonical v1 execution identity 与 node artifact digest 均使用长度分隔/固定顺序 SHA-256；executor 或调用方 collection 顺序不进入结果。
- 实验比较维度固定为 DataSnapshot、UniverseSnapshot、Graph、Parameters、RuntimeImage、Environment、Seed、Implementation、Result。

## 需 Human 决策

- Phase 4 本地内核是否按本 brief 验收；本迭代不授权版本交付。

## 最终真实测试证据

- `cargo test --offline --locked -p ficant-runtime --test native_execution`：exit code 0，3/3。
- `cargo clippy --offline --locked -p ficant-runtime --all-targets -- -D warnings`：exit code 0。
- `./scripts/check-fast.ps1`：exit code 0；工作区非环境测试与 doc tests 全部通过，新增 Native execution 3/3、既有 Graph execution 4/4、storage library 3/3、Phase 3A 5/5、Phase 3B 2/2。

## 残余风险

- NativeNode trait 与 engine 已执行真实 Rust 实现并验证类型/血缘，但首个固定收益业务节点 catalog 尚未装配；它应随 Phase 5 人可触业务切片增加，而不是在无产品验收下继续扩通用框架。
- Lease Queue、graph Journal 与 Native engine 已分别用真实 PostgreSQL/纯运行时验证，尚未组成常驻 `ficant-worker` 进程的端到端故障注入；Phase 5 第一个业务切片必须把三者装配并人工中断一次。
