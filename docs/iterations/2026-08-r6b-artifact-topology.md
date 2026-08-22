# R6B 迭代 brief — Artifact 与生产拓扑闭合

**迭代：** R6B · **点亮目标：** 无新增 AC（服务拓扑闭合） · **execution base：** `0ccd079f8a575b82b3f107e506e3d8e6dcc215f6` · **authority commit：** `368a90cbf27cb42667d38e289fda1cc657013a17`

本 brief 是 R6B 面向 Human 的唯一设计与后续最终证据载体。R6A 已由公共 PR #60 以 rebase merge 进入 `main`；合并后的 Git tree 与取得完整本地、集成证据的 R6A 候选完全相同。R6B 只处理 Artifact 的诚实服务边界、公共服务生产可达性和冗余 Web 进程，不接入组合投研 WebApp、COGA 或任何新 WebApp。2026-08-19 Human 在审阅准备结果后指令“完成 R6B”，明确批准 §5 的推荐 D1–D6；R6A/AC37 后绑定已先由 private authority PR #21 独立完成。本文据此冻结公共契约、execution base、实施闭集与受保护事实并授权代码实施。

## 1. 目标

R6B 只交付一个平台结果：公共契约声明的每个服务都由同一个生产路由组合真实承载，同时 Artifact 的发布与查询不再存在“契约看似可用、生产无法诚实执行”的假入口。

现有 Rates Analytics、ResearchGraph 和 Worker 已能在服务端完成 blob stage、hash/size 验证、不可变提升与 Artifact/SignalSet 持久化；R6B 不建立第二条发布算法。公共 Artifact 查询只有在 PostgreSQL metadata、领域 payload、lineage edge、正式 blob reference 与 Ceph-compatible payload 全部一致时才成功。服务拓扑由 descriptor 与实际 production routes 的机械门禁约束，不能再通过只增加 proto、生成 stub 或测试 fake 宣称服务已实现。只提供健康检查的 `ficant-web` 冗余进程退出拓扑，React `ficant-ui` 继续直接反代 `ficant-server`，不新增或替换业务 UI。

**Acceptance sentence：**

> 给定最终 descriptor 中任一公开 FICANT service，生产 `ficant-server` 的唯一 route builder 必须同时在 native gRPC 与 gRPC-Web 上承载它；从 route builder 删除任一服务或向 descriptor 增加一个未组合服务时，fast topology gate 必须失败。给定由既有 Rates、ResearchGraph 或 Worker verified publish 路径产生的 exact Artifact 或 SignalSet，Researcher 以正确 owner/tenant/scope 查询时，服务必须先验证数据库字段、领域 payload、完整有序 lineage、Artifact↔SignalSet 绑定、blob reference、实际 bytes、hash 与 size，再返回 metadata 或分页 lineage；任一身份、owner、kind、media type、hash、size、lineage、cursor 或 payload 漂移均返回统一 typed error 并在需要时记录 integrity event，不返回部分结果。generic caller 不能只提交 Artifact metadata 就制造“已发布”结果。生产及发布拓扑不再包含只做健康检查的 `ficant-web`，R6A、R5D/R5E 与 Phase 4 的既有发布、重放和恢复语义不回退。

## 2. 验收

| 条目 | R6B 可执行判据 |
|---|---|
| Artifact 发布权威 | 正式 Artifact/SignalSet 只由已经持有服务端 `VerifiedBlobRef` 的 Rates Analytics、ResearchGraph/Phase 1 或 Worker/Phase 4 use case 发布。公共请求不得把 `artifact_id + content_hash + blob_size + lineage` 的自称 metadata 当作 blob 验证证明；不得从 hash/size 直接构造虚假 verified capability。 |
| Artifact exact get | `GetArtifact` 在成功前必须逐项交叉验证 tenant/owner、artifact id、kind、media type、content hash、blob size、编码 payload、SQL columns、lineage edges、正式 blob reference 和实际 immutable bytes。metadata 缺失返回 NotFound；metadata 存在但 payload/blob/lineage 缺失或漂移返回 HashMismatch/LineageIncomplete/ImmutableViolation，并产生不含敏感内容的 integrity event。 |
| SignalSet exact get | `GetSignalSet` 必须验证 SignalSet 与独立 Artifact 的 owner、kind=`SIGNAL_SET`、artifact ref、content hash、blob size、Run/Snapshot/RulePack/input Artifact lineage 及实际共享 payload。任一侧缺失或只改单侧均失败关闭；不得只返回 metadata-only repository 的自洽副本。 |
| Lineage 查询 | Artifact 与 SignalSet lineage 均以领域 payload 中的规范顺序为权威，并与 PostgreSQL ordinal edges 精确相等；分页 cursor 使用 AEAD，绑定 active principal、tenant/owner、resource id、content hash、lineage kind、page size 与位置。cursor 跨用户、跨 owner、跨资源、跨版本或内容漂移必须拒绝。 |
| 幂等与重放 | 既有 `publish_verified_blob` / `publish_signal_set` 重放必须读取并精确比较持久化业务行、blob reference 与 lineage，返回 persisted value；禁止像当前 Artifact repository 一样在 idempotency replay 时直接回显 caller value。key 相同但任何字段漂移必须冲突且零部分写入。 |
| 角色边界 | Artifact/SignalSet 属于 Researcher 研究输出面。读操作要求单一 active role `RESEARCHER`、`artifacts:read` scope 与 exact tenant/owner；Platform Admin 不因 admin role 自动成为研究输出 superuser，同一 Human 如需研究读取应使用 Researcher session。发布继续继承产生它的受信 use case 与执行身份，不新增 caller-supplied actor/role。 |
| 契约诚实性 | ArtifactService 只保留本轮可以通过生产服务诚实完成的方法；response 统一使用 success/error oneof。删除的旧公开方法、字段、enum number/name 按 protobuf 规则保留，Rust/Python/TypeScript consumer 同步迁移，不提供 metadata-only compatibility shim。 |
| 声明即生产可达 | fixed descriptor 的完整 service name 集合与唯一 production route builder 实际注册的 `NamedService::NAME` 集合完全相等、无重复。门禁必须带两个真实反例：descriptor 多一个未组合 service、route 少一个 service，二者均 RED。仅在 API crate 构造 fake 或存在 tonic generated server 不算生产可达。 |
| 双传输与真实组合 | ArtifactService 在 `ficant-server` 真实 composition 中同时支持 native gRPC 与 gRPC-Web。生产 SIT 使用真实 PostgreSQL 与 Ceph-compatible BlobStore，经既有 server-owned publish 路径创建 Generic Artifact 和 SignalSet，随后覆盖 exact get、lineage page、重启重读及 metadata/edge/blob/bytes 篡改失败关闭。 |
| gRPC-Web 收敛 | 删除 `serve_grpc_web_with_rates_and_...` 的累加函数梯子和手写嵌套 service router；生产只保留一个以 `tonic::service::RoutesBuilder` 或等价单一注册点构造的 route set。focused tests 可构造局部 `Routes`，但不得再导出或冒充另一套生产组合。 |
| `ficant-web` 清理 | 删除只调用 `ficant_bootstrap::entry(ServiceRole::Web)` 的 Rust binary、Web bootstrap config 与 Compose/service/image 条目。`ficant-ui` 保持唯一静态 Platform Shell，并继续把 `/ficant-api/` 直接反代 `ficant-server`。Cargo metadata、许可证 inventory、供应链锁、Compose/security gate、release matrix、health/smoke 脚本和事实文档同步到实际包/镜像集合。 |
| 确定性与回归 | fixed Buf 双临时生成树、descriptor 与三语言 consumer、Artifact/Signal/Phase 1/Phase 4、Rates required-read、R6A production input plane、migration、许可证/供应链和三个统一本地入口全部转绿；不得降低 hash、owner、lineage、idempotency、错误、恢复、expected 或容差判据。 |

RED-first 子循环拟按以下顺序执行；首次真实非零命令、exit code 与首错必须在执行时立即留存，不能在最终绿灯后补造：

1. **contract RED：** descriptor 先要求诚实 Artifact query/error envelope、删除虚假 publish 方法和 orphan kind；Rust/Python/TypeScript 旧 consumer 必须先真实失败。
2. **verified-read RED：** 先证明现有 metadata-only get、SQL column/lineage tamper 与 Artifact replay caller echo 可通过，再收敛 Application/Storage proof。
3. **API RED：** 先建立 role/scope/owner、Artifact↔SignalSet、cursor 与 integrity error 的负向矩阵，再实现 Artifact gRPC adapter。
4. **topology RED：** 先用 descriptor-vs-route 反例证明当前 14 个声明 service 只有 13 个进入 R6A production route，再改为唯一 route builder 并组合 ArtifactService。
5. **orphan RED：** 先用 cargo metadata、Compose、release/security gate 证明 `ficant-web` 仍被当作应用包和镜像，但其运行体只有健康检查；再删除并同步所有机械绑定。
6. **production RED：** 先证明 Artifact RPC 在真实 server 返回 UNIMPLEMENTED，再完成 native/gRPC-Web、PostgreSQL/Ceph、重启和篡改 SIT。
7. **regression RED：** 恢复 Phase 1/4、Rates、R6A、供应链与统一入口；禁止通过保留第二 route、跳过 integration 或放宽 expected 消除失败。

## 3. 非目标

- 不接入、生成、托管或修改组合投研 WebApp；不新增 React 页面、App Registry entry、Excel 导入、Playwright journey 或业务导出。
- 不接入或修改 `cogawork` / COGA Core，不建设 COGA Instance、Domain Harness、React recipe、coding-agent/Git adapter 或 descriptor-lock 工厂流程。
- 不建设通用外部 Artifact 上传协议、presigned URL、对象下载 API、client-streaming 或任意 URI import。若 Human 不接受 §5 D1 的 server-owned publish 推荐项，这些能力必须重新拆轮并冻结安全、配额、重放和 owner 语义，不能塞入当前边界。
- 不新增 Portfolio/Book、交易、现金、负债、NAV/P&L、Benchmark、归因、VaR、穿透、模拟组合、优化器或正式 PMS/OMS/会计能力。
- 不实施 AC04、AC11–AC13、AC30–AC33、跨 clang 数值裁决、完整 outbox/crash recovery、灾备、MANUAL 全量重取证或 domain crate 大拆分；这些仍属于 R7A/R7B。
- 不补 FundingRulePack Decimal Oracle、DataHealth property suite、平行 convention enum 或拆分 `rates.rs`；这些是已记录但与 R6B 单一结果无依赖的债务。
- 不实现 DMQuant、Policy/Constraint、完整 DataHealth 扩展、AI/GeneratedNode sandbox 或 Python node runtime；继续顺延至 v0.2。
- 不修改 private authority、公共根目录 ignored authority、本地未跟踪审计报告、Golden/Oracle/expected/容差、C/C++/FFI 数值实现或 domain packs。
- 不改变 GitHub branch protection、CODEOWNERS、审批/status checks、Dependabot、secret scanning、push protection、commit signing 或 Release 策略；不创建 version/tag、推送镜像、部署或触发远端 CI/CD。删除 `ficant-web` 后对仓库内 workflow/Compose/package matrix 的机械同步不等于治理策略变更。

## 4. 公共契约变化

以下是待 §5 批准的推荐 contract shape。v0.1 前继续采用破坏性诚实收敛，不为从未生产可达的 metadata-only Artifact publish 契约提供 shim。

**ArtifactService：**

- 删除 `PublishArtifact`、`PublishSignalSet` 两个公共 RPC 及其 request/response message。正式发布继续由现有 server-owned Analytics/ResearchGraph/Worker use case 在 blob verify/promote 后调用 `PublishArtifact` / `PublishSignalSet` Application command；删除公共 RPC 不删除内部 command、repository 或原子执行路径。
- 保留 `GetArtifact`、`GetSignalSet`、`ReadArtifactLineage`、`ReadSignalSetLineage` 四个 RPC。所有 response 改为 `oneof result { success; ficant.core.v1.ErrorDetail error; }`；lineage success 使用新的 `LineagePage { repeated LineageRef lineage; PageResponse page; }`，不返回半页加独立错误。
- `GetArtifact` / `GetSignalSet` 成功只返回 metadata，不返回原始 payload bytes、object key、credential 或 presigned URL；成功语义保证服务端已完成 required verified read。未来真正需要浏览/下载大对象时另行冻结内容交付契约。
- 请求不接受 actor、role、owner claim、blob path 或 content bytes。owner/tenant 从成功解析出的 metadata 与逐请求 principal 交叉验证。

**ArtifactKind 与 SignalSet：**

- `ARTIFACT_KIND_GENERIC=1` 与 `ARTIFACT_KIND_SIGNAL_SET=5` 保留。当前没有任何生产构造路径的 `CURVE_SNAPSHOT=2`、`DATA_SNAPSHOT=3`、`UNIVERSE_SNAPSHOT=4` 删除并同时 reserve number/name；Curve/Data/Universe 继续由各自 Fact/Snapshot authority 管理，不在 Artifact table 制造第二身份。
- `SignalSet` 公共字段保持不变；服务端必须通过独立 Artifact、Run、Snapshot、RulePack、input Artifact 与 blob proof 后返回。

**统一错误与分页：**

- 缺少资源返回 `NOT_FOUND`；principal/role/scope/owner 越界返回 `FORBIDDEN`；SQL/payload/lineage 结构漂移返回 `IMMUTABLE_VIOLATION` 或 `LINEAGE_INCOMPLETE`；blob 缺失、hash/size/bytes 漂移返回 `HASH_MISMATCH`；存储不确定性返回 retryable `STORAGE_UNAVAILABLE`。
- lineage cursor 由现有 AEAD cursor codec 生成，不暴露 offset、数据库 key 或 owner；任何 scope/resource/hash 漂移失败关闭。

公共 Protobuf 删除项必须由 descriptor test 精确见证，生成物只来自 fixed Buf；不得手改 Rust/Python/TypeScript generated source。

## 5. 需 Human 决策

Human 已于 2026-08-19 批准下列 D1–D6，实施不得自行改选。任何语义、公共契约、路径或受保护事实变更都必须在首次相关写入前停止并取得新的 Human 明确授权。

| 决策 | Human 批准选择 | 冻结的排除边界 |
|---|---|---|
| **D1 · Artifact 发布边界** | **server-owned publish**：删除公共 metadata-only `PublishArtifact` / `PublishSignalSet`；保留并硬化 Rates、ResearchGraph、Worker 已有 verified publish command。 | 若要求外部 caller 发布，必须另行设计多步 staged upload/capability、chunk 幂等、配额、owner 隔离和 gRPC-Web 兼容；当前 request 绝不可直接接 repository。 |
| **D2 · ArtifactKind 收敛** | 删除并 reserve 从无生产构造者的 snapshot kinds 2–4；Snapshot/Fact 是唯一权威。 | 若保留，必须为每个 kind 定义唯一 authority、构造者、读写/迁移和与 Snapshot identity 的无歧义关系，显著扩大本轮。 |
| **D3 · 查询交付范围** | 本轮只做 verified metadata + lineage，不交付 payload bytes/download URL。 | 若要内容下载，需要独立的大对象交付、安全 header、范围读取、限流和浏览器契约，不能以 unary bytes 临时实现。 |
| **D4 · `ficant-web` 处置** | 删除冗余 Rust binary/Compose/image，保留 `ficant-ui` 直接反代 `ficant-server`；一方绑定从 19 Cargo + Python SDK 共 20，机械收敛为 18 Cargo + Python SDK 共 19。 | 若保留，必须给它唯一且被实际消费的运行职责；不得继续只返回 health/readiness，也不得引入第二业务后台。 |
| **D5 · production route 与门禁** | 用单一 `RoutesBuilder` 注册全部服务；同一注册过程产出实际 `NamedService` inventory，fast gate 与 descriptor 精确比较并带缺失/额外反例。 | 禁止维护手写平行清单或继续增加 `serve_grpc_web_with_*` 函数；替代方案必须同样从实际 route 证明可达。 |
| **D6 · R6A authority 后绑定** | 已由 private authority PR #21 rebase merge 为 `368a90cbf27cb42667d38e289fda1cc657013a17`，manifest 精确绑定公共 `0ccd079f8a575b82b3f107e506e3d8e6dcc215f6`，AC37 已独立点亮；R6B 本身不再修改 authority。 | 禁止把 R6B 的无新增 AC 实现写成另一项 authority 点亮，也禁止实施期间再次修改 private authority。 |

角色分离、owner/tenant、immutable Artifact、完整血缘、服务端 verified publish 和 R7B 恢复边界来自既有 authority/ADR，不在本轮重新选择。R6B 不新增第三角色，也不让 Platform Admin 成为隐式研究 superuser。

## 6. 最终真实测试证据

**准备与授权事实（2026-08-19）：** 公共 PR #60 已于 `2026-08-19T04:01:58Z` rebase merge；`origin/main` 为 `0ccd079f8a575b82b3f107e506e3d8e6dcc215f6`，其 Git tree `1dee0ad5eca1a2f5347690d791961d43ef0124a9` 与 R6A 已取证候选 `472ff04ec606f5baa7b6ef9e4f20ddd7abb4d867` 的 tree 完全相同。远端 R6A 分支已删除。Human 随后批准 D1–D6；private authority PR #21 于 `2026-08-19T04:38:55Z` rebase merge，`main` / `origin/main` / `HEAD` 为 `368a90cbf27cb42667d38e289fda1cc657013a17`，authority verifier 确认三份私有文档与 manifest 精确绑定公共 `0ccd079f8a575b82b3f107e506e3d8e6dcc215f6`，AC37 已独立点亮。

**只读差距核验：** descriptor 声明 14 个公共 service；`serve_grpc_web_with_r6a_input_plane` 实际组合其余 13 个，`ArtifactService` 没有 API adapter 或生产 route。现有 proto `PublishArtifactRequest` 只有 idempotency key 与 caller-supplied Artifact metadata，但 Application `PublishArtifact` 强制要求 `VerifiedBlobRef`；required blob reader 又必须先从正式 Artifact metadata 行解析引用，因此当前 RPC 无法在首次发布时诚实构造验证证明。Artifact repository 的 idempotency replay 当前直接返回 caller clone，metadata read 没有逐项复核 SQL fields/lineage edges。`ficant-web` Cargo package 的 main 只调用 `ficant_bootstrap::entry(ServiceRole::Web)`；静态 `ficant-ui` 已将 `/ficant-api/` 直接代理到 `ficant-server`。当前 cargo metadata 为 19 个 package，许可证策略另加 Python SDK，共 20 个一方绑定。

**本次规划允许写路径（冻结闭集）：**

- `docs/iterations/2026-08-r6b-artifact-topology.md`（新建）
- `docs/iterations/README.md`（只更新当前迭代指针）

除上述两项外，本次公共准备没有修改代码、契约、生成物、migration、审计报告、CI/CD、部署、远端 GitHub 设置或版本状态。R6A authority 后绑定是 D6 要求的独立 Human 前置动作，不属于 R6B 公共写路径；该动作完成后，R6B 实施继续禁止修改 private authority。

**R6B execution base：** public `0ccd079f8a575b82b3f107e506e3d8e6dcc215f6`，authority `368a90cbf27cb42667d38e289fda1cc657013a17`。§5 已获 Human 明确批准，以下实施闭集现已生效：

- `Cargo.lock`、`cicd.yml`
- `interface/proto/ficant/research/v1/artifact.proto`
- `crates/ficant-contracts/src/generated/**`
- `python/node-contracts/src/ficant_contracts/generated/**`
- `web-dm/packages/contracts-generated/src/**`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`、`r5d_layer_dependencies.rs`
- `python/tests/test_contract_import.py`
- `web-dm/platform-shell/tests/contracts-consumer.test.ts`
- `crates/ficant-domain/src/research/artifact.rs`、`signal_set.rs`
- `crates/ficant-application/src/lib.rs`、`ports/artifacts.rs`、`ports/signals.rs`、`ports/fingerprint.rs`、`ports/mod.rs`
- `crates/ficant-application/src/use_cases/verified_reads.rs`、`use_cases/mod.rs`
- `crates/ficant-application/tests/required_verified_reads.rs`、`review_round5.rs`
- `crates/ficant-storage/src/postgres/artifacts.rs`、`signals.rs`、`codec.rs`、`common.rs`、`mod.rs`
- `crates/ficant-storage/src/s3/staging.rs`
- `crates/ficant-storage/tests/postgres_repository.rs`、`migration_acceptance.rs`、`phase4_execution_sit.rs`
- `migrations/postgresql/0024_r6b_artifact_topology.sql`（新建）
- `crates/ficant-api/src/artifact.rs`（新建）、`core_error.rs`、`grpc_web.rs`、`lib.rs`
- `crates/ficant-api/tests/artifact_service.rs`（新建）、`grpc_web_boundary.rs`、`phase2e_sdk_live.rs`
- `binaries/ficant-server/src/lib.rs`、`binaries/ficant-server/Cargo.toml`
- `binaries/ficant-server/tests/composition.rs`、`data_source_registry_sit.rs`、`r6a_governed_input_sit.rs`、`service_topology.rs`（新建）、`r6b_artifact_service_sit.rs`（新建）
- `crates/ficant-acceptance/tests/phase1_business_loop.rs`、`negative_invariants.rs`
- `binaries/ficant-worker/src/production.rs`、`binaries/ficant-worker/src/tests.rs`、`binaries/ficant-worker/tests/phase4_worker_sit.rs`
- `binaries/ficant-bootstrap/src/lib.rs`
- `binaries/ficant-web/**`（删除）
- `deploy/dev/config/ficant.toml`、`deploy/dev/docker-compose.yml`
- `deploy/test/config/ficant.toml`、`deploy/test/compose.test.yml`、`deploy/test/env.example`、`deploy/test/validate_release.py`
- `deploy/test/bin/deploy.sh`、`healthcheck.sh`、`smoke-test.sh`
- `.github/workflows/release-test.yml`
- `.github/scripts/compose_security_gate.py`、`tests/test_compose_security_gate.py`
- `.github/scripts/license-inventory.lock.json`、`supply-chain.lock.json`
- `.github/scripts/verify-repo-policy.sh`、`verify-reproducibility.sh`
- `scripts/check-fast.ps1`、`scripts/check.ps1`、`scripts/check-release-candidate.ps1`
- `README.md`、`docs/development.md`、`docs/product/scope.md`、`docs/delivery/release-notes.md`
- `docs/architecture/layering-refactor.md`（只在 D1 批准后把“公共发布面”澄清为 server-owned publish + verified query）
- `docs/iterations/2026-08-r6b-artifact-topology.md`（实施期只追加本节最终真实证据与 §7 残余风险；不得改写 §1–§5、execution base、允许路径或受保护事实）

**禁止写路径：** 所有未逐项列出的路径。特别禁止 private authority、公共根目录 ignored authority、两份未跟踪审计报告、GitHub branch/security settings、其他 workflow、版本/tag、镜像推送、目标环境部署、WebApp/UI source、COGA、Rates/PortfolioRisk 业务实现、C/C++/FFI 数值代码、domain packs、Golden/Oracle/expected/容差以及 R7 recovery/MANUAL 路径。

**受保护事实：** R5D 五个 Rates 精确输入与 ResultMetadata、R5E TaxRulePack v2/双口径/Decimal Oracle、R6A principal/role/DataSource authorization/Definition/Fact/Snapshot/FoundationChange、canonical quote v1/schema/hash、KRD与税后 expected、Phase 1/4 execution/lease/fencing/checkpoint/recovery、Artifact Arrow schema/media type/hash、20个一方包在 R6B base 上的许可证事实、L1→L2结构门禁、现有 PostgreSQL/Ceph fail-closed 与错误语义保持不变。D4若批准，允许一方包数量因删除真实 orphan 从20机械变为19，但剩余每个包仍必须完整受控；不得为维持历史计数保留空包。

以下是最终候选必须执行的门禁；尚未执行的不得写成已通过：

- fixed Buf format/lint；两棵独立完整生成树逐路径/hash一致；descriptor 与 Rust/Python/TypeScript consumer 验证
- Artifact/Signal exact get、lineage pagination、role/scope/owner、replay 与 SQL/payload/edge/blob/bytes tamper 矩阵
- descriptor-vs-production routes gate 及“descriptor额外/route缺失”两个反例 fixture；该门禁进入 `check-fast.ps1`
- production ArtifactService native gRPC + gRPC-Web SIT，真实 PostgreSQL/Ceph、server-owned publish、重启重读与 integrity event
- Phase 1、Phase 4 Worker/lease/recovery、Rates required-read、R6A production input plane 回归
- migration 0024 正/负向及 legacy orphan kind fail-close；PostgreSQL 全套串行回归
- cargo metadata/package adjacency、许可证 inventory/绑定、supply-chain 与 Compose/release static policy，要求删除后 18 Cargo + Python SDK 共19个一方包完整受控
- `scripts/check-fast.ps1`、`scripts/check.ps1`、`scripts/check.ps1 -IncludeIntegration`
- `cargo fmt --all -- --check`、strict Clippy、`git diff --check`、最终允许路径闭集与受保护事实逐项核对

§5 批准并冻结 execution boundary 后，本节只追加同一最终候选上的真实 RED、命令、exit code 与可得 test count；不得把上述计划命令写成通过。无版本号，因此不运行 cicd skill、发布候选镜像入口、tag、部署或远端 CI/CD。

**实施期扩权与提交边界（2026-08-19）：** Human 在实现核验后明确批准额外修改 `crates/ficant-api/Cargo.toml`、`interface/buf.gen.yaml`、`.github/scripts/verify-contract-generation.sh`、`.github/scripts/verify-supply-chain.sh`，并批准把最终候选组织为两个本地提交、一次 GitHub 分支推送。前一提交固定实现与生成物，后一提交固定 contract-generation baseline 与本文最终证据；两份受保护未跟踪审计报告继续不读、不改、不暂存。

**真实 RED：** 首次以标准 workspace 配置执行 `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`，exit `1`，Rust workspace check 首先因 `tonic::service::{Routes, RoutesBuilder}` 未启用 `router` feature 而编译失败；启用正式 feature 后不再依赖测试配置偶然点亮生产路由。首次执行 Python contract consumer，exit `2`，首错为 `ModuleNotFoundError: ficant_contracts.generated.ficant.research.v1.artifact_pb2_grpc`，证明 Artifact Python stub 尚未生成。首次执行 `.github/scripts/verify-supply-chain.sh`，exit `1`，先命中 supply lock 摘要漂移，刷新绑定后又以旧的 20 个一方包 expected 失败，证明删除 `ficant-web` 必须同步收敛为 19。首次执行最终 contract-generation gate 时两棵 fresh tree 摘要相同，但 tracked generated tree 摘要不同，exit `1`；门禁未把“fresh 两棵相同”误当作仓库生成物已同步。

**focused 与回归绿灯：** `cargo test --offline --locked -p ficant-api --test artifact_service` exit `0`，3 passed；`cargo test --offline --locked -p ficant-server --test service_topology` exit `0`，3 passed，其中 descriptor-extra 与 route-missing 两个反例均被拒绝；`cargo test --offline --locked -p ficant-contract-tests` exit `0`，descriptor 20 passed、layering 3 passed。完整 crate 回归分别为 Application 139 passed、Storage 串行 72 passed、API 67 passed/1 ignored、Server 21 passed/2 ignored。`python/tests/test_contract_import.py` exit `0`，1 passed；固定 Node `22.17.0`、pnpm `10.12.4` 的 TypeScript contract consumer 1 passed，Platform Shell 全套 35 passed。migration integration 7 passed，许可证 inventory/绑定返回 19 个一方包、648 个受控包的摘要 `8d985cb750c32718fa40e5546afe7849bb19046f230a74fce83cb5f2d35c47fc`；`.github/scripts/tests/run-gates-tests.sh` exit `0`，全部反例 fixture PASS。

**统一入口与生产证据：** `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1` exit `0`；`pwsh -NoProfile -NonInteractive -File scripts/check.ps1` exit `0`；`pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration` exit `0`。最后一条在真实 PostgreSQL 与 Ceph-compatible BlobStore 上执行 R6A governed input SIT 1 passed，以及 R6B Artifact production SIT 1 passed；后者通过同一生产 composition 覆盖 native gRPC、gRPC-Web、server-owned Generic Artifact/SignalSet 发布、重启重读、规范 lineage 分页及 SQL/edge/blob/bytes 篡改失败关闭。所有刻意篡改只产生安全 `storage.published_content_integrity_failure` 事件并返回 typed error，没有部分成功。

**固定生成与供应链事实：** fixed Buf `1.56.0` 的 descriptor 为 `182841` bytes，SHA-256 `93445a41c2ca01f03f37aa1e172fa72ca58272ff3375c736d55e9c646c6a1749`；breaking baseline 固定到实现提交 `6c805930f201b3d82bbcbee9030b791e48fb08e7`。`.github/scripts/verify-contract-generation.sh` exit `0`，两棵独立 fresh tree 与 tracked Rust/Python/TypeScript 生成树逐路径、逐 bytes 相等，三语言 consumer 同次通过。Cargo metadata 已机械收敛为 18 个 Cargo package，连同 Python SDK 共 19 个一方包；`ficant-web` 不再出现在 workspace、Compose、release matrix、license inventory 或 supply-chain lock 中。`.github/scripts/verify-supply-chain.sh` exit `0`，输出 `/tmp/tmp.3fRzdBK3Hi`：82 个已发布历史提交中的 3 条 `generic-api-key` 测试字面量误报以 commit/path/rule/line 精确锁定，候选两提交与最终 release tree 均为 0 findings；648 包 SBOM、Cargo all-feature/all-target 可达图、Cargo/PyPI/npm 固定离线漏洞库、许可证与 provenance 全部通过。最终候选没有版本号，未创建 tag、镜像或部署。

## 7. 残余风险

- 推荐的 D1 会诚实删除从未生产可达的 generic caller publish，但也明确留下“外部大 Artifact 如何进入平台”的产品缺口。当前正式产物由 FICANT 自身 Analytics/ResearchGraph/Worker 产生；未来若确需第三方产物导入，必须独立设计 staging capability、配额、malware/content policy、owner隔离、重放与浏览器传输，不能复活 metadata-only RPC。
- 本轮 query 成功会证明 payload 完整，但只返回 metadata/lineage，不提供任意大对象下载。Phase 5A 已有受限、typed node-output读取，不能外推为通用 Artifact 浏览或下载能力。
- PostgreSQL metadata/lineage 与 Ceph blob 仍不能由一个事务覆盖。R6B 可以强化读侧验证和既有发布重放，但完整 outbox、crash recovery、orphan回收与灾备证据仍属于 R7B。
- 删除 `ficant-web` 会改变下一版本的应用镜像集合和本地 Compose 服务数。R6B 只做仓库内机械收敛与本地验证；真正版本镜像、Linux CI、扫描和测试环境交付仍须 Human 选定版本后由 CICD 闭合。
- topology gate 证明“声明的 service 已注册并可被 transport 到达”，不证明每个业务分支都正确；Artifact focused/production SIT 与既有每服务测试仍是必需的独立证据。
- private authority 已把 AC37 独立绑定到合并后的 R6A；R6B 不新增 AC，也不再次修改 authority。未来若要把 Artifact 查询、外部上传或恢复语义升级为新的产品承诺，必须另行取得 Human authority，而不能从本轮拓扑闭合自动外推。
- 单一 production route builder 会消除大部分 `grpc_web.rs` 手工嵌套，但不会顺带拆分 `rates.rs`、`portfolio_risk.rs` 或其他大文件；这些维护性债务不得借 R6B 无行为见证地扩张。
- R6B 完成后，FICANT 仍不是组合投研系统/PMS，也仍缺 R7A/R7B 的 AC04、AC11–AC13、AC30–AC33 全量证据；不得把“服务拓扑闭合”宣传为一期发布完成。
