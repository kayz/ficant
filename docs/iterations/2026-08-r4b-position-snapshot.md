# R4b 迭代 brief — PositionSnapshot、会计三态与回购穿透

**迭代：** R4b · **本轮目标条目：** AC14、AC17、AC18、AC19 · **execution base：** `f620d62329248362a64f5b290d17d3dff5b81f09` · **authority commit：** `3c89da0bc62375edaf05ddc2cb51b53d3a6218c2`

本 brief 是 R4b 面向 Human 的唯一设计。R4b 以不可变、内容哈希寻址的 `PositionSnapshot` 记录已导入的持仓事实，并在一个真实的知识时点查询中选择当时可见的修订；它只提供经济 / 会计视图、资本占用聚合前置所需的严格分类门槛，以及质押式回购的敞口 / 可用流动性归属。R4a 的 CTD 历史事实边界与 R4c 的 Factor / KRD 保持独立。本 brief 只冻结设计和执行边界，不构成实现、push、Pull Request、merge、authority 绑定或发布授权。

## 1. 目标

将 Position 作为外部导入的点时事实，而不是可变账户或总账。每个 `PositionSnapshot` 必须有精确 Subject 版本、owner、完整 `MarketTime` 的 `observed_at` / `visible_at`、不可变 canonical 内容哈希、可追溯 lineage，以及逐仓位显式会计三态与回购持有形态。服务端以 `(subject_ref, observed_at, knowledge_at)` 选择该知识时点可见的最新修订，不得把后来补录的会计科目或回购穿透状态带回过去。

**Acceptance sentence：**

> 对同一 Subject 与同一完整 `observed_at`，先发布可见于 T0 的 PositionSnapshot，后发布一个仅修正会计分类或回购穿透的快照并令其在 T1 才可见；以 T0 查询只返回初始内容、以 T1 查询才返回修订，两个结果均是已验内容哈希的不可变 snapshot。任何仓位分类为显式 `UNKNOWN` 时，`CalculateCapitalUse` 在返回数值前失败关闭并逐一指明 position id；同一经济数量、价值和经济 P&L 的 AC 与 FVTPL 仓位在经济视图相同、在导入的会计 P&L 视图不同。正回购出质的自有券仍计入持仓敞口、但不计入可用流动性；质押式逆回购收到的券不计入持仓敞口或可用流动性，只能作为担保品事实展示，不能被当作自有或可处分资产。R4a 已点亮条目、C++ / C ABI、RulePack、Golden、Oracle、canonical schema/hash、Phase 2C/2D matrix 与 allowlist 均不变。

## 2. 验收

| 条目 | R4b 可执行判据 |
|---|---|
| AC14 | 以同一 `(owner, subject_ref, observed_at)` 注册两个内容不同、snapshot id 不同的 immutable PositionSnapshot：初始快照 `visible_at = T0`，修订快照 `visible_at = T1`。`ResolvePositionSnapshot` 以完整 `MarketTime` 接受 `observed_at` 与 `knowledge_at`；T0 只返回初始 snapshot，T1 返回修订 snapshot。任一记录 `observed_at > visible_at`、不同 owner / Subject 的混入、或 content hash 与 canonical payload 不符必须失败关闭；按 snapshot id 的读取也必须拒绝 `visible_at > knowledge_at`。 |
| AC17 | `CalculateCapitalUse` 只接受经上述 resolver 选出的 immutable PositionSnapshot，并先检查每一仓位的显式分类。任一 `UNKNOWN`（包括 protobuf `UNSPECIFIED`、缺 classification 或 UNKNOWN 携带 book）均在聚合前以 non-retryable `VALIDATION_FAILED` 失败，错误只给出稳定 field `positions[*].accounting_classification` 与全部未知 position id；不得返回部分 capital amount，也不得把 UNKNOWN 当 AC / FVOCI / FVTPL、NOT_APPLICABLE 或经济视图。分类有效时，该窄聚合仅合计每个已导入仓位的 `capital_requirement`，不虚构外部占用、限额判断或监管资本公式。 |
| AC18 | 两个只在显式 `CLASSIFIED(AC)` / `CLASSIFIED(FVTPL)` 及外部导入 `accounting_pnl` 不同的 PositionSnapshot，必须保留同一 instrument version、quantity、economic value 和 `economic_pnl`；`GetPositionViews` 返回相同经济字段、不同会计 P&L。该会计 P&L 是 snapshot 所携带的外部事实，R4b 不产生估值入账、科目转换、总账分录或会计政策推导。 |
| AC19 | `GetPositionViews` 对 `OWNED`、`REPO_SOLD`、`REVERSE_REPO_COLLATERAL` 分别产生可核验 inclusion：OWNED 进入敞口和可用流动性；REPO_SOLD 进入敞口、排除可用流动性；REVERSE_REPO_COLLATERAL 同时排除，并只在 `collateral_facts` 展示。不得以“收到质押券”推断出售权、再质押权或可用流动性；R4b 也不建立买断式回购或 collateral reuse 权利。 |

R4b 闸门：

1. 先只加入 AC14、AC17–AC19 的 application / gRPC 判据并亲眼取得非零 exit code；RED 不构成 checkpoint。Position domain、repository、canonical codec、API adapter、server composition 或 PostgreSQL migration 不得早于该 RED。
2. 双时间选择使用完整 `MarketTime`：`observed_at` 与 `knowledge_at` 必须同时匹配 UTC instant、IANA timezone 与 local trading date。resolver 只可从同一 `(owner, subject_ref, observed_at)` 中选取 `visible_at <= knowledge_at` 的最大 `visible_at`；没有可见版本返回 NotFound，不能返回当前版本或按本地日期偷换时间。
3. `AccountingClassification` 为显式、封闭三态：`CLASSIFIED` 必须含且只含 AC / FVOCI / FVTPL book；`NOT_APPLICABLE` 与 `UNKNOWN` 不得携带 book；protobuf 缺失 / UNSPECIFIED 是无效输入，不是 UNKNOWN 的旁路。所有依赖分类的路径先检查全量仓位，后计算，错误一次列全未知 id。
4. `economic_value`、`economic_pnl`、`accounting_pnl` 与 `capital_requirement` 都是已导入、带 UnitRef 的外部事实；R4b 只按 snapshot 做确定性投影 / 聚合，不能把会计、资本或流动性数值硬编码进 domain、C++、FFI 或调用方默认值。`capital_requirement` 只证明本轮的分类 fail-closed 与聚合边界，不能被描述为监管资本、额度占用或约束引擎。
5. `REVERSE_REPO_COLLATERAL` 是质押式逆回购担保品的明确事实形态：它不进入持仓敞口或可用流动性。若未来要纳入可用流动性，必须先存在独立、已验证的法律与合同处分 / 再质押权契约；不得从该形态或价值推断该权利。
6. PositionSnapshot 采用既有 immutable verified-blob / content-hash / lineage 基础设施；metadata lookup 不足以构成 AC14 读取。服务必须核验其 owner、canonical payload hash 与 durable blob 引用后才投影或聚合；失败结果不得产生 partial view 或 lineage。
7. 不得修改 expected、Oracle、断言、容差、Golden、Phase 2C/2D matrix、guarded hash、selector、command、canonical schema/hash 或 `scripts/layering-allowlist.json`。allowlist 必须保持 `[]`，脚本不在写路径内。任何冻结清单外路径在首次写入前停止并取得 Human 明确授权；不得先改后补或修改本 brief §6 追认。
8. contract descriptor、生成物、migration、API composition 与 public service inventory 的 diff 必须分别呈现。新增 `PositionSnapshotService` 只能扩大服务 inventory，不能修改既有服务 / message 的 wire tag、方法输入输出或语义。

## 3. 非目标

- R4a 的 CTD snapshot / FuturesContract 输入语义，以及 AC27、AC28；R4c 的 Factor / Exposure / KRD、AC05、AC16。
- 持仓变动流水、估值入账、科目转换处理、总账、监管报表、会计政策或税务计算。
- 约束、ShadowPrice、限额余量、SubjectState 的额度校验、监管资本公式、外部已占用申报、CoverageDeclaration、DataHealthReport 或组织级结论。
- 买断式回购、债券借贷的完整契约、质押券处分 / 再质押权、担保品折扣或以担保品推断可出售流动性。
- 新增 / 修改 RulePack、Python 数值实现、C++ / C ABI、Arrow canonical schema、Golden、Oracle、matrix、allowlist、scripts、`SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、任何 ADR、`.gitignore`、`.github/**`、`cicd.yml`、`deploy/**` 或既有 R1–R4a brief。

## 4. 公共契约变化

- 新增 `ficant.research.v1.PositionSnapshot`、`Position`、`AccountingClassification`、`PositionHoldingForm` 及相应请求 / 响应。wire shape 固定为：`PositionSnapshot.snapshot_id = 1`、`owner = 2`、`subject_ref = 3`、`observed_at = 4`、`visible_at = 5`、`content_hash = 6`、`lineage = 7`、`positions = 8`；`Position.position_id = 1`、`instrument_ref = 2`、`quantity = 3`、`economic_value = 4`、`economic_pnl = 5`、`accounting_pnl = 6`、`capital_requirement = 7`、`accounting_classification = 8`、`holding_form = 9`；`AccountingClassification.state = 1`、`book = 2`。state enum 固定为 `UNSPECIFIED = 0`、`CLASSIFIED = 1`、`NOT_APPLICABLE = 2`、`UNKNOWN = 3`；book enum 固定为 `UNSPECIFIED = 0`、`AC = 1`、`FVOCI = 2`、`FVTPL = 3`；holding enum 固定为 `UNSPECIFIED = 0`、`OWNED = 1`、`REPO_SOLD = 2`、`REVERSE_REPO_COLLATERAL = 3`。所有经济 / 会计 / capital Decimal 必须带 UnitRef；PositionSnapshot canonical content 不包含其宣称的 hash，hash 必须由剩余确定性 payload 重算。
- 新增 `ficant.research.v1.PositionSnapshotService`：`PublishPositionSnapshot`、`GetPositionSnapshot`、`ResolvePositionSnapshot`、`GetPositionViews` 与 `CalculateCapitalUse`。每个 request 的业务 payload 固定从 field `1` 开始；发布以 `idempotency_key = 1`、`snapshot = 2`，其余四项均以 snapshot id / resolver key / knowledge time 的 typed request 与 oneof success-or-ErrorDetail response 表达。发布要求经验证的 immutable blob、owner / tenant scope、idempotency、hash 与 lineage；id 读取及 resolver 都要求 `knowledge_at`，不暴露 `visible_at > knowledge_at` 的事实。resolver 以 exact `(owner, subject_ref, observed_at)` 取最大可见版本，不提供“当前快照”默认值。
- 新增 `GetPositionViews` 与 `CalculateCapitalUse` 到同一服务。前者只从已验证 snapshot 投影经济、会计、敞口、可用流动性和担保品事实；后者先对完整 snapshot 施行会计三态门槛、再确定性合计 `capital_requirement`。二者成功响应均绑定精确 PositionSnapshot 的 id / hash / lineage；未知分类时不返回 amount。
- 应用层新增 PositionSnapshot repository / verified-read port，存储层新增 immutable metadata 与 payload / blob 引用。PositionSnapshot 是现有 `SnapshotValue` 的第三个 variant，复用 server-verified blob、content-addressed persistence、AccessScope、idempotency 与 lineage 机制；它不让 domain 引用存储、网络或认证。
- 新服务及 message 进入 Rust、Python、TypeScript 生成合同和 descriptor inventory。既有 proto tag、服务、generated API 及 C++ / C ABI 不作破坏性更改；新增 Python gRPC 输出只在 buf 配置显式列出 PositionSnapshotService 后生成。

## 5. 需 Human 决策

- **已裁决——AC19 优先：** AC19 是 SPEC 之外回购细节的验收权威。质押式逆回购收到的质押券不计入持仓敞口或可用流动性；它可作为担保品 / 信用保护事实展示，但不是自有、可出售或可处分资产。正回购出质的自有券仍计入持仓敞口，但不计入可用流动性。
- **ADR-0011 待 authority 后续修订（本轮不改 ADR）：** 将“逆回购收到的质押券不计入持仓敞口，但计入流动性视图”改为“作为担保品进入信用保护视图；只有存在明确、已验证的法律与合同处分或再质押权时，才可进入可用流动性视图”。R4b 不建立该权利契约，因此质押式逆回购担保品一律排除于可用流动性。未来买断式回购或可复用担保品必须另行冻结 `collateral_disposition` / `reuse_right` 契约，不能从“收到质押券”推断。
- **已作实现性澄清：** AC17 所称“资本占用计算”在 R4b 是严格的、snapshot 内逐仓位 `capital_requirement` 聚合与分类门槛，不是监管资本、约束、额度余量或外部占用输入。这个窄面足以验证 UNKNOWN fail-closed，且不越过 v0.2 Constraint / ShadowPrice 边界。
- **已作边界澄清：** AC18 的 accounting P&L 是外部系统在 snapshot 中导入的事实；R4b 并不由 AC / FVOCI / FVTPL 推导会计金额或制作分录。classification 只决定哪些视图 / capital 聚合可被安全请求，经济与会计数值并行可追溯。
- **执行期扩权（事前授权）：** 为实现 AC17 的同 UnitRef `capital_requirement` 聚合，Human 于 2026-07-31 授权仅修改 `crates/ficant-domain/src/primitives/decimal.rs`，新增由 domain 拥有的 `DecimalValue::checked_add`。现有 API 无公开算术且其内部 decimal 转换私有；在 application 层重解析字符串会绕过单位 / 精度边界。该授权不修改或追认 §6 冻结清单，也不扩展至其他 primitives、公式、Constraint 或 capital 语义。
- **执行期扩权（事前授权）：** 为将 AC17 的全部未知 position id 安全传递至 transport，Human 的“不要再打断直到完成本轮”授权涵盖 `crates/ficant-application/src/error.rs` 的最小扩展：新增 `UnknownAccountingPositions { position_ids }` client-safe detail。它只服务本轮 ValidationFailed 错误，不改变既有 error code、RulePack 或 Subject detail，也不修改 §6。
- **执行期扩权（事前授权）：** PositionSnapshot 成为既有 `SnapshotValue` 第三个 immutable variant 后，Human 持续授权涵盖由穷尽匹配强制暴露的 `crates/ficant-application/src/ports/fingerprint.rs`、`crates/ficant-application/src/use_cases/data_snapshot.rs` 与 `crates/ficant-application/src/use_cases/phase1_business_loop.rs`。前者新增 position fingerprint；后二者对非自身 Data / Phase1 发布路径 fail-closed。它们不扩展既有 Data/Universe 行为，也不追认 §6。
- **执行期扩权（事前授权）：** `crates/ficant-api/src/core_error.rs` 必须把新增的 client-safe UNKNOWN detail 转换为稳定 `positions[*].accounting_classification` field violation，才能兑现 AC17“指明哪些仓位”的公开错误合同。该最小 mapper 分支不改变既有 code、trace、retry 或其他 detail；由 Human 持续授权覆盖，不改 §6。
- **执行期扩权（事前授权）：** 为让 R4b application contract test 使用既有锁定 runtime，Human 持续授权覆盖 `crates/ficant-application/Cargo.toml` 的 test-only workspace `tokio` 声明；无新 package 或生产依赖。Cargo 依赖图的已有锁定项因新增 direct dev-dependency 在 `Cargo.lock` 出现一条最小记录变更；该文件本已在 §6，未更改版本选择。该路径因 test harness 漏列未在 §6，现如实记录。
- **执行期扩权（事前授权）：** 首次 PostgreSQL integration test 以 `LineageIncomplete` 暴露通用 lineage candidate query 遗漏 PositionSnapshot：任何以后引用已发布 PositionSnapshot 的血缘都会错误失败关闭。Human 的持续授权涵盖仅修改 `crates/ficant-storage/src/postgres/common.rs`，在既有 immutable snapshot candidates 中加入 `research.position_snapshots` 的 `(snapshot_id, content_hash)` 事实；不改变其他对象、允许范围、lineage 写入或错误分类。该路径不在 §6，现作为精确扩权记录而非追认。
- **实施授权已给出：** Human 于 2026-07-31 明确授权按本 brief 实现。该授权不包含 push、Pull Request、merge、authority binding 或发布。

## 6. 最终真实测试证据

**候选状态：** 本地自测候选已完成，尚未创建 commit、push、Pull Request、merge 或 authority binding。execution base 仍为 `f620d62329248362a64f5b290d17d3dff5b81f09`；最终候选未修改 allowlist、canonical schema/hash、Golden、Oracle、Phase 2C/2D matrix、C++ / C ABI、既有 proto 或 ADR。

**冻结允许写路径（随 execution base 与本次 Human 裁定一并冻结）：**

- `Cargo.lock`
- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/position_snapshot_sit.rs`（新建）
- `crates/ficant-api/src/grpc_web.rs`
- `crates/ficant-api/src/lib.rs`
- `crates/ficant-api/src/position_snapshot.rs`（新建）
- `crates/ficant-api/tests/position_snapshot_service.rs`（新建）
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/ports/positions.rs`（新建）
- `crates/ficant-application/src/ports/snapshots.rs`
- `crates/ficant-application/src/use_cases/mod.rs`
- `crates/ficant-application/src/use_cases/position_views.rs`（新建）
- `crates/ficant-application/tests/position_snapshot_contracts.rs`（新建）
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.tonic.rs`
- `crates/ficant-domain/src/research/mod.rs`
- `crates/ficant-domain/src/research/position_snapshot.rs`（新建）
- `crates/ficant-domain/tests/position_snapshot_contracts.rs`（新建）
- `crates/ficant-storage/src/postgres/codec.rs`
- `crates/ficant-storage/src/postgres/mod.rs`
- `crates/ficant-storage/src/postgres/positions.rs`（新建）
- `crates/ficant-storage/src/postgres/snapshots.rs`
- `crates/ficant-storage/tests/position_snapshot_postgres.rs`（新建）
- `docs/iterations/2026-08-r4b-position-snapshot.md`
- `docs/iterations/README.md`
- `interface/proto/ficant/research/v1/position.proto`（新建）
- `interface/buf.gen.yaml`
- `interface/README.md`
- `migrations/postgresql/0016_position_snapshots.sql`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/position_pb2.py`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/position_pb2_grpc.py`（新建）
- `web-dm/packages/contracts-generated/src/ficant/research/v1/position_pb.ts`（新建）

**禁止写路径：** 所有未逐项列出的路径，特别是 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、`README.md`、`docs/architecture/adr/**`、既有 `.proto`、除明确列出的 `ficant.research.v1` 外的 generated contract、`cpp/**`、`domain-packs/**`、`scripts/**`、`tests/golden-cases/**`、`tests/oracle/**`、`tests/phase2c/**`、`tests/phase2d/**`、`crates/ficant-data/src/canonical.rs`、`.gitignore`、`.github/**`、`cicd.yml`、`deploy/**`、R1–R4a brief。

**规定的针对性命令（仅在最终候选实际执行后才能填结果）：**

- `cargo test --offline --locked -p ficant-domain --test position_snapshot_contracts`
- `cargo test --offline --locked -p ficant-application --test position_snapshot_contracts`
- `cargo test --offline --locked -p ficant-storage --test position_snapshot_postgres`
- `cargo test --offline --locked -p ficant-api --test position_snapshot_service`
- `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory`
- `cargo test --offline --locked -p ficant-server --test position_snapshot_sit`
- `pwsh -NoProfile -File scripts/check-layering.ps1`
- `pwsh -NoProfile -File scripts/test-layering-check.ps1`
- `pwsh -NoProfile -File scripts/check-fast.ps1`
- `pwsh -NoProfile -File scripts/check.ps1`
- `pwsh -NoProfile -File scripts/check.ps1 -IncludeIntegration`
- `git diff --check`

**RED-first、forward-only checkpoint 与最终命令结果：**

- RED-first：在创建 PositionSnapshot domain contract 前执行 `cargo test --offline --locked -p ficant-domain --test position_snapshot_contracts`，因测试目标中的 `PositionSnapshot` 未解析而以 exit `101` 失败；RED 未作为 checkpoint。随后只前进式补齐 domain → generated contract → application/storage → transport/router，每个阶段先跑其直接测试再继续。
- `cargo test --offline --locked -p ficant-domain --test position_snapshot_contracts`：exit `0`，`5 passed`。覆盖双时间与 canonical hash（含 lineage）、AC / FVTPL 经济 / 会计事实分离、三态封闭、AC19 三种持有形态，以及 UnitRef 精确聚合。
- `cargo test --offline --locked -p ficant-application --test position_snapshot_contracts`：exit `0`，`1 passed`。UNKNOWN 在聚合前拒绝、返回全部 position id detail，逆回购担保品不进入敞口或可用流动性。
- `cargo test --offline --locked -p ficant-storage --test position_snapshot_postgres`：exit `0`，`1 passed`。在 disposable PostgreSQL 上发布同一 observed time 的两个修订；T0 只解析初始 snapshot，T1 解析较晚可见修订，读操作仍受 owner scope 约束。
- `cargo test --offline --locked -p ficant-api --test position_snapshot_service`：exit `0`，`1 passed`；adapter 实现冻结的 PositionSnapshotService。其 module tests 另以 `2 passed` 验证 protobuf UNSPECIFIED / 缺 book 拒绝及显式 UNKNOWN 保留。
- `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory`：exit `0`，`15 passed`。固定 PositionSnapshot message/enum/oneof 字段与五个 unary RPC 的精确 inventory。
- `cargo test --offline --locked -p ficant-server --test position_snapshot_sit`：exit `0`，`1 passed`；PositionSnapshotService 有独立的公开 gRPC route。
- `buf format --diff --exit-code interface`、`buf lint interface`、`cargo fmt --all -- --check`、`cargo clippy --offline --workspace --all-targets --locked --exclude ficant-contracts --exclude ficant-contract-tests --no-deps -- -D warnings`、`git diff --check`：均 exit `0`。
- `pwsh -NoProfile -File scripts/check-layering.ps1`：exit `0`，读数为 `AC03=0`、`AC01=0`、Phase 2C C++/FFI=0、R3a Funding=0、R3b Tax=0、allowlist=0；`test-layering-check.ps1` 同样 exit `0`。
- `pwsh -NoProfile -File scripts/check-fast.ps1`：exit `0`（完整快检通过）。在本机 Node `22.17.0`、锁定 uv、并从 Windows User environment 导入六个不输出的 `FICANT_TEST_*` 值后，`pwsh -NoProfile -File scripts/check.ps1` 与 `pwsh -NoProfile -File scripts/check.ps1 -IncludeIntegration` 均 exit `0`。首次 integration 入口曾因 `target/debug/deps` 临时 rmeta 文件在扫描中消失而在分层 gate exit `1`；未改脚本、allowlist 或期望值，单独 gate 重跑通过后从头重跑完整 integration 门禁，最终结果如上。

## 7. 残余风险

- R4b 的 PositionSnapshot 只代表已导入范围内的点时事实。CoverageDeclaration 与 DataHealthReport 由 R5 处理；在那之前，任何 capital amount 只能被表述为该 immutable snapshot 的聚合，不是组织级真实占用或剩余限额。
- 此轮的 `capital_requirement` 是导入的逐仓位度量，用来建立三态 fail-closed 边界；它不是监管资本方法、风险权重、约束引擎或可以替代主体额度的数值。完整资本 / 限额语义仍需 v0.2 的 Constraint / ShadowPrice 契约。
- 会计 P&L 作为外部事实并行展示，不构成会计处理、科目转换或财务报表。若未来要求 ficant 推导 AC / FVOCI / FVTPL 会计金额，必须独立评估 ADR-0011 的“不核算”边界。
- R4b 明确排除质押式逆回购担保品的可用流动性。买断式回购、再质押、处分权、担保品折扣和债券借贷仍无对象合同；任何将担保品计入可用流动性的需求都必须先冻结权利事实与证据来源。
- 为 AC14 引入的 resolver 只在 exact Subject、owner 和完整 observed time 内选择最晚可见修订；它不替代 R4a DataSnapshot 的知识时点规则，也不定义跨 Subject 合并组合或“当前时间”默认查询。
- 公开代码候选经 Human rebase merge 后，authority 仍需在新的独立提交中：批准 AC14、AC17–AC19，更新 MANUAL 的可调用能力与 ADR-0011，并由 `verify-authority.ps1` 绑定公开 commit。该后续流程不得倒写为 execution freeze 或被实施者自行追认。
