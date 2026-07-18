# iteration-2 设计：Phase 0 + Phase 1

## 目标

在一个开发轮次内建立可复现的唯一技术栈与契约/存储/运行基线，并实现 Phase 1 的市场事实和研究资产领域内核。交付必须通过真实 PostgreSQL 16、MinIO 和 Rust 服务形成市场事实 → 快照 → 实验 → RunJournal → Artifact/最小 SignalSet → 查询/重放/拒绝覆盖闭环。

## 范围边界

- `Valuation`、`CurveSnapshot` 和来源方 `Cashflow` 只保存外部输入事实；不实现 Phase 2 定价、收益率、曲线或风险算法。
- 最小 `SignalSet` 只实现领域记录、不可变和血缘；不包含 Phase 9 Registry、审批、下游发布或 `TargetExposure`。
- iteration-2 只实现 Platform Shell 与多 WebApp 加载边界，不实现完整 DMQuant。
- C++ 只验证 Clang/CMake/Ninja/C ABI 可构建基线，不提供数值假实现。

## 目录与所有权

```text
interface/                         唯一 Protobuf/后台接口源（W2）
crates/ficant-domain/              纯领域类型与不变量（W2）
crates/ficant-application/         用例与 ports（W2）
crates/ficant-runtime/             Journal 顺序与重放（W2）
crates/ficant-contracts/           Rust 生成契约（W2）
crates/ficant-contract-tests/      descriptor/漂移测试（W2）
crates/ficant-storage/             PostgreSQL/MinIO adapters（W3）
crates/ficant-api/                 Registry/session/error gRPC-Web adapter 与生成契约到 domain 的转换（W1 集成）
crates/ficant-acceptance/          真实业务闭环测试（W3）
migrations/postgresql/             前向 Migration（W3）
web-dm/platform-shell/             Web 宿主（W4）
web-dm/webapps/<app-id>/           页面设计、源码、manifest、测试共置（W4）
deploy/dev/                        Compose 与配置（W1）
cpp/fixed-income-kernel/           仅工具链/C ABI 自检（W1）
python/                            GeneratedNode 基础镜像与契约 smoke（W1；生成契约子目录由 W2 独占）
python/node-contracts/src/ficant_contracts/generated/  Python 生成契约（W2）
web-dm/packages/contracts-generated/                   TypeScript 生成契约（W2）
.github/workflows/                 十项 CI gates（W1）
tests/golden-cases/                确定性中国国债 fixture（W3）
docs/                              当前中文人类文档（Orchestrator 串行合并）
```

README 中的 `proto/` 已收敛为根 `interface/`。`web-dm` 只消费生成 client，不拥有后台事实。

## 模块与依赖

```text
ficant-api → ficant-application → ficant-domain
ficant-application → ficant-runtime → ficant-domain
ficant-storage → ficant-application + ficant-domain
web-dm → generated gRPC-Web client → ficant-api
```

`ficant-domain` 不依赖 SQLx、网络、文件系统、Web、模型服务或容器。Repository/BlobStore 端口属于 application；PostgreSQL/MinIO 实现在 storage；Protobuf 类型属于 API/生成边界。

## 领域语义

- Definition：Instrument/Bond/FuturesContract/Calendar/Unit/MarketRulePack 按 identity 追加版本。
- Fact：Cashflow/Quote/Trade/Valuation 按来源幂等键追加 revision，允许 `supersedes_id`，不改历史。
- Snapshot：CurveSnapshot/DataSnapshot/UniverseSnapshot 内容寻址，发布后不可变。
- Run：ExperimentRun 重跑创建新 ULID，状态按 expected revision/CAS 转移。
- Artifact/SignalSet：服务端复算 SHA-256，血缘完整后发布，不可修改。
- RunJournal：按 run + sequence 仅追加，乱序/重复/并发冲突返回稳定错误。

所有对象共享 ULID、明确版本、Decimal/单位、UTC 与市场时区、租户/所有者、内容哈希、状态和血缘引用。

## 契约与错误

根 `interface/proto/ficant/{core,market,research,app}/v1/` 是唯一 Protobuf 输入。Rust、Python 和 TypeScript 生成物由 Buf 管理并在 CI 防漂移。

稳定错误码至少覆盖：非法单位、非法生效时间、版本冲突、内容哈希不匹配、断裂血缘、非法状态迁移、Journal 乱序、未授权、资源不存在和依赖不可用。API 错误携带安全消息与 `trace_id`，不得泄露内部 SQL、密钥或对象存储凭证。

## PostgreSQL / MinIO 数据流

```text
请求验证
→ PostgreSQL staging/事务上下文
→ MinIO staging object
→ 服务端流式复算 SHA-256
→ 幂等 promote 到 ficant-<env>-immutable/sha256/<prefix>/<hash>
→ PostgreSQL metadata + lineage commit
→ RunJournal append
```

数据库提交失败产生的未引用对象由带宽限期的 orphan collector 清理；已有 metadata/lineage 引用的对象不得删除。Migration 只前向升级，证明空库、上一版本升级、重复执行、失败原子性和 reconciliation。

## Platform Shell

`web-dm/platform-shell/` 负责 Registry、应用路由、会话与错误边界。`web-dm/webapps/<app-id>/` 共置设计、manifest、源码与测试。Phase 0 Shell 验证 registry/app/session/error/permission/a11y 状态，通过生成 gRPC-Web client 调用真实 Rust 服务；不以硬编码响应或完整 DMQuant 假页面充数。

## TDD 与 Quality

- 每个行为必须先产生带 `Q2-*` ID 的有效红灯，再用同一命令变绿，随后跑所属层回归和重构后回归。
- 真实边界使用 PostgreSQL 16/MinIO 容器；禁止 mock repository、内存数据库和本地目录假对象存储替代。
- Quality round-2 审契约/Migration，round-3 审领域/真实存储切片，round-4 审完整 Compose/业务/Web/清理。
- 证据记录命令、cwd、git SHA、工具/容器摘要、fixture hash、退出码、expected/observed、数据库/对象存储指纹和 reviewer；无业务观察的 exit 0 最多是 collected。

## Worker 编排

```text
W1 基线/共享根
→ W2 契约/领域/runtime/application
→ (W3 storage/Migration/业务闭环 ∥ W4 web-dm)
→ W1 串行集成
→ Quality round-4 + 各角色复核 + Review
```

- W1 独占 Workspace/locks/toolchain/Compose/CI/binaries/C++、除 `python/node-contracts/src/ficant_contracts/generated/` 外的 Python 基线、allowlist 与最终集成。
- W2 独占 `interface/`、`ficant-contracts`、`ficant-contract-tests`、domain/application/runtime、contract/domain tests，以及 Python/TypeScript 的两个精确生成目录。
- W3 独占 storage/migrations/acceptance/golden fixtures。
- W4 独占除 `web-dm/packages/contracts-generated/` 外的 `web-dm/`，只读消费 W2 生成契约，不写生成契约或共享根。

唯一开发环境启动命令为 `docker compose -f deploy/dev/docker-compose.yml --project-name ficant-iteration-2 --profile dev up --build --wait`；所有角色、CI 和 worker 只可基于这一个 Compose 文件、project name 和 profile 派生命令。
- 每个 worker 使用独立 worktree/分支；生产 worker 不直接互相通信，所有冲突由 Orchestrator 路由。

## 文档策略

`docs/` 自然语言使用中文并原位更新。DMQuant 详细页面设计已迁到 `web-dm/webapps/dmquant/design.md`；`docs/interface/ui-reference.md` 仅作索引。后台契约说明位于 `interface/README.md`。除中文 ADR 模板外不新建平行报告。

## 完成标准

完成标准以 `iteration-2-checklist.md` 为唯一合同，并要求 Product/Architecture/Interface/Quality/Delivery 复核、Review `pass` 或 `pass-with-accepted-findings`、全部 worker 清理、无测试桩/假实现/硬编码成功数据/未解决占位。

## Validity

Valid: iteration-2 only
