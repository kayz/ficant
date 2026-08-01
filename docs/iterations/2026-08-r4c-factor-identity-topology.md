# R4c 迭代 brief — Factor 身份与拓扑

**迭代：** R4c · **本轮目标条目：** AC05 · **execution base：** `610392000726ec55ea591332adec512117e29bd9` · **authority base：** `6b57108048b348d22aa4f28689aea42f8fe65f48`

本 brief 是 R4c 面向 Human 的唯一设计。R4c 建立全局、不可变的 Factor 身份、敏感度口径和静态目标关系，使同一经济量能由债券、期货与稳定曲线节点引用同一个 `FactorId`，并可双向查询。R4d 依赖本轮，但才负责内部曲线冲击、重定价、逐仓位 KRD 与组合数值聚合。本文只冻结设计和执行边界，不构成实现、push、Pull Request、merge、authority binding 或发布授权。

## 1. 目标

建立平台全局可读的字符串 `FactorId`，命名严格遵循 ADR-0015 已批准的 `<市场>.<类别>.<经济量>.<期限>` 全小写点分形状，例如 `cn.gov.yield.10y`。每个 id 只可注册一个不可变 `FactorDefinition`；定义的 canonical content hash 覆盖其 FactorId、因子 UnitRef、bump、单/双边、曲线重建策略及二阶项策略。相同 id 的定义或口径有任何差异都失败关闭，不能以新版本、覆盖写或调用方自报来规避。

Factor 与对象的关系由独立、append-only 的 `FactorTargetBinding` 表达。target 只能是 exact owner + Instrument `VersionRef`，或已注册的稳定 `CurveNodeDefinition` 引用；曲线节点以跨快照的 `curve_family_id + tenor` 身份及内容 hash 注册，绝不能用某次 CurveSnapshotId 加 maturity date 充当全局身份。服务提供 target → Factors 与 Factor → targets 两个查询面。

**Acceptance sentence：**

> 注册内容哈希确定的 `cn.gov.yield.10y` FactorDefinition、一个稳定的中国国债收益率 10 年曲线节点定义，以及同 owner 的国债和 T Futures Instrument 版本；把三者绑定至该 FactorId 后，分别以任一 target 查询均返回完全相同的 id，以该 id 反查返回三项 exact target。以相同 FactorId 注册不同 UnitRef、bump、方向、重建或二阶策略，或以不存在 / 无权限 / 非 exact 的 Instrument、未注册或 hash 漂移的曲线节点建立 binding，均失败关闭且不产生部分关系。R4d 之外不产生权重、DV01、KRD、总敞口或其他 Exposure 数值；定价公式、Rates 分析请求、C++ / C ABI、RulePack、Golden、Oracle、canonical schema/hash、Phase 2C/2D matrix 与 allowlist 均不变。

## 2. 验收

| 条目 | R4c 可执行判据 |
|---|---|
| AC05 | 用 `FactorRegistryService` 注册 `cn.gov.yield.10y` 的唯一 immutable definition；将同一 id 精确绑定到一个 Bond Instrument version、一个 Futures Instrument version 与一项已注册 `CurveNodeDefinition`。三次 target → factor 查询都只返回该 exact id，factor → targets 查询按稳定 target key 排序后同时列出三项。重复相同 command 只作 idempotent replay；同 id 的任一 canonical 字段差异、target owner / version 漂移、未注册或 hash 不符 curve node、越权读取或绑定均 fail-closed，不返回 partial topology。 |

R4c 闸门：

1. 先只加入 FactorDefinition / CurveNodeDefinition / binding 的 domain 与 application 判据，并亲眼取得非零 exit code；RED 不构成 checkpoint。proto、PostgreSQL、API、server composition 或 generated output 不得早于该 RED。
2. `FactorId` 是全平台身份，不带 owner、tenant、市场分支或可变 alias；它必须满足 ADR-0015 的四段全小写点分命名。FactorDefinition 是单次写入的 immutable content-addressed object，不能 append version。相同 id 的 content hash 不同即失败，不能以 “最后写入者获胜”、覆盖、软删除或别名绕过。
3. `SensitivityConvention` 是 FactorDefinition 的一部分：bump 必须是带 UnitRef 的 positive Decimal，方向、曲线重建与二阶策略必须为显式非 UNSPECIFIED enum。R4c 不实现该 convention 的计算，但任何未被 hash 覆盖、缺失或不支持的口径都不得注册为可用 Factor。
4. 曲线节点是定义，不是某次 CurveSnapshot 中的行：`CurveNodeDefinition` 固定 `curve_family_id`、tenor、因子 UnitRef 与 content hash；binding 引用其 exact id + hash。snapshot id、maturity date、裸字符串或调用参数均不得成为它的替代身份。R4c 不把 Factor 或 node 字段塞入 Rates 分析请求来只携带血缘。
5. Instrument target 必须含 exact OwnerRef + VersionRef，并在绑定前由定义仓储核验其存在、owner 与版本；只支持 Bond 和 Futures Instrument subtype。普通 Instrument、未解析 subtype、连续 / 拼接序列、跨 owner 或未来 / 旧 version 均失败关闭。曲线 target 必须先验证其 immutable definition hash。服务读取必须同时受现有认证与 target owner 的 AccessScope 约束。
6. target → factor 与 factor → target 都只投影已验证、append-only binding；结果按 canonical target key 稳定排序、去重，并不得推导数值 Exposure、权重、DV01、KRD、可交易性、持仓或市场分支。缺 target、无授权或完整性失败时不返回 partial topology。
7. 不得修改 expected、Oracle、断言、容差、Golden、Phase 2C/2D matrix、guarded hash、selector、command、canonical schema/hash 或 `scripts/layering-allowlist.json`。allowlist 必须保持 `[]`，脚本不在写路径内。任何冻结清单外路径在首次写入前停止并取得 Human 明确授权；不得先改后补或修改本 brief §6 追认。
8. Factor service、descriptor、生成物、migration、API gRPC-Web route、server composition 与 public service inventory 的 diff 必须分别呈现。新增服务只能扩大 inventory，不能修改既有 proto tag、服务方法输入输出或业务语义。

## 3. 非目标

- R4d 的曲线冲击、曲线重建、债券或期货重定价、逐仓位 Exposure、KRD、DV01、权重、总敞口、组合聚合及 AC16；R4d 必须在 R4c 合并并验证后开始。
- 修改 `AnalyzeBond`、`InterpolateYieldCurve`、`AnalyzeCarryRoll`、`AnalyzeFuturesDelivery`、`AnalyzeFuturesHedge` 或任意 Rates request / response；不得把 Factor 仅带入血缘却不进入计算。
- PositionSnapshot、会计三态、资本占用、回购视图、Subject / SubjectState、Constraint、ShadowPrice、CoverageDeclaration、DataHealthReport、RulePack、税收或资金语义。
- 新增市场、市场条件分支、Factor alias、Factor 版本、外部 Factor 映射、自动因子发现、曲线快照持久化、曲线节点数值或定价算法。
- C++ / C ABI、Python 数值实现、Arrow canonical schema、Golden、Oracle、matrix、allowlist、scripts、`SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、任何 ADR、`.gitignore`、`.github/**`、`cicd.yml`、`deploy/**` 或既有 R1–R4b brief。

## 4. 公共契约变化

- 新增 `ficant.research.v1.factor.proto` 与 `FactorRegistryService`。服务只包含 `RegisterFactorDefinition`、`RegisterCurveNodeDefinition`、`BindFactorTarget`、`GetFactorDefinition`、`GetFactorTargets`、`GetTargetFactors` 六个一元 RPC；写操作需要 `factors:write`，全局 definition 读取需要 `factors:read`，target 关系读取还需通过被引用 Instrument owner 的 AccessScope。每个响应使用 typed success-or-`ErrorDetail` oneof。
- `FactorDefinition` 的固定 wire shape 为 `factor_id = 1`、`factor_unit = 2`、`sensitivity_convention = 3`、`content_hash = 4`。`SensitivityConvention` 固定为 `bump = 1`、`direction = 2`、`curve_rebuild = 3`、`second_order = 4`；三个策略 enum 均有 `UNSPECIFIED = 0`，但注册拒绝它。Factor 的 canonical content 不包含其宣称 hash；hash 从其他确定性字段重算。
- `CurveNodeDefinition` 的固定 wire shape 为 `curve_node_id = 1`、`curve_family_id = 2`、`tenor = 3`、`factor_unit = 4`、`content_hash = 5`。它是全局 immutable identity，`curve_node_id` 与 `curve_family_id` 均为规范化、非空的小写点分 id，tenor 使用规范 ISO-8601 period；同 id 不同内容失败关闭。`CurveNodeRef` 固定为 `curve_node_id = 1`、`content_hash = 2`。
- `FactorTargetRef` 固定为 `oneof target`：`InstrumentTarget instrument = 1` 或 `CurveNodeRef curve_node = 2`；`InstrumentTarget` 固定为 `owner = 1`、`instrument = 2`。`FactorTargetBinding` 固定为 `factor_id = 1`、`target = 2`、`content_hash = 3`。binding canonical hash 覆盖 FactorId 与完整 target ref；它不可更新或删除。
- FactorDefinition / CurveNodeDefinition 本身无 owner、tenant 或版本：这落实单机构部署下的全平台唯一性。Instrument binding 保留 owner，防止把同一 VersionRef 误解释为跨 owner 可读。未来多租户支持必须先重裁 SPEC §0 的主体隔离 / 授权契约，不能把 tenant 前缀塞入 FactorId。
- 新服务及 message 进入 Rust、Python、TypeScript 生成合同与 descriptor inventory；Python gRPC 只在 buf 配置显式列出 FactorRegistryService 后生成。既有 proto、Rates API、C++ / C ABI 与 generated API 不作破坏性更改。

## 5. 需 Human 决策

- **已裁决——R4c / R4d 依赖：** R4c 名为“Factor 身份与拓扑”，只点亮 AC05；R4d 依赖 R4c，才点亮 AC16。两轮不能并行。AC05 不验证数值 Exposure、权重或总敞口；ADR-0015 对这些数值能力的要求只有在 R4d 完成后才算全部落实。
- **已裁决——全局 immutable Factor：** FactorId 是不带 owner、tenant、alias 或 version 的全局字符串；相同 id 的不同 definition / convention 失败关闭。此选择遵守 ADR-0015 的“同一经济量只有一个 FactorId 与一套敏感度口径”，并避免以 version 偷渡不可相加的 Exposure。versioned Factor 或 tenant-scoped id 不属于 R4c；如未来需要，必须重新评估 S5 与跨主体聚合语义。
- **已裁决——曲线节点的稳定身份：** 使用 immutable `CurveNodeDefinition(curve_family_id, tenor, factor_unit, content_hash)` 与 exact `CurveNodeRef`，而非 CurveSnapshotId + maturity date 或 Rates 请求字段。curve node id 与 curve family 均采用同一小写点分规范，tenor 采用 ISO-8601 period；R4c 不注册或计算任何曲线数值。
- **已裁决——拓扑对象边界：** 静态 binding 只接受 exact Bond / Futures Instrument subtype 和 CurveNodeRef；不允许普通 Instrument、连续 / 拼接期货、Asset class、裸 maturity、裸 FactorId 或调用方数值。Researcher 具有 `factors:write`（ADR-0018），但所有 FactorId 共享全平台 collision gate；读取 binding 仍遵循目标 Instrument owner scope。
- **执行期事前授权——0017 migration inventory：** Human 已在首次 migration acceptance 取得真实失败证据后，明确授权只扩展 `crates/ficant-storage/tests/migration_acceptance.rs` 的首个 forward-migration 测试及专用于 0017 的局部断言 / fixture。该测试必须精确核验成功历史为 0001–0017、0017 只成功登记一次、重复执行不改变历史，且人为使 0017 失败时不残留其部分 schema 或 history；不得改动另外三个 migration 测试、共享升级辅助、legacy / FK 判据、失败消息、夹具或既有原子回滚断言，也不得用 ignore、过滤、重试或弱化断言制造通过。execution base 已含 0016 而旧断言仍为 15，故此同时记录 R4b 遗留的 migration-inventory 债务，并非完全由 R4c 引入；本条是事前窄范围授权，不修改下列 §6 冻结清单。
- **执行期追加授权——migration 测试隔离：** 上述窄授权后，完整 `migration_acceptance` 并行运行暴露四个测试共享同一 disposable PostgreSQL schema 的相互 reset 竞争。Human 随后明确授权为取得真实 4/4 而完成范围内必要工作；据此只在同一测试文件加入 file-local async mutex，并在四个测试入口持有 guard，使既有四项判据串行使用该共享数据库。没有改动另外三项测试的 fixture、失败消息、业务断言或共享升级辅助，没有使用 ignore、过滤、重试或降低 expected。此项是后续明确授权，不追溯改写原窄授权，也不修改 §6 冻结清单。
- **执行证据偏差——application RED 未独立留存：** domain contract test 在实现前真实取得 exit 101（未解析的 Factor topology import），RED 不是 checkpoint。application contract test 没有在 application 实现前独立留存一次非零 exit code，且 forward-only 纪律禁止为了补记录回退已验证实现；因此 R4c 不宣称完整满足闸门 1 的双 RED 取证。最终 application 判据、实现和全量检查均为绿，但这个过程证据缺口仍需 Human 在候选审阅时可见，不能由最终绿灯倒推为曾经取得 RED。
- **authority 绑定前置：** `SPEC.md`、`ACCEPTANCE.md` 与 `MANUAL.md` 仍位于私有 authority，未被公共候选追踪。R4c 实现不受此阻塞；但 AC05 点亮与 MANUAL 确认必须在公共候选合并后以 authority commit 绑定，并由 Human 逐条批准。agent 不得改写三件套或把 brief 自报证据当作批准。

## 6. 最终真实测试证据

**候选状态：** R4c 已形成完整、尚未 commit 的本地自测候选。execution base 始终为 `610392000726ec55ea591332adec512117e29bd9`，authority base 始终为 `6b57108048b348d22aa4f28689aea42f8fe65f48`；当前分支为 `codex/r4c-factor-krd`。候选只实现 AC05 的 Factor 身份与静态拓扑，不点亮 AC05、不进行 authority binding，也未执行 commit、push、PR 或 merge。

**冻结允许写路径（随上述 Human 裁定与实现授权生效）：**

- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/composition.rs`
- `binaries/ficant-server/tests/factor_registry_sit.rs`（新建）
- `crates/ficant-api/src/factor_registry.rs`（新建）
- `crates/ficant-api/src/grpc_web.rs`
- `crates/ficant-api/src/lib.rs`
- `crates/ficant-api/tests/factor_registry_service.rs`（新建）
- `crates/ficant-api/tests/grpc_web_boundary.rs`
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/ports/factor_topology.rs`（新建）
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/use_cases/factor_topology.rs`（新建）
- `crates/ficant-application/src/use_cases/mod.rs`
- `crates/ficant-application/tests/factor_topology_contracts.rs`（新建）
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.tonic.rs`
- `crates/ficant-domain/src/research/factor_topology.rs`（新建）
- `crates/ficant-domain/src/research/mod.rs`
- `crates/ficant-domain/tests/factor_topology_contracts.rs`（新建）
- `crates/ficant-storage/src/postgres/factor_topology.rs`（新建）
- `crates/ficant-storage/src/postgres/mod.rs`
- `crates/ficant-storage/tests/factor_topology_postgres.rs`（新建）
- `docs/architecture/layering-refactor.md`
- `docs/iterations/2026-08-r4c-factor-identity-topology.md`
- `docs/iterations/README.md`
- `interface/buf.gen.yaml`
- `interface/proto/ficant/research/v1/factor.proto`（新建）
- `interface/README.md`
- `migrations/postgresql/0017_factor_topology.sql`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/factor_pb2.py`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/factor_pb2_grpc.py`（新建）
- `web-dm/packages/contracts-generated/src/ficant/research/v1/factor_pb.ts`（新建）

**禁止写路径：** 所有未逐项列出的路径，特别是 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、`README.md`、`docs/architecture/adr/**`、所有既有 `.proto`（包括 `analytics.proto`、`definition.proto`、`position.proto`）、除明确列出的 generated contract、`cpp/**`、`domain-packs/**`、`scripts/**`、`tests/golden-cases/**`、`tests/oracle/**`、`tests/phase2c/**`、`tests/phase2d/**`、`crates/ficant-data/src/canonical.rs`、`.gitignore`、`.github/**`、`cicd.yml`、`deploy/**`、R1–R4b brief。

**规定的针对性命令（仅在最终候选实际执行后才能填结果）：**

- `cargo test --offline --locked -p ficant-domain --test factor_topology_contracts`
- `cargo test --offline --locked -p ficant-application --test factor_topology_contracts`
- `cargo test --offline --locked -p ficant-storage --test factor_topology_postgres`
- `cargo test --offline --locked -p ficant-api --test factor_registry_service`
- `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory`
- `cargo test --offline --locked -p ficant-server --test factor_registry_sit`
- `pwsh -NoProfile -File scripts/check-layering.ps1`
- `pwsh -NoProfile -File scripts/test-layering-check.ps1`
- `pwsh -NoProfile -File scripts/check-fast.ps1`
- `pwsh -NoProfile -File scripts/check.ps1`
- `pwsh -NoProfile -File scripts/check.ps1 -IncludeIntegration`
- `git diff --check`

**RED-first 与 forward-only 事实：** domain contract test 在任何 domain 实现前以未解析 import 取得 exit 101；该 RED 未作为 checkpoint。application RED 未独立留存，已在 §5 如实记录为过程证据偏差。随后 immutable domain、generated contract、application、PostgreSQL、transport / route 分别在直接测试通过后成为 forward-only checkpoint；普通编译、Clippy、夹具与本地依赖失败均在最近兼容 checkpoint 上前进修复，没有回退已验证结果，没有修改 expected、Oracle、Golden、matrix、canonical hash、allowlist 或门禁断言。

**最终候选的针对性结果：** 以下结果均来自当前同一工作树；Buf 使用锁定的 1.56.0，Python 使用 uv 管理的 CPython 3.12.11。完整入口硬性要求 Node v22.17.0，因此从本机现有可信工具目录注入该版本；未修改脚本来接受 Node 24。

| 命令 | exit code | 结果 |
|---|---:|---|
| `cargo test --offline --locked -p ficant-domain --test factor_topology_contracts` | 0 | 2/2 |
| `cargo test --offline --locked -p ficant-application --test factor_topology_contracts` | 0 | 2/2 |
| `cargo test --offline --locked -p ficant-storage --test factor_topology_postgres` | 0 | 1/1 |
| `cargo test --offline --locked -p ficant-api --test factor_registry_service` | 0 | 1/1 |
| `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory` | 0 | 16/16 |
| `cargo test --offline --locked -p ficant-server --test factor_registry_sit` | 0 | 1/1 |
| `cargo test --offline --locked -p ficant-storage --test migration_acceptance -- --test-threads=1` | 0 | 4/4；精确 0001–0017、repeat 不变、0017 单次成功、注入失败原子回滚 |
| `pwsh -NoProfile -File scripts/check-layering.ps1` | 0 | AC03=0、AC01=0、C++/FFI=0、Funding=0、Tax=0、allowlist=0 |
| `pwsh -NoProfile -File scripts/test-layering-check.ps1` | 0 | 51 assertions |
| `pwsh -NoProfile -File scripts/check-fast.ps1` | 0 | `FICANT fast local checks passed.` |
| `pwsh -NoProfile -File scripts/check.ps1` | 0 | `FICANT complete local checks passed.`；C++ 8/8、Web 35/35、既有 matrix / Oracle / SDK / data checks 全绿 |
| `pwsh -NoProfile -File scripts/check.ps1 -IncludeIntegration` | 0 | `FICANT complete local checks passed.`；migration 4/4，新增执行的 integration tests 合计 32/32 |
| `git diff --check` | 0 | 无 whitespace error；另对全部未跟踪候选文件逐行扫描，trailing whitespace 命中 0 |

**完整入口失败与恢复事实：** 首次完整入口在执行代码前因活动 Node v24.18.0 与脚本锁定 v22.17.0 不一致而 exit 1；改为选择机器上已存在的 v22.17.0 后继续。随后 strict Clippy 真实暴露缺失 `# Errors` 文档、tuple type complexity 与过长测试函数，均以文档和结构拆分修复，未加 lint 豁免。下一次完整入口在 Web typecheck 前发现 `web-dm/node_modules` 缺失；以 `corepack pnpm@10.12.4 install --offline --frozen-lockfile` 恢复被忽略的本地依赖后通过。最终语义审计又发现 FactorId 原为“至少四段”，与冻结的“精确四段”不一致；domain 与 0017 constraint 同步收紧并加入五段 id 负向判据，之后重新取得上述全部最终结果。

**Acceptance sentence 与 AC05 证据：**

- domain canonical hash 覆盖 FactorId、Factor UnitRef、带 UnitRef 的 positive bump、方向、曲线重建与二阶策略；FactorId 只接受精确四段小写点分形状。PostgreSQL 直接测试逐一更改 UnitRef、bump、方向、重建和二阶策略，五类同 id 注册均以 `AlreadyExists` 失败关闭。
- `cn.gov.yield.10y`、稳定 `cn.gov.curve.cny.10y` / `P10Y` 节点、同 owner 的 exact Bond v1 与 T Futures v1 均经真实 0017 schema 持久化。相同 definition 与 binding command 的重复执行为幂等重放。
- 三个 target → Factor 查询都只返回同一 exact definition；Factor → targets 返回三项并按 canonical target key 稳定排序。hash 漂移 / 未注册曲线节点、未注册 Instrument version、无 Bond/Futures subtype 及越权读取或绑定均失败，失败后关系总数仍为 3，不返回 partial topology。
- 新 `FactorRegistryService` 六个 unary RPC 已进入 Rust / Python / TypeScript 生成物、descriptor inventory、gRPC-Web 路由和 server 的真实 PostgreSQL 生产组合；既有五服务组合入口保留为兼容 wrapper。
- R4c 的 proto、domain、application、storage 与 API 源码扫描 `Exposure|DV01|KRD|weight` 命中为 0；Rates 请求、定价公式、C++ / C ABI、RulePack、Golden、Oracle、canonical schema/hash、Phase 2C/2D matrix 与 allowlist 均无 diff。因此本 brief 的 acceptance sentence 在本地候选上成立；AC05 是否点亮仍只由公共 merge 后的 authority / Human 决定。

**写路径与冻结资产审计：** `git diff --name-only` 加未跟踪文件共 32 个路径；与 §6 冻结清单及 §5 明确授权的 `migration_acceptance.rs` 逐项比对，冻结范围外为 0。`SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、ADR、既有 proto、`cpp/**`、`domain-packs/**`、`scripts/**`、Golden、Oracle、Phase 2C/2D matrix、`crates/ficant-data/src/canonical.rs`、发布与部署路径的 diff 均为 0；唯一新增 proto 是本轮授权的 `factor.proto`；`scripts/layering-allowlist.json` 内容仍精确为 `[]`。

## 7. 残余风险

- R4c 的 Factor topology 不含数值 Exposure。它只能证明“哪些对象共享哪一个经济量”，不能证明敏感度大小、权重、可加性或套保效果；R4d 必须从精确 PositionSnapshot、曲线快照、Instrument definition 与本轮 FactorDefinition 内部冲击并重定价。
- R4c 不改变 Rates request，因此现有 CurveSnapshot / YieldCurveNode 没有稳定 node identity 的限制仍然存在于数值调用面。R4d 需要在自己的独立契约中解决它，不能回填为本轮的血缘字段。
- 全平台 global FactorId 与单机构部署边界一致；未来多租户须先重裁主体隔离、授权、factor collision 与查询可见性，不能从 R4c 的 owner-scoped Instrument binding 推断多租户安全。
- 公共候选合并后，authority 仍需独立绑定公共 commit、Human 批准 AC05 并更新 MANUAL。该私有流程不是本轮 execution freeze 的一部分，也不能被 brief 的计划或测试证据替代。
