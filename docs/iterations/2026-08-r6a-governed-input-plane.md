# R6A 迭代 brief（实施前准备）— 受治理输入平面

**迭代：** R6A · **点亮目标：** AC37 · **planning base：** `e5c5f34c1418c87f3e97cf84e394617876ead5ff` · **authority base：** `dea6eafe4fcccf364fd19f00f23f7dde900e0513`

本 brief 是 R6A 面向 Human 的唯一设计与后续最终证据载体。它把 2026-08-13 Portfolio360 / COGA 讨论只作为一次外部需求压力测试：讨论确认 FICANT 需要先闭合受治理的后台输入面，但不授权接入该 WebApp、引入 COGA 或扩张为 PMS。2026-08-13 Human 已批准 §5 的 D1–D6 并授权启动；本文据此冻结公共契约、execution base、代码允许路径和受保护事实，按 RED-first 实施。

## 1. 目标

R6A 只交付一个平台结果：建立一个生产可达、角色不可绕过、输入可验证且变更可追责的受治理输入平面。平台管理员能够登记并版本化基础 Definition、DataSource、Fact 与 Snapshot 权威；研究用户只能通过平台管理员对“精确数据源版本 + 精确导入接口”的授权执行规范化数据导入。Definition、Fact、Snapshot 三个已声明但尚未组合的公共服务在真实生产 server 中闭合；每次基础数据变更记录服务端确定的 actor、active role、原因、来源证据和前后内容身份。

本轮是后续投研 WebApp 的后台前置条件，不是 Portfolio360 产品迭代。它不新增 Portfolio、Book、NAV、P&L、Benchmark、归因、VaR、模拟组合或 UI，也不让 COGA 进入 FICANT 的构建、部署或运行链。

**Acceptance sentence：**

> 给定同一 Human 可分别取得 Platform Admin 与 Researcher 两个 active-role session，系统必须在 scope 检查之外独立验证当前 active role，并从受信身份派生 actor、tenant 与 owner 边界。Platform Admin 可以用非空变更原因及带内容 hash 的来源证据，原子发布完整 MarketDefinition、注册/版本化 DataSource、授权精确 DataSource 版本在精确 import interface 上被研究用户使用，并通过生产组合的 MarketFactService 与 SnapshotService 发布受验证输入。Researcher 持有正确 scope 但使用未授权、不同版本、不同接口、不同 owner/tenant、过期或已撤销的数据源时，必须在打开 adapter、暂存 blob、调用数值逻辑或写入任何 repository 前失败关闭；公共 ErrorDetail 必须明确指出该 exact DataSource 未获平台管理员授权。授权成功时，server 从管理员登记的 connection binding 读取数据，机械执行 mapping、Calendar、Unit、schema 与双时间验证，发布 canonical immutable snapshot，并把 exact DataSource、authorization、mapping、Calendar、Unit、actor 与时间证据写入 fingerprint、lineage 和不可变变更日志。重启后可只依赖 FICANT 持久层读回相同 snapshot；Definition、Fact、Snapshot 的 native gRPC 与 gRPC-Web 生产 SIT 均可达；任何 identity/version/hash/content/time 漂移均不产生部分写入，AC37 成立，R5E 及既有输入血缘行为不回退。

## 2. 验收

| 条目 | R6A 可执行判据 |
|---|---|
| 受信 principal | 认证结果必须包含服务端配置的 `subject_id`、actor ULID、tenant、允许 owner、单一 active role 与 scopes；业务请求不得提交或覆盖这些字段。AccessScope 必须逐请求从 principal 派生，不能继续复用部署级固定 actor。缺失/未知 role、actor/tenant/owner 不一致或 caller claim 漂移均在 use case 前拒绝。 |
| 角色不可由 scope 替代 | Platform Admin 与 Researcher 是闭枚举且作用于 mutation authorization；正确 scope + 错误 role 必须拒绝。即使同一 Human 同时承担两种职责，每个 session 仍只有一个 active role，不能因同时具有 admin assignment 而让 researcher import 绕过白名单。读权限继续受 owner/tenant 与 scopes 约束。 |
| 基础写入矩阵 | Definition、DataSource registration/authorization、Subject/SubjectState、PositionSnapshot、DataHealth threshold、管理员直接 Fact/Curve/Universe 写入只允许 Platform Admin；Factor/ResearchGraph/Analytics 保持 Researcher 边界。研究用户的数据写入只经受控 import use case，不直接调用管理员 append RPC。 |
| 数据源授权 | “已登记”与“允许研究导入”是两个状态。授权对象必须绑定 tenant/owner、exact DataSource id/version/content hash、闭枚举 import interface、canonical schema identity、有效期和 forward-only 状态；管理员的更新/撤销产生新版本，不能改写历史授权。Snapshot lineage 必须绑定实际使用的授权版本。 |
| AC37 负向 | Researcher + 正确 scope 对未授权 source/interface 的调用返回 `FORBIDDEN`，typed detail 为 `DataSourceNotAuthorized`，`resource_ref` 精确指向 source id@version，message/field violation 明确为管理员未授权，而不是 adapter 技术故障。adapter read count、blob stage/promote、Fact/Snapshot/Audit repository mutation count全部为零。 |
| Canonical import 正向 | Researcher 只提交 exact source、authorization、mapping、Calendar、Unit、知识/观测窗口与目标 snapshot identity；不得提交 connector path、credential、connection string 或自称已验证的 payload/hash。server 通过管理员登记的 connection binding 选择 adapter，一次读取后规范化、排序、编码、hash、暂存、读回验证并原子发布。相同输入幂等重放得到相同 identity/fingerprint且不重复读取/写入。 |
| 完整 Definition | Instrument 与 Bond/Futures subtype 必须以一个完整 `DefinitionValue` 原子发布，不能先产生只有 Instrument、后补 subtype 的可见中间版本。Calendar、Unit、RulePack 与 Instrument 共用 exact get/as-of/list semantics；expected-latest、owner、effective/knowledge time 与 content hash 漂移失败关闭。 |
| 可用 Fact 面 | Quote、Trade、Cashflow、Valuation 与 CurveSnapshot 的 append/query/get 都通过 Application proof 验证 Definition、Unit、RulePack、DataSource 与时间；correction 使用显式 supersedes 语义而不是原地修改。研究 import 产生的事实/快照必须与授权来源一致；管理员 direct append 必须带变更理由。 |
| 可用 Snapshot 面 | DataSnapshot 发布改为 server-side verified import；UniverseSnapshot 由完整成员输入在 server 规范化并计算 hash，不能只接受调用方声明的 metadata/content hash。Get 必须验证数据库 metadata 与 blob payload/manifest 身份，并在外部 source 不可用时仍可读回。 |
| 基础变更留痕 | 每次管理员基础 mutation 与每次研究 import 都生成 append-only change record：server actor、active role、operation、resource exact ref、before/after hash、idempotency/fingerprint、server time、非空 reason、至少一个内容寻址 source reference，或对研究 import 精确引用管理员授权及其理由。业务写入与日志同一事务提交；失败不留孤儿日志，重放不重复。connector secrets不得进入日志。 |
| 生产组合 | `MarketDefinitionService`、`MarketFactService`、`SnapshotService` 在 `ficant-server` 真实 composition 中同时提供 native gRPC 与 gRPC-Web；使用真实 PostgreSQL、BlobStore/Ceph-compatible backend 和一个确定性 fixture adapter 完成 SIT。R6B 的 Artifact service 与通用“声明即生产可达”门禁不提前并入。 |
| 确定性与回归 | fixed Buf 双临时生成树、Rust/Python/TypeScript consumers、migration 正/负向、角色矩阵、allowlist 矩阵、Phase 3A/3B 数据源与 snapshot 回归、R5D/R5E Rates、统一本地入口全部转绿；不得降低既有 hash、双时间、owner、幂等、错误或线路隔离判据。 |

RED-first 子循环拟按以下顺序执行；首次真实非零命令、exit code 与首错必须在执行时立即留存，不能在最终绿灯后补造：

1. **principal/role RED：** 先证明当前 scope-only identity 与部署级固定 AccessScope 能让错误角色触达 mutation，再引入 active role 与逐请求 principal。
2. **authorization RED：** 先建立 AC37 “正确 scope、错误 source/interface、零副作用、明确错误”矩阵，再实现版本化 DataSource authorization。
3. **contract RED：** 先让 descriptor 与三个 consumer 要求原子 Definition、受控 import、correction、typed governance error 和 change evidence，旧契约真实失败后再修改 proto/生成物。
4. **application/storage RED：** 先覆盖原子写入/日志、幂等、rollback、时间/hash/owner漂移，再实现 use case、migration、repository 与 blob transaction choreography。
5. **production RED：** 先证明三个服务在当前 server route 不可达，再组合 API/production server；AC37 负向必须在真实 server 上保持 adapter/blob/repository 零调用。
6. **regression RED：** 恢复 Phase 3A/3B、Position、DataHealth、Factor、R5D/R5E 与统一入口；禁止通过放宽 expected、容差、错误断言或跳过 integration消除失败。

## 3. 非目标

- 不接入、生成、托管或修改 Portfolio360 WebApp；不新增任何 React 页面、App Registry entry、Playwright journey、Excel 上传或业务导出。
- 不接入或修改 `cogawork` / COGA Core，不建设 COGA Instance、Domain Harness、React recipe、coding-agent adapter、worktree/PR adapter 或 descriptor-lock 工厂流程。未来工厂可以消费 R6A/R6B 完成后的 public commit 与 descriptor，但不是运行依赖。
- 不新增 Portfolio、Book、PortfolioGroup、Benchmark、Mandate、交易、批次成本、现金、负债、NAV/P&L、收益率、归因、VaR、跟踪误差、穿透、模拟组合、优化器或 Black-Litterman。
- 不扩展美国国债、基金、广义人民币债、多币种、FX、指数或跨资产估值；不把 FICANT 变成正式 PMS、总账、会计分录、监管报表、OMS/EMS、报单、清算或结算系统。
- 不实现 ArtifactService、Artifact publish/query production composition、通用“声明即生产可达”门禁、dead gRPC-Web/`ficant-web` 清理；这些保留给 R6B。
- 不实现 AC04、AC11–AC13、AC30–AC33、跨 clang 裁决、完整恢复取证、DMQuant、Policy/Constraint、完整 DataHealth扩展、AI/GeneratedNode sandbox或完整 OIDC/组织目录。
- 不修改 private authority、公共根目录 ignored authority 副本、现有未跟踪 `docs/review/full-audit-2026-08-07.md`、CI/CD、远端 GitHub、安全设置、version/tag、镜像、部署或 branch。

## 4. 公共契约变化

以下 contract shape 已获 Human 批准。v0.1 前采用一次破坏性收敛，不为当前未生产组合的 Definition/Fact/Snapshot 旧写入面保留虚假兼容；删除的旧 field/tag 必须 `reserved`，生成消费者同步迁移。

**Session 与 principal：**

- 新增闭枚举 `PlatformRole`: `UNSPECIFIED=0`、`PLATFORM_ADMIN=1`、`RESEARCHER=2`；成功 session 拒绝 UNSPECIFIED。
- `Session` 保留 1–5，并加法新增 `actor_id=6`、`active_role=7`、`tenant_id=8`、`allowed_owner_ids=9`。同一 Human 可以有两个 role assignment，但必须分别建立 active-role session；业务方法不接受 caller-supplied role。
- 内部 `AuthorizedPrincipal` 固定为 subject、actor ULID、tenant、allowed owners、active role、scopes 与 credential fingerprint。每个 service 从它构造 AccessScope；删除生产 route 对单一部署级 actor/owner 的复用。
- mutation authorization 同时要求 active role、scope、tenant/owner；三个条件分别测试，任一失败不能触达 repository。

**Definition 与 Fact：**

- `MarketDefinition` 的 instrument branch 改为 `CompleteInstrumentDefinition { Instrument instrument=1; oneof subtype { Bond bond=2; FuturesContract futures_contract=3; } }`；`InstrumentKind.OTHER` 不携 subtype，Bond/Futures 必须携匹配且引用同一 exact instrument version。
- 将分离的六个 append RPC 收敛为单一 `AppendDefinition`。`AppendDefinitionRequest` 固定 `idempotency_key=1`、`expected_latest_version=2`、`definition=3`、`change=4`；response `oneof result { definition=1; error=2; }`。旧 append request/response message 和 service method 删除，不提供 shim。
- 保留 exact get、as-of resolve、list versions；返回成功或统一 `ErrorDetail`，不存在默认 latest 或 partial subtype。
- `MarketFactService` 收敛为 `AppendMarketFact`、`CorrectMarketFact`、`PublishCurveSnapshot`、`QueryInstrumentFacts`、`GetCurveSnapshot`。Append/Correct 固定携带一个 `MarketFact`、idempotency、change；correction另携带 original fact id并要求payload `supersedes_id`一致。Curve publish携带 canonical `CurvePointSet`，server计算content hash，不能只接收metadata。
- 研究用户不直接使用 append/correct；这些管理员入口与研究 import use case物理分离，避免“有 scope 即可绕过 authorization”。

**DataSource authorization 与 Snapshot import：**

- `RegisterDataSourceRequest` 加法新增 `change=4`。新增闭枚举 `ImportInterface`: `UNSPECIFIED=0`、`CANONICAL_QUOTE_SNAPSHOT=1`；`DataSourceAuthorization` 固定包含 authorization ref、owner、exact source ref及其content hash、exact immutable InstrumentMapping id及其content hash、interface、schema id/hash、effective_from/effective_to、state、supersedes ref、content hash。`DataSourceRegistryService` 新增 `PublishDataSourceAuthorization`、`GetDataSourceAuthorization`、`ListDataSourceAuthorizations`；撤销以新版本/替代版本前向生效，不原地更新。
- `SnapshotService` 删除 metadata-only `PublishDataSnapshot`，新增 `ImportCanonicalQuoteSnapshot`。请求固定携带 idempotency、target snapshot id、exact authorization、带ULID/content hash的`InstrumentMapping`、Calendar、Unit、as_of、visible_at与研究import reason；不得重复提交DataSource/connection binding，也不得携payload或claimed snapshot hash。server只能由authorization解析exact DataSource，再由其registry binding选择adapter。response是snapshot/error oneof。
- `PublishUniverseSnapshot` 接收 snapshot id、owner、完整 member refs、filter digest、lineage、idempotency与change；server排序、验证 Definition、编码并计算 content hash，调用方不能提交claimed content hash。
- Snapshot metadata/lineage 新增实际使用的 authorization 与 actor evidence；DataSource credential/connection detail始终只通过 registry binding解析，不进入公共响应。

**错误与变更证据：**

- Application 新增 typed `DataSourceNotAuthorized { data_source_ref, import_interface }`，API 映射为 `FORBIDDEN`，精确 source ref进入 `resource_ref`，字段错误明确为“not authorized by Platform Admin”；adapter自身连接失败继续使用独立 technical error。
- 公共治理 message 放入新 `ficant.core.v1.governance.proto`：`ChangeJustification { reason=1; repeated SourceDocumentRef sources=2; }`，`SourceDocumentRef { uri=1; sha256=2; }`，`FoundationChangeRecord { record_id=1; actor_id=2; active_role=3; operation=4; resource_ref=5; before_hash=6; after_hash=7; change=8; request_fingerprint=9; occurred_at=10; authorization_ref=11; }`。reason规范化非空且至少一个source；研究import可用authorization_ref继承管理员change，仍需非空import reason。
- 新增 append-only `FoundationChangeRecord`：record id、principal identity/role、operation、resource exact ref、before/after hash、justification/authorization ref、request fingerprint、server timestamp。它由 server生成，客户端不能提交 actor、role、record id或时间。
- 新增 `FoundationChangeService`（不扩大service inventory语义）：`GetFoundationChange`与`ListFoundationChanges`，只允许Platform Admin读取；list按resource/actor/time过滤并使用加密游标。不建设审批流、合规工作台或通用Policy engine。

## 5. 需 Human 决策

authority base 已冻结以下不可选边界：Platform Admin 与 Researcher 两种角色都必须存在且不能被 scopes 替代；基础数据写权属于 Platform Admin；Researcher 只能从管理员白名单内的来源/接口导入；越界必须失败关闭并明确指出未授权 source；基础变更必须记录原因与依据；单用户部署也不能合并权限。R6A 不新增 Approver/Auditor 等第三角色。

2026-08-13 Human 已明确“批准，并启动”，因此 D1–D6 全部按推荐项冻结：

| 决策 | 已批准选择 | 禁止漂移 |
|---|---|---|
| **D1 · active role** | 一个 session 恰好一个 active role；同一 Human 用两个独立 session/credential承担两种职责。principal携带服务端actor/tenant/owners，AccessScope逐请求派生。 | 禁止多active-role session、caller role或部署级固定actor。 |
| **D2 · 白名单粒度** | authorization精确绑定DataSource version/content hash + ImportInterface + canonical schema + owner/tenant + effective window；registration不等于authorization，撤销前向生效。 | 禁止只按source id、adapter kind或scope授权。 |
| **D3 · Definition写契约** | 破坏性删除分步subtype append，改为完整`MarketDefinition`原子append，不提供shim。 | 禁止可见半成品Instrument版本。 |
| **D4 · Snapshot入口** | DataSnapshot只允许server-side adapter ingest；Universe由server从完整成员集编码/hash。 | 禁止metadata-only publish、caller-supplied verified bytes/hash。 |
| **D5 · 变更日志** | 管理员基础写入与研究import都在业务提交中原子写append-only record；管理员可按exact resource/actor/time查询。 | 禁止只有应用日志、自由文本或异步best-effort留痕。 |
| **D6 · mutation矩阵** | Subject/State、PositionSnapshot、DataSource、Definition、DataHealth配置、direct Fact/Curve/Universe归admin；Factor/ResearchGraph/Analytics归researcher；researcher数据写只走allowlisted import。 | 禁止scope-only mutation授权。 |
| **D7 · 精确路径扩权** | 2026-08-18 Human 明确批准新增且仅新增：`crates/ficant-domain/src/subject.rs`、`crates/ficant-application/src/ports/subjects.rs`、`crates/ficant-storage/src/postgres/subjects.rs`、`crates/ficant-api/tests/phase2e_sdk_live.rs`、`binaries/ficant-worker/tests/phase4_worker_sit.rs`、`.github/scripts/license-inventory.lock.json`、`scripts/generate-cgb-interest-tax-pack.ps1`。前三项仅用于闭合 Subject/SubjectState exact owner、governed command 与业务+审计同事务；后四项仅用于修复既有真实门禁的身份夹具、Windows 文本规范化和一方包绑定。 | 禁止由此扩张到其他 domain/application/storage、CI/CD、authority、WebApp、远端或发布路径。 |
| **D8 · DataSource 哈希收敛扩权** | 2026-08-19 Human 明确批准且仅批准新增 `crates/ficant-application/src/use_cases/rates_materialization.rs`，只用于把既有 R5D DataSource content hash 委托给 domain 单一规范实现；保留公开函数签名、既有哈希字节和 R5D materialization 语义。 | 禁止修改 Rates 契约、输入物化、数值逻辑、证据语义、expected/容差，或继续扩张其他 R5D 路径。 |

任何 active-role语义、authorization最小键、原子Definition shape、server-side ingest、审计原子性、mutation矩阵、字段号、允许路径或受保护事实变更，都必须在首次相关写入前停止并取得Human明确扩权；不得边实现边改验收。

## 6. 最终真实测试证据

**准备阶段事实（2026-08-13）：** public `main`、`origin/main` 与 `HEAD` 均为 `e5c5f34c1418c87f3e97cf84e394617876ead5ff`；private authority `main`、`origin/main` 与 `HEAD` 均为 `dea6eafe4fcccf364fd19f00f23f7dde900e0513`，其 manifest 精确绑定上述 public commit。R5D 与 R5E brief 已记录最终真实证据，故外部讨论中“R5D最终结果仍未填写”的判断已过期。当前 public worktree唯一既有项是未跟踪 `docs/review/full-audit-2026-08-07.md`，本文不读取、修改、暂存或删除它。

**只读差距核验：** Definition、Fact、Snapshot protobuf声明、Application ports/repositories与PostgreSQL实现已经存在；`ficant-api` 与生产 server 尚无对应三个service adapter/composition。当前认证只有subject + scopes，没有role；生产service复用部署级AccessScope。DataSource registry只有“登记”而没有研究导入authorization。当前 DataSnapshot publish请求只有metadata，没有payload、upload token或source import instruction；但Application发布路径要求canonical payload + manifest bytes并执行stage/read-back/promote。以上是规划输入，不是R6A通过证据。

**本次规划允许写路径（冻结闭集）：**

- `docs/iterations/2026-08-r6a-governed-input-plane.md`（新建）
- `docs/iterations/README.md`（只更新当前迭代指针）

除上述两项外不得在准备阶段修改任何代码、契约、生成物、migration、测试、authority、CI/CD、部署或远端状态。`docs/architecture/layering-refactor.md` 已准确把角色/白名单/Definition/Fact/Snapshot安排在R6A，因此本次不复制或改写路线。

**R6A execution base：** public `e5c5f34c1418c87f3e97cf84e394617876ead5ff`，authority `dea6eafe4fcccf364fd19f00f23f7dde900e0513`。现有未跟踪 `docs/review/full-audit-2026-08-07.md` 属于Human，始终候选外。

**R6A实施允许写路径（冻结闭集）：**

- `Cargo.toml`、`Cargo.lock`
- `interface/buf.gen.yaml`
- `interface/proto/ficant/app/v1/session.proto`
- `interface/proto/ficant/core/v1/governance.proto`（新建）
- `interface/proto/ficant/core/v1/subject.proto`、`interface/proto/ficant/core/v1/subject_state.proto`
- `interface/proto/ficant/market/v1/data_source.proto`、`definition.proto`、`fact.proto`
- `interface/proto/ficant/research/v1/snapshot.proto`、`position.proto`、`health.proto`
- `crates/ficant-contracts/src/generated/**`
- `python/node-contracts/src/ficant_contracts/generated/**`
- `web-dm/packages/contracts-generated/src/**`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `python/tests/test_contract_import.py`
- `web-dm/platform-shell/tests/contracts-consumer.test.ts`
- `crates/ficant-domain/src/market/data_source_authorization.rs`（新建）、`crates/ficant-domain/src/market/mod.rs`
- `crates/ficant-domain/src/governance.rs`（新建）、`crates/ficant-domain/src/lib.rs`
- `crates/ficant-application/src/error.rs`、`lib.rs`
- `crates/ficant-application/src/ports/access.rs`、`data_sources.rs`、`definitions.rs`、`facts.rs`、`snapshots.rs`、`fingerprint.rs`、`mod.rs`
- `crates/ficant-application/src/ports/governance.rs`（新建）、`ingestion.rs`（新建）
- `crates/ficant-application/src/use_cases/data_sources.rs`、`data_snapshot.rs`、`position_views.rs`、`data_health.rs`、`mod.rs`
- `crates/ficant-application/src/use_cases/governed_inputs.rs`（新建）、`canonical_import.rs`（新建）
- `crates/ficant-application/tests/access_scope.rs`、`definition_aggregate.rs`、`data_source_port.rs`、`position_snapshot_contracts.rs`、`r5c_data_health_contracts.rs`
- `crates/ficant-application/tests/r6a_role_authorization.rs`（新建）、`r6a_governed_inputs.rs`（新建）
- `crates/ficant-data/src/canonical.rs`、`mapping.rs`、`snapshot.rs`、`source.rs`、`lib.rs`
- `crates/ficant-data/src/catalog.rs`（新建）、`crates/ficant-data/src/governed_import.rs`（新建）
- `crates/ficant-data/tests/canonical_ingestion.rs`、`dual_source_sit.rs`、`snapshot_codec.rs`、`snapshot_publication_sit.rs`
- `crates/ficant-data/tests/r6a_authorized_import.rs`（新建）
- `migrations/postgresql/0023_r6a_governed_input_plane.sql`（新建）
- `crates/ficant-storage/src/postgres/data_sources.rs`、`definitions.rs`、`facts.rs`、`snapshots.rs`、`codec.rs`、`mod.rs`
- `crates/ficant-storage/src/postgres/governance.rs`（新建）、`ingestion.rs`（新建）
- `crates/ficant-storage/tests/migration_acceptance.rs`、`data_source_registry_sit.rs`、`postgres_repository.rs`
- `crates/ficant-storage/tests/r6a_governed_input_postgres.rs`（新建）
- `crates/ficant-api/src/session.rs`、`registry.rs`、`grpc_web.rs`、`core_error.rs`、`lib.rs`
- `crates/ficant-api/src/data_source_registry.rs`、`subject_registry.rs`、`position_snapshot.rs`、`data_health.rs`、`factor_registry.rs`、`experiment.rs`、`rates.rs`、`portfolio_risk.rs`
- `crates/ficant-api/src/market_definition.rs`（新建）、`market_fact.rs`（新建）、`snapshot.rs`（新建）、`governance.rs`（新建）
- `crates/ficant-api/tests/platform_service.rs`、`grpc_web_boundary.rs`、`core_business_error.rs`、`data_source_registry_service.rs`、`factor_registry_service.rs`、`position_snapshot_service.rs`、`data_health_service.rs`、`rates_service.rs`
- `crates/ficant-api/tests/r6a_role_matrix.rs`（新建）、`market_definition_service.rs`（新建）、`market_fact_service.rs`（新建）、`snapshot_service.rs`（新建）、`governance_service.rs`（新建）
- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/composition.rs`、`data_source_registry_sit.rs`、`position_snapshot_sit.rs`、`data_health_sit.rs`、`factor_registry_sit.rs`、`rates_sit.rs`、`portfolio_risk_sit.rs`
- `binaries/ficant-server/tests/r6a_governed_input_sit.rs`（新建）
- `scripts/check-fast.ps1`、`scripts/check.ps1`
- `README.md`、`docs/product/scope.md`、`docs/development.md`
- `docs/iterations/2026-08-r6a-governed-input-plane.md`（实施期只追加本节最终真实证据与§7残余风险；本次授权冻结后不得改写§1–§5、execution base、允许路径或受保护事实）

**禁止写路径：** 所有未逐项列出的路径。特别禁止private authority、公共根目录ignored authority、未跟踪审计报告、`.github/**`、`cicd.yml`、`deploy/**`、WebApp/UI源码、`cogawork`、Artifact service实现、C/C++/FFI/native数值实现、domain packs、既有Oracle/Golden/expected/容差与版本/远端状态。

**受保护事实：** R5D/R5E五个Rates精确输入契约、TaxRulePack v2及权威payload、KRD与税后Decimal Oracle、Phase2C/2D native公式和expected、Arrow Artifact schema/hash、20个一方包许可证策略、L1→L2结构门禁、现有DataSnapshot canonical schema/Parquet writer binding保持不变。R6A可加authorization与actor lineage，但不得改变canonical quote业务列、单位/Calendar验证、市场数值或既有ResultMetadata语义。

以下是最终候选必须执行的门禁；尚未执行的不得写成已通过：

- fixed Buf format/lint；两棵独立完整生成树逐路径/hash一致；descriptor与Rust/Python/TypeScriptconsumer验证
- principal/active-role/scope/owner矩阵；scope-only绕过与部署级actor复用的真实负向
- AC37 exact source/interface/version/owner/effective/revocation矩阵，验证adapter/blob/repository/audit零副作用
- Definition原子append/as-of/list；Fact append/correct/query/curve；Snapshot canonical import/universe/get契约与Application测试
- PostgreSQL migration正反向、idempotency/rollback、变更日志同事务与跨重启验证
- DataSource file/PostgreSQL deterministic fixtures；Phase3A/3B既有dual-source、codec、publication/restart回归
- 三个service的production native gRPC + gRPC-Web SIT，真实PostgreSQL与Ceph-compatible BlobStore
- Subject/State、PositionSnapshot、DataHealth、Factor角色回归；R5D/R5E Rates与现有完整性门禁
- `scripts/check-fast.ps1`、`scripts/check.ps1`、`scripts/check.ps1 -IncludeIntegration`
- `git diff --check`、最终允许路径闭集与受保护事实逐项核对

§5批准并冻结execution boundary后，本节只追加同一最终候选上的真实命令、exit code与可得test count；不得用计划命令、并行脏树结果或事后重跑冒充首个RED。无版本号，因此不运行cicd skill、发布候选镜像、tag、部署或远端GitHub操作。

**实施候选与后续精确扩权（2026-08-19）：** R6A 在上述 execution base 上完成。除 §5 D7 已记录的七个路径外，Human 于 2026-08-19 再次明确批准且仅批准 `crates/ficant-storage/tests/position_snapshot_postgres.rs`，用于把既有 PostgreSQL Position 测试夹具迁移到 R6A 的 exact tenant/owner Subject 结构；实现没有放宽 migration、domain 或 repository 的 owner 约束。Brooks-Lint 审计复核期间，Human 又明确批准 §5 D8 的单一路径，只允许把 R5D DataSource hash 委托给 domain 规范实现。D1–D7、原实施闭集、private authority、远端、CI/CD、版本与部署边界均未改动。

**留存的首个真实 RED：**

- Contract 子循环首先执行 `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory descriptor_inventory_is_unique_and_preserves_phase1_semantics -- --exact --nocapture`，exit `101`，`0 passed / 1 failed`；首个真实不一致是旧 Subject descriptor 没有 R6A 要求的 owner 字段。该失败发生在修改 proto/生成物之前。
- 最终 Storage 候选首次全套执行 `cargo test --offline --locked -p ficant-storage --tests -- --test-threads=1`，exit `1`；`position_snapshot_postgres` 的两个用例均在旧夹具插入 Subject 时被 PostgreSQL `23502`（`tenant_id` 为 null）拒绝。Human 扩权后仅修正夹具的 identity/tenant/owner 写入；同一命令最终 exit `0`，聚合 `69 passed / 0 failed / 0 ignored`。
- 审计 owner-drift 子循环首次执行 `cargo test --offline --locked -p ficant-storage --test r6a_governed_input_postgres governed_import_owner_drift_returns_typed_exact_source_error -- --exact --test-threads=1`，exit `1`，`0/1`；生产 PostgreSQL exact read 先返回通用 `Forbidden` 且没有 source detail，证明 Application 的 typed `DataSourceNotAuthorized` 分支不可达。修复后同一用例 exit `0`，且普通 exact/list 管理读取仍保持 scope 授权，不暴露跨 owner authorization payload。
- DataSource hash 黄金向量捕获首次用四个占位 expected 运行 `cargo test --offline --locked -p ficant-data --test r6a_authorized_import catalog_rejects_duplicate_admin_bindings_and_data_source_hash_matches_r5d -- --exact --nocapture`，exit `1`，`0/1`；这是一条为冻结既有字节而刻意制造的测试 RED，不是生产哈希漂移。输出的四个真实 SHA-256 随后原样冻结，禁止为收敛实现而改变 expected。
- 其余子循环的早期失败有并行中间态或错误夹具成分，现有记录不足以诚实区分并复原一个精确的首条产品 RED 命令，因此本节不事后补造 exit code。最终负向矩阵仍由 application、API、PostgreSQL 与 production SIT 明确证明错误 role、scope、owner、source/interface/version/hash/effective/revocation、replay/tamper 在 adapter/blob/repository/engine 前失败关闭。

**公共契约与确定性：** fixed Buf `1.56.0` 的 format/lint 均 exit `0`。最终公共契约分别生成到两棵全新临时树，每棵 `82` 个源文件，逐路径与逐字节一致，规范化 manifest SHA-256 均为 `b4d6a9d89d685d8d04ac88f23a505fb7ad7920cadd932a774cf6483d66837098`；仓库生成树也是相同的 `82` 个文件与相同摘要。descriptor 独立构建两次均为 `183602` bytes、SHA-256 `17874d82950236a6cbf6b3fba12839bf204b6e3b82525e43f3d76a8e4419ce08`。`cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory` exit `0`（`20/20`），`cargo test --offline --locked -p ficant-contract-tests --test r5d_layer_dependencies` exit `0`（`3/3`）；Python contract consumer `1/1`，固定 Node `22.17.0` + pnpm `10.12.4` 的 TypeScript focused consumer `1/1`。

**核心与生产闭合：**

- `cargo test --offline --locked -p ficant-domain --tests` exit `0`（`90 passed`）；`cargo test --offline --locked -p ficant-application --tests` exit `0`（`138 passed`）；`cargo test --offline --locked -p ficant-data --tests` exit `0`（`14 passed`）；`cargo test --offline --locked -p ficant-api --tests` exit `0`（`64 passed / 1 ignored`）。
- `cargo test --offline --locked -p ficant-storage --test position_snapshot_postgres -- --test-threads=1` exit `0`（`2/2`）；`cargo test --offline --locked -p ficant-storage --test r6a_governed_input_postgres -- --test-threads=1` exit `0`（审计修复后 `9/9`）；`cargo test --offline --locked -p ficant-storage --tests -- --test-threads=1` 在最终候选上 exit `0`，聚合 `66 passed / 0 failed / 0 ignored`。`cargo clippy --offline --locked -p ficant-storage --all-targets --no-deps -- -D warnings` exit `0`。
- Phase 2E live Python SDK parity exit `0`（`1/1`）；production Worker SIT exit `0`（`1/1`）。`binaries/ficant-server/tests/r6a_governed_input_sit.rs` 使用真实 PostgreSQL、Ceph-compatible BlobStore、native gRPC 与 gRPC-Web，exit `0`（`1/1`），同时覆盖三个新 input service 的生产路由与 AC37 pre-adapter fail-close。

**统一候选门禁：** 审计修复后的 `scripts/check-fast.ps1` exit `0`；`scripts/check.ps1` 首次因本机默认 Node 为 `v24.18.0`、而冻结版本为 `v22.17.0` 在任何产品测试前 exit `1`，prepend 仓库固定的本地 Node `22.17.0` 后 `scripts/check.ps1` exit `0`，`scripts/check.ps1 -IncludeIntegration` exit `0`，最终输出均为 `FICANT complete local checks passed`（fast 入口为 `FICANT fast local checks passed`）。最后一条集成入口包含 migration `6/6`、lease queue `1/1`、execution closure `3/3`、Worker `1/1`、Phase 1 loop `1/1`、negative invariants `13/13`、Carry/Delivery/Hedge SIT 各 `1/1`、Phase 3A registry/parity、Phase 3B codec/publication，以及 R6A production SIT `1/1`；R5D KRD Oracle `3/3`、R5E tax Oracle `13/13` 与 Web `35/35` 同时回归通过。审计时复现的 PostgreSQL `PoolTimedOut` 来自 Docker engine 未运行；启动本地 Docker Desktop 后上述两轮真实集成均通过，故未归类为产品回归。

**审计修复与供应链、文本、闭集：** DataSource authorization-binding hash 现只在 domain 有一个生产实现；Application port 直接重导出，R5D Rates 与 Data import 的既有公开函数仅委托该实现。`rg -n "ficant\\.rates\\.data-source\\.v1" crates binaries` 只得到 domain 生产实现与 production SIT 的独立见证各一处；四个可构造 source 状态的跨层黄金 SHA-256 精确不变，focused data `4/4`、R5D materialization `10/10`、domain/application/data all-target strict Clippy 均 exit `0`。`verify-license-inventory.py verify-bindings --require-first-party --require-native-lf` exit `0`，inventory 为 `645` 个包、`20` 个 first-party 绑定，最终 digest `8b7989ca913819402901140164ba0eab36c5b6ec1dda2c5e867a1eb475a4798b`；R5E RulePack 生成脚本的 canonical LF 检查 exit `0`。`cargo fmt --all -- --check` 与 `git diff --check` 均 exit `0`（仅 Git 的 LF→CRLF 提示，无 whitespace error）。最终只读闭集审计得到 `candidate_paths=147`、`violations=0`；两份 Human 审计报告均保持未跟踪、只读且候选外，检测为 `protected_audit_present=2`。`rg --files interface/crates interface/python interface/web-dm` 无输出且 exit `1`，确认没有遗留禁止目录生成物。

**完成判断：** R6A 的单一结果已经成立：Platform Admin/Researcher active role 不可由 scope 替代；Definition、Fact、Snapshot 与 FoundationChange 生产服务闭合；exact DataSource authorization、server-only adapter binding、canonical import、不可变 lineage/fingerprint、同事务业务/审计与幂等重放均有负向和真实持久层证据。AC37 在该本地实施候选上点亮；没有版本号，因此未创建 tag、镜像、部署或远端 CI/CD 运行。

## 7. 残余风险

- R6A闭合的是可信输入控制面，不是完整业务数据目录。即使AC37成立，Portfolio360仍缺Portfolio/Book、交易/现金/负债、NAV/P&L、历史收益、Benchmark、归因、VaR、穿透与业务UI；这些需要独立authority、产品模型和后续迭代，不能从R6A能力外推。
- 当前本地bootstrap identity模型不是企业OIDC/组织目录。R6A推荐的active-role principal可证明平台边界，但真实多租户身份生命周期、credential rotation、SCIM/SSO与集中密钥治理仍需独立安全设计。
- server-side ingest只能安全使用已实现且可确定重放的adapter。File NDJSON与PostgreSQL之外的数据源、复杂credential broker、流式/增量导入及第三方行情授权不在本轮；不得以通用connection string临时绕过registry。
- migration `0023` 对已有、但没有 tenant/owner 证据的 pre-R6A Subject/SubjectState 行刻意失败关闭，不猜测归属。升级这类数据库前，operator 必须通过独立、可审核的数据治理步骤建立 exact tenant/owner 映射后再迁移或重导；本轮只验证了拒绝与事务原子性。
- PostgreSQL 业务/审计提交与 BlobStore 仍不可能由单一数据库事务覆盖。R6A 已用 stage → verify/promote → governed metadata/change、补偿与故障注入避免可见半成品；完整 crash recovery、outbox/recovery marker 和灾难恢复取证仍属于 R7B。
- R6A只聚焦三个input service的生产可达性。Artifact与全descriptor拓扑一致性仍依赖R6B；在R6B完成前，不得宣传所有公开service均已生产闭合。
- ordinary Python contract suite 仍按环境条件 skip live-server case；独立 Phase 2E live SDK 门禁已在最终候选上 `1/1` 通过。后续不得把普通 suite 的 skip 单独解释为生产 parity 证据。
- FundingRulePack 当前有 exact selector 单测、Rates 物化回归与完整集成，但仍缺一个与生产实现公式隔离的 Decimal Oracle；DataHealth 也仍缺覆盖随机边界组合的独立 property suite。两项属于后续测试完整性债务，不影响 R6A 的 AC37 输入治理结论。
- Bond/analytics 的部分 convention enum 仍存在平行表达；是否收敛会影响公共/domain authority，必须在后续迭代取得 Human 决策，不能在本轮机械合并。
- 若干 R6A API、Application 与 PostgreSQL 文件已变得较大。当前没有发现未组合的 R6A service 或新增逻辑孤儿，但按职责拆分这些大文件仍是可维护性债务；不得为了行数在无行为见证时重构。
- 当前只是经过本地完整与集成门禁的实施候选，尚未形成版本/tag、不可变镜像、远端 CI 证据或部署状态；这些不在本轮授权内。
