# ficant 验收与证据索引

> **历史记录（superseded）：** 除“当前治理证据”索引外，本文主体记录 iteration-2 当时的 PROQAID Quality、Delivery 和 Review 事实。旧角色名、路径和 verdict 仅用于审计，不是当前 OPAID 权威；后续治理边界见 ADR-0009，HOQA 状态与 Iteration 3 checklist 分别归档于 `docs/history/hoqa/governance/state.toml` 和 `docs/history/hoqa/iteration-3-checklist.md`。
>
> **后续处置：** 2026-07-19 的 Ceph RGW 迁移候选已从活动 Cargo/Compose/CI 合同移除 `minio` 与 `async-std`，并把风险接受集合收敛为空。下文继续保留 iteration-2 当时的原始证据；当前选择与升级条件见 ADR-0010。

> **当前候选：** R9A 已通过 PR #66 合入公共主线；Human 于 2026-09-04 选择 `v0.1.0-alpha.10` 后，首次 clean-main 发布预检在 tag 前发现 Rust release build 的候选 commit/tree 未传入并正确失败。R9B 只补齐本地/远端构建身份与防回归门，不改业务或数值证据；本地结果见 [`2026-09-r9b-release-identity-binding.md`](../iterations/2026-09-r9b-release-identity-binding.md)。版本 CI 和测试环境事实仍只能由 tag 后外部运行提供。

**iteration-2 结论：** iteration-2 已 `CLOSED`。原 Phase 0/1 的真实业务、运行时与可重放证据保持有效；2026-07-13 closure audit 对 `RUSTSEC-2025-0052` 完成了精确机器门、真实对象存储验证和候选绑定 CI。独立 Quality verdict 为 `PASS-WITH-ACCEPTED-RISK`，内部 Review 为 `pass-with-accepted-findings`（C0/I0/M1）；唯一 accepted finding 是下述限时维护风险，不是未关闭 blocker。

## 状态词汇

| 状态 | 含义 |
|---|---|
| `planned` | 已定义验收，但尚未收集执行证据 |
| `collected` | 已收集证据，尚未评审 |
| `passed` | 证据满足预期且已评审 |
| `failed` | 观察结果不满足预期 |
| `accepted-deviation` | 人类在当前清单中明确接受偏差 |

文档存在不等于可执行行为通过。

## iteration-2 唯一 Quality 退出证据集

### 候选绑定与结论

| 证据项 | 观察结果 |
|---|---|
| 最终执行 | GitHub Actions run [`29193249268`](https://github.com/kayz/ficant/actions/runs/29193249268)，`success`，10/10 job 通过 |
| 候选 | commit `ef96c5edea11b0d5f6ebc693501f40a9b40df061`；parent `42f570f309e20c867f65cffbce76e7f6d64d65d5` |
| 被验收树 | `2d1fa3a1be11e563c486d7c67df349ec06faf4d0`；与 integration commit `9f044b796a912746df2080c5d42bf696797c4424` 的树一致 |
| 固定环境 | 10 个 job 均声明 `ubuntu-24.04`；执行器为 Ubuntu 24.04.4、runner image `ubuntu24/20260705.232` |
| 执行时段 | 2026-07-12 `12:47:30Z` 至 `13:00:02Z` |
| Quality verdict | 原 Phase 0/1 **PASS**；2026-07-13 closure Quality 为 `PASS-WITH-ACCEPTED-RISK`，内部 Review 已补齐为 `pass-with-accepted-findings` |

此前六次失败运行 `29183053454`、`29185252321`、`29185454751`、`29189267286`、`29189731911`、`29190374218` 只作为故障发现与修复追溯证据，不计入任何通过结论；最终通过结论只绑定上述 run、候选和树。

### 十项门禁摘要

| Job | 退出证据 |
|---|---|
| `repo-policy` | 发布树 allowlist/deny、仓库策略与 Compose 配置静态安全检查通过；真实 Compose 运行验收见下述 Delivery 专项引用 |
| `contract` | 固定 Buf 基线、breaking 检查、Rust/Python/TypeScript consumer 与生成防漂移通过 |
| `rust` | 固定 Rust 1.96.1 容器内 workspace build、非验收模块测试及 storage library 测试通过 |
| `python` | 固定派生 `python-node-runtime` image 构建和运行通过 |
| `cpp` | 固定 Clang 18/CMake/Ninja 配置、构建和 CTest 通过 |
| `web` | 固定 Web build、Vitest、Playwright 与真实 gRPC-Web 路径通过 |
| `migration` | 真实 PostgreSQL 16 串行 Migration 门通过并完成服务清理 |
| `business-loop` | 真实 PostgreSQL 16.10 与 MinIO 上，正向业务闭环 1 项及负向不变量 13 项全部通过，0 failed、0 ignored |
| `supply-chain` | SBOM、许可证、漏洞、敏感信息与可达性门通过；候选绑定的许可证清单共 620 项，即第三方 607 项加精确枚举的一方内部包 13 项 |
| `reproducibility` | Rust、Python、C++、Web 四类可重放构建及产物比较通过 |

### Delivery Compose 专项引用

Delivery 在 commit `87db3897d82b0bea4e35eee3595178f366bbf041`、树 `e8fb65c5a86bac93382e93e50c90926954e4298f` 上完成唯一一次最终 Docker/Compose 运行验收：七服务 DAG 全部满足启动、健康、非 root、只读根文件系统、能力收口、回环端口、资源限制、重启持久性与最小 smoke 合同；最终 `down --volumes --remove-orphans --rmi local` 后，项目容器、网络、卷、构建镜像、测试数据和 cache tag 均为 0。该基线到最终发布树 `2d1fa3a1be11e563c486d7c67df349ec06faf4d0` 之间仅许可证清单和证据文档发生变化，Compose 配置、镜像构建配置及运行时安全检查 blob 未变化，因此 Delivery 证据继续适用。

13 项一方内部包均在许可证锁中按名称、版本、purl 与 source 精确枚举，不存在名称前缀豁免，该分类不构成任何开源授权。供应链通过包含人类批准的 D-026：`async-std 1.13.2` 经 `minio 0.4.0` 可达，`RUSTSEC-2025-0052` 状态为 `accepted-unfixed`，仅限 iteration-2；它没有被标记为已修复，必须在 iteration-3 入口或首次外部发布前（以较早者为准）重新评估。供应链 artifact `ficant-supply-evidence` 同时绑定候选 commit 与树；三类敏感信息扫描均为 0 finding。

## RUSTSEC-2025-0052 closure audit

2026-07-13 的复核结论是：**当前安全风险低、维护风险中等，保留 `accepted-unfixed`，不得标记为已修复或忽略。**

- RustSec 将该项分类为 `INFO / unmaintained`，说明 `async-std` 已停止维护；没有 CVSS、已知利用路径或 patched version。它不是已知内存安全、认证绕过、数据泄露或远程执行漏洞。
- `cargo tree -i async-std --locked` 证明发布 Workspace/生产 storage 代码链为 `async-std 1.13.2 -> minio 0.4.0 -> ficant-storage`。`ficant-storage` 的正式 MinIO adapter 调用 `get_object`、`put_object_content` 和 `delete_object`；`minio` 的请求签名/内容处理路径实际调用 `async_std::task::spawn_blocking`，因此不能降级为 lock-only 或不可达。当前 `ficant-server`/`ficant-worker` 组合根尚未直接装配该 adapter，这降低当前运行暴露但不改变发布 Workspace 的风险归类。
- 2026-07-13 crates.io API 显示 `minio 0.4.0` 仍为最新版；上游 `minio/minio-rs` 未归档且 2026-07-10 仍有提交，但最新版及 `master` 都继续无条件依赖 `async-std 1.13`。不存在可验证的 patch/minor 升级来消除该项。
- 用其他 S3 SDK 或维护本地 fork 会改变 storage adapter、依赖树、错误映射、签名与真实 MinIO 验收边界，超出“以现状收束”范围；不能以未经完整业务回归的依赖替换冒充安全修复。
- 当前补偿控制包括固定版本/lock、候选绑定的 SBOM/OSV/可达性门、MinIO 内网边界与凭证注入、不可变内容地址、正式读取 SHA-256/size/lineage fail-closed，以及真实 PostgreSQL/MinIO 业务与重启验收。它们降低当前暴露，但不消除上游停止维护的长期风险。

处置决定：D-026 作为明确的维护风险被 iteration-2 接受，接受范围仅为已验收的 Phase 0/1 内部开发切片；在 **iteration-3 Entry Gate、首次外部发布或 2026-10-13（最早者）**，Architecture/Delivery 必须选择并验证“上游移除 `async-std`、受控 fork 或迁移到受维护 S3 SDK”之一。任何版本、依赖链、调用边界、advisory 集合/分类或发布范围变化都会使本接受立即失效；供应链门必须继续保留原始 RustSec finding 和 `accepted-unfixed` provenance。

### Closure Quality 合同

| ID | 验收 | 当前状态 |
|---|---|---|
| `QRS-01` | advisory 精确为 `RUSTSEC-2025-0052`、`informational=unmaintained`、无 patched version | `passed`；官方 RustSec/OSV 复核 |
| `QRS-02` | `async-std 1.13.2 -> minio 0.4.0 -> ficant-storage` 可达链与实际 API 使用明确 | `passed`；Cargo tree、上游源码与本地 adapter 复核 |
| `QRS-03` | 关闭前 24 小时内确认 crates.io 最新版与上游维护状态 | `passed`；2026-07-13 API 复核 |
| `QRS-04` | 机器门只接受唯一 advisory；替换/追加 ID、security/unsound 漂移和过期全部 fail closed | `passed`；closure fixture 与 Supply job |
| `QRS-05` | 真实 PostgreSQL/MinIO 的五项 `ficant-storage` 对象存储集成测试通过 | `passed`；5 passed、0 failed、0 ignored，专用资源清理为 0 |
| `QRS-06` | Architecture/Delivery 所有权、2026-07-13 决策日及最早触发复核点写入机器策略 | `passed-with-accepted-risk`；最晚日期为 2026-10-13 |
| `QRS-07` | 最终证据绑定同一 clean candidate commit/tree，Quality 与 Review 给出退出 verdict | `passed`；`f492eefb...` / tree `5debcd4b...`、CI `29200796715`、Supply artifact 与两个独立 verdict |

Closure CI run [`29200796715`](https://github.com/kayz/ficant/actions/runs/29200796715) 在候选 `f492eefb19d7b60e74cbcc1b7a0b862b31bc3d1f` / tree `5debcd4b60b3585a4c168d3af0d5c92218ec528e` 上 10/10 job 成功。其 `accepted-unfixed.json` SHA-256 为 `76111880a5a61d4dbdf8c7c2274d0dbfbf24ab8c3d7b91b53dbd46aa906eb784`，只接受 `RUSTSEC-2025-0052`。最终状态 successor 只收敛本文档状态，不改变机器策略、生产代码、契约、Migration 或运行时；在 fast-forward `main` 前仍须通过同一 required CI 与 targeted final Review。

## 当前治理证据（更新于 2026-09-04）

### R7B 已落地证据边界

R7B 的公共实现、最终本地证据与主线合并已经完成；[`../iterations/2026-08-r7b-evidence-recovery.md`](../iterations/2026-08-r7b-evidence-recovery.md) 仍是该轮唯一 Human brief。落地能力包括统一 `FormalOutputEvidence`、同步 formal output 持久化、Graph 13 维、publication intent/orphan recovery、隔离备份恢复和 exact authority MANUAL runner。最终命令、exit code、test count、descriptor/生成树/恢复 manifest digest 与候选身份见该 brief §6；公共实现和本地证据不能代替 AC30–AC33 的独立 private authority 绑定。

R7B 的灾备证据明确限于本地隔离 source-destroy/fresh-restore：PG dump 与全部 immutable object key/size/SHA-256 同时绑定公共 Code 和实际 Runtime，恢复后 required-read Graph Artifact 与同步 Analytics record。它不构成生产 HA、PITR、RPO/RTO 或版本交付证据。

### R8B 已落地证据边界

R8B 已通过 [PR #65](https://github.com/kayz/ficant/pull/65) 合入 `main`；[`../iterations/2026-08-r8b-portfolio-performance.md`](../iterations/2026-08-r8b-portfolio-performance.md) 仍是该轮唯一 Human brief。其最终候选证据已覆盖 18-service descriptor/生产 route、69/7/62 coverage inventory、FormalInputKind 22..24、独立 Python Decimal Oracle 与 Rust 生产公式逐字段对照、PostgreSQL 0027 不可变/双时间/租户负例、幂等 bootstrap、native gRPC/gRPC-Web 真实组合，以及正式证据先持久化后返回和进程重启读回。

2026-09-04 在同步的公共基线 `main == origin/main == 1788bcfba8d0609002008043908c8f0013474fce` 上执行 `.\scripts\check.ps1 -IncludeIntegration`，exit `0`；最终隔离恢复 manifest SHA-256 为 `D2321253E7CA1B32905DDE2A6445FEC69AB1E5B11321AE98E6AE8A72C605FAAF`。完整原始候选证据、Git rebase 映射和残余风险见 R8B brief §6–§7。R8B 是研究计量输入与收益序列，不构成正式会计 NAV、PMS/OMS、版本发布或 WebApp 已接入证据。

## iteration-2 历史治理检查表

| ID | 验收 | 当前状态 | 证据位置 |
|---|---|---|---|
| QG-01 | 当前迭代、总体目标和有效期明确 | collected | `.proqaid/orchestrator/current-iteration.md` |
| QG-02 | Product/Architecture/Interface/Quality/Delivery/Review 全覆盖 | passed | Product/Architecture/Interface/Delivery 已完成；2026-07-13 closure Quality 与内部 Review 已补齐 |
| QG-03 | Codex/Claude 工具硬约束语义相同 | collected | `.codex/AGENTS.md`、`.claude/CLAUDE.md` |
| QG-04 | 每个角色 context 声明 docs 产物、用途和文件边界 | collected | `.proqaid/<role>/context.md` |
| QG-05 | Review 阻塞/重要发现全部路由 | passed | closure Review 为 `pass-with-accepted-findings`（C0/I0/M1）；M-01 文档措辞已纠正，唯一 accepted finding 为 D-026 |
| QG-06 | 清理与 Git 变更清单完整 | collected | `repo-policy` 已通过；最终 worktree、临时分支、Compose 资源与迭代归档由退出清理门确认 |
| QG-07 | 外部系统与密钥访问符合授权边界 | passed | GitHub 仓库创建与 allowlist push 已获授权；测试机和 `C:\git\key` 未访问；仓库敏感标记扫描无命中 |
| QG-08 | 无法证明的模型应用标为 unverified | collected | dispatch log 与各角色输出 |

## iteration-2 里程碑一：真实业务波次

### Verdict

**PASS。** 唯一业务命令退出码为 `0`，14 个测试全部通过、0 skipped。执行前后 integration worktree 均为 clean，HEAD 始终为 `dbcff34793e79e73ed63872e28ed6298feedfbc4`。

```bash
cargo nextest run --locked -p ficant-acceptance --test phase1_business_loop --test negative_invariants
```

| 证据项 | 观察结果 |
|---|---|
| 执行身份 | nextest run ID `24556685-366c-4060-821d-bf94b58a6802`；2026-07-12 `04:06:53Z` 至 `04:09:02Z` |
| 固定环境 | Ubuntu 24.04.4 LTS、x86_64；Rust/Cargo 1.96.1；cargo-nextest 0.9.140 |
| Fixture | `tests/golden-cases/china-rates/phase1-business-loop.json`；SHA-256 `133ce56bc70feaaac509fb205164fd20c1300425961600f5988540f1a37d67f6` |
| Runtime provenance | `sha256:8e97031468b2ad51ab8484d06d8af9d63f1b73f8c04654f17be40ac629076cd9`；当前业务 SHA 派生的 linux/amd64 `python-node-runtime` OCI image manifest digest |
| PostgreSQL | 真实 PostgreSQL 16.10，隔离数据库；镜像 RepoDigest `postgres@sha256:38471f330eb885e04de130b768d6db4e10469e2311879c7e5c699f6d2d8a1c74` |
| MinIO | 真实 MinIO `RELEASE.2025-04-22`，隔离 bucket；镜像 RepoDigest `minio/minio@sha256:a1ea29fa28355559ef137d71fc570e508a214ec84ff8083e39bc5428980b015e` |
| 持久执行证据 | `/var/tmp/ficant-iteration2-quality-wave-dbcff347/` 下的 `start.metadata`、`full.log`、`exit.code`、`end.metadata`；凭证未写入日志或本文 |

### 业务闭环观察

- 正向用例通过真实 application、PostgreSQL 与 MinIO 完成市场定义和事实 → Curve/Data/Universe Snapshot → ExperimentRun revision `1 → 2 → 3` → Artifact/SignalSet → RunJournal sequence `1..5`。
- 重连 PostgreSQL 与 MinIO 后，Artifact、SignalSet、DataSnapshot 和 UniverseSnapshot 四类 required read 均按冻结角色、hash、size 和 lineage 返回；成功路径未产生完整性事件。
- 两次 production replay 结果相同且事件数为 5；正式 MinIO 内容对象为 5 个，`staging_uploads=0`、`orphan_candidates=0`，逻辑业务行数与 fixture 合同一致。
- `Q2-INV-01..12` 全覆盖：单位、市场日期、不可变版本、Snapshot 漂移、幂等、断裂血缘、hash、Journal 顺序、promote 中断、并发、required read 完整性和 RulePack 半开区间均通过真实边界断言。
- D-023 的 object missing、同尺寸内容篡改、尺寸漂移、正式引用 hash 漂移和正式引用缺失全部 fail closed；返回 `HashMismatch`、`retryable=false`，每次恰好一个结构化完整性事件，且 metadata、Run、Journal、正式引用及七维副作用计数不增加。

本波次没有重复 Migration、server/storage 模块回归、Web 或 Docker/Compose 专项；这些仍由各自唯一责任门持有。

## Phase 0 Quality 证据映射

| ID | 必须证明的闭环 | 状态与证据 |
|---|---|---|
| QP0-01 | Ubuntu 24.04 x86_64 一条命令启动开发环境 | `passed`；固定 CI 环境及 Delivery 七服务 Compose 运行验收 |
| QP0-02 | Rust、Python、C++、Web 构建可重复，版本和 lock 固定 | `passed`；`rust`、`python`、`cpp`、`web`、`reproducibility` |
| QP0-03 | Protobuf 对 Rust/Python/TypeScript 生成且 CI 防漂移 | `passed`；`contract` |
| QP0-04 | PostgreSQL 16 空库迁移、重复执行、升级/恢复证据 | `passed`；最终 `migration` job 的真实 PostgreSQL Migration acceptance |
| QP0-05 | README Phase 0 命名交付物清单完整 | `passed`；`repo-policy` 与现有权威文档复核 |
| QP0-06 | 唯一技术栈、可复现、依赖/SBOM/漏洞与密钥安全门禁 | `passed`，含 D-026 `accepted-unfixed`；`supply-chain` 与 `reproducibility` |

## 后续 DMQuant 业务闭环证据

以下全部为 `planned`：

| ID | 必须证明的闭环 |
|---|---|
| QD-01 | AI 流式输出、生成步骤、完成、断线/失败与重试 |
| QD-02 | AI 参数应用、人工修改和来源标记 |
| QD-03 | 策略新版本保存与幂等回测提交 |
| QD-04 | queued/running/succeeded/failed/cache 状态及阶段/进度/原因 |
| QD-05 | 指标、校验、fingerprint、NAV/信号序列与单位正确 |
| QD-06 | 草稿/失败策略源码与成功运行 Artifact 的不同可用规则 |
| QD-07 | 错误 `code`、`trace_id`、复制和恢复路径 |
| QD-08 | viewer/researcher 后端权限与对象级 RBAC/ABAC |
| QD-09 | 导出/下载/删除的确认、审计与失败结果 |
| QD-10 | 时区、无障碍和评审工具条不进入产品 |

## JSON 证据边界

未来机器可读证据只能放在 `docs/quality/evidence/*.json`，并至少包含：验收 ID、版本/摘要、环境、动作、预期、观察、采集时间和评审状态。必须脱敏；禁止凭证、令牌、私钥、敏感业务载荷、原始二进制、测试代码或生产 Artifact。

## Validity

Valid: long-term until superseded
