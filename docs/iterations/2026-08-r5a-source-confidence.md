# R5a 迭代 brief — 价格来源类型与可信度标记

**迭代：** R5a · **承接条目：** AC15 · **execution base：** `0a7b75a337c74f93ca968cd4807682fae8da1385` · **authority base：** `9234962ccee81f2503dc0bafa0ded26270fe58bc`

本 brief 是 R5a 面向 Human 的唯一设计。R5 已由 Human 拆为严格依赖的 `R5a（AC15）→ R5b（AC35）` 与 `R5a（AC15）→ R5c（AC36）`；本轮只让价格记录可追到封闭来源类型，并让真实消费多种类型的组合风险结果携带结构化标记。本文冻结验收、非目标、公共契约、测试与逐文件写路径；不创建状态页、子任务 brief、治理 checklist 或进度副本。

## 1. 目标

把 SPEC I9 的四种价格来源性质建成不可猜测、可精确查询的公共契约：`REAL_TRADE`、`ACTIVE_QUOTE`、`MODEL_VALUATION`、`CURVE_INTERPOLATION`。外部数据集的类型属于 exact immutable DataSource version；同一版本内数据集必须同质。Quote / Trade / Valuation 的 `FactSource` 可携带该 exact DataSource ref，canonical quote v1 继续使用其既有 `data_source_id` / `data_source_version` 列与 manifest，不增加来源类型列。

生产端提供 DataSource 注册与 exact-version 查询路由。新注册版本必须显式给出非 `UNSPECIFIED` 类型；旧未标型版本不推断、不回填，进入 typed calculation 时失败关闭。组合 KRD 对内部曲线重建产生的价格明确标为 `CURVE_INTERPOLATION`；含具体期货时，对 verified canonical quote snapshot 精确解析 DataSource version 并要求 `ACTIVE_QUOTE`。结果按实际价格证据消费次数给出稳定排序的类型分布；两种以上类型时 `mixed = true`，但不改变数值、不降级也不阻断。

**Acceptance sentence：**

> 注册三个分别声明真实成交、活跃报价与模型估值的 immutable DataSource exact version，并由内部曲线算法产生第四种 `CURVE_INTERPOLATION` 证据；由任一携带 exact DataSource ref 的外部价格事实或 canonical quote 行可经生产 DataSource registry 查得唯一非空来源类型。对同一 PositionSnapshot / CurveSnapshot 的 bond-only `CalculateKeyRateDv01`，结果明确只含 `CURVE_INTERPOLATION`；加入 exact FuturesContract 与 verified canonical quote snapshot 后，结果同时含 `ACTIVE_QUOTE` 与 `CURVE_INTERPOLATION`、`mixed = true`，而全部逐仓位 KRD 与 totals 与加标前口径逐位相同。未注册、legacy 未标型、类型与 canonical quote 不符、exact ref / owner / version 漂移或 protobuf enum 越界，均在 delivery / curve / bond 数值引擎调用前失败关闭。canonical quote v1/schema/hash、Golden、Oracle、Phase 2C/2D matrix、C++ / C ABI、既有定价公式与 allowlist 均不变。

## 2. 验收

| 条目 | R5a 可执行判据 |
|---|---|
| AC15 · 类型可查 | `DataSourceRegistryService` 注册并 exact 查询三种外部封闭类型；`CURVE_INTERPOLATION` 只由内部算法产生并在外部注册时失败，`UNSPECIFIED` 与 enum 越界同样失败。`FactSource.data_source` 是 exact `VersionRef`；canonical quote v1 仍从冻结列 / manifest 取得同一 ref。 |
| AC15 · 单一标记 | bond-only KRD 的 `source_confidence` 非空、只含 `CURVE_INTERPOLATION`，计数等于实际产生逐仓位曲线插值价格的 included position 数，`mixed = false`。 |
| AC15 · 混合标记 | Bond + Futures KRD 同时包含稳定排序的 `ACTIVE_QUOTE` 与 `CURVE_INTERPOLATION` 计数，`mixed = true`；响应外重新核对全部 position / total Decimal 与 UnitRef，证明标记不改变算法结果。 |
| AC15 · 失败关闭 | missing exact DataSource、legacy 未标型 source、非 `ACTIVE_QUOTE` 的 canonical quote source、owner / version 漂移在 RulePack parser、delivery engine、curve engine 与 bond engine 调用前失败；API enum 越界在 repository write 前失败。 |

R5a 闸门：

1. RED-first 分三次取得：domain / protobuf 来源类型与结果标记 contract；application 的 exact source resolution 与 mixed-result contract；transport / persistence / production composition contract。每次先只加判据并取得真实非零 exit code，记录首个真实错误；RED 不是 checkpoint。domain、application、storage、transport 只有对应直接测试转绿后才能成为 forward-only checkpoint。
2. `PriceSourceType` 是封闭枚举，0 仅作 wire 缺省并在所有写入 / typed read 中无效。外部 DataSource exact version只能声明 `REAL_TRADE`、`ACTIVE_QUOTE` 或 `MODEL_VALUATION`；`CURVE_INTERPOLATION` 是 ficant 内部算法证据，不得伪造成供应商 DataSource 属性。
3. DataSource version 是同质数据集的最小声明边界；同一 id/version 以不同来源类型重放必须触发 immutable / idempotency 冲突。换来源性质必须发布下一 immutable version，不能逐行覆盖。
4. `FactSource` 只增加 exact DataSource ref，不复制来源类型。服务端从该 ref 查询类型；canonical quote 从冻结 v1 行与 verified manifest 的同一 exact ref 查询。不得从自由文本 `source_id`、`Valuation.method`、instrument、market、文件名或供应商名称推断。
5. migration 0021 只增加 nullable legacy-compatible 来源类型和 exact-ref columns。旧行保持 `NULL`，读取兼容但 typed operation 失败；不得根据表名或历史值静默 backfill。新 registry write 必须非空，新 typed price fact 可持久化 exact ref 并由 FK 约束版本存在。
6. canonical quote v1 的 16 列、schema id/hash、排序与 manifest 形状全部冻结。R5a 只读取既有 `data_source_id` / `data_source_version`，修改 `crates/ficant-data/src/canonical.rs` 即失败候选。
7. `PortfolioKeyRateExposure` 新增字段 9 `source_confidence`。分布按 enum code 升序、每项计数大于 0、不得重复；`mixed` 必须严格等于不同类型数大于 1。计数语义是价格证据消费次数：每个 included position 的内部曲线插值计一次；每个期货仓位所消费的 verified canonical quote 记录各计一次，同一 immutable record 被两个仓位消费则计两次。该结构进入 portfolio content hash；exact DataSource ref 进入 lineage。
8. R5a 不建立全局高低可信度序、不选择“更可信”价格、不改变 KRD / CTD / midpoint / fixed-CTD 公式。混合只标记；任何数值差异、自动降级或拒绝混合都失败。
9. API 注册 / 查询、PostgreSQL round-trip、gRPC-Web 路由与 `ficant-server` 生产组合必须使用同一 `DataSourceRepository`。只在测试 helper 中存在、未进入 `run_from_env` 的服务不算完成。
10. descriptor 变更只能反映 §4 的已授权加法；不得删除既有 message / field / method，不得改既有 tag。生成器版本和参数保持，生成输出由固定 Buf 重建。
11. migration acceptance 精确冻结 forward inventory 为 0001–0021，证明 0021 只登记一次、重复执行不改变集合，且人为注入 0021 尾部失败时新增 column / constraint / FK 与 migration history 全部原子回滚。保留既有 legacy / FK 判据、失败消息与其余测试。
12. 不得修改 expected、Oracle、Golden、Phase 2C/2D matrix、guarded hash、selector、command、canonical hash、allowlist 或分层门禁断言制造通过。descriptor 的新增 service / field 断言是本轮已授权公共契约的 RED-first 判据，必须先红后绿并单独审阅。

## 3. 非目标

- R5b CoverageDeclaration、组合级分母 / 缺失字段 / 可信度覆盖分布；`PortfolioKeyRateExposure` 字段 10 保留给后续独立冻结，不在本轮预占或实现。
- R5c DataHealthReport、阈值、预警、降级、阻断、质量评分或自动选择来源；混合来源本轮只标记。
- TaxRulePack、税后 IRR / CTD、AC08 / AC09 / AC10 后续升级、Constraint、ShadowPrice、Policy 或 v0.2 能力。
- canonical v2、记录级来源类型覆盖、同一 DataSource version 内混合类型；真实供应商需要混排时必须先另开 schema / ingestion 迭代。
- 实现当前仅存在于 proto 的完整 MarketFactService，或为 Cashflow 建价格来源类型。FactSource exact ref 是加法契约，不把本轮扩大成所有事实 transport 重写。
- 修改定价公式、Factor convention、PositionSnapshot、C++ / C ABI、Golden、Oracle、matrix、allowlist、authority 三件套、任何 ADR、`.github/**`、`cicd.yml`、`deploy/**`、版本 tag 或发布。

## 4. 公共契约变化

- 新增 `ficant.market.v1.PriceSourceType`：0 `UNSPECIFIED`、1 `REAL_TRADE`、2 `ACTIVE_QUOTE`、3 `MODEL_VALUATION`、4 `CURVE_INTERPOLATION`。前 3 项可注册为外部 DataSource 属性；第 4 项只由内部计算路径产生。
- 新增 `ficant.market.v1.DataSourceDefinition` 与 `DataSourceRegistryService.RegisterDataSource / GetDataSource`。definition 携带 exact VersionRef、owner、transport kind、dataset / schema binding 与非空来源类型；注册保持 append-only version / idempotency 语义，查询只接受 exact version。
- `ficant.market.v1.FactSource` 保留字段 1–3，新增 `data_source = 4` exact VersionRef；不新增复制的来源类型字段。legacy payload 可读，缺字段不能进入 typed calculation。
- `ficant.research.v1.PortfolioKeyRateExposure` 保留字段 1–8，新增 `source_confidence = 9`；新增 `PriceSourceCount` 与 `PriceSourceSummary`，复用 market 的来源类型枚举。字段进入 content hash，数值 exposure / totals wire 不变。
- `CanonicalSnapshotDecoder` 的 transport-neutral projection 附带 verified manifest 的 exact DataSource ref；quote 列与 schema 不变。registered-futures materialization 在 RulePack parser / native engine 前通过 `DataSourceRepository` 解析并要求 `ACTIVE_QUOTE`，把 exact ref 加入 lineage。
- PostgreSQL forward-only migration 0021 给 `data.sources` 增加 nullable `price_source_type`，给 Quote / Trade / Valuation 增加成对 nullable exact DataSource id/version与 FK；不改 0001–0020、不回填 legacy 行。新 typed payload 使用可区分的 codec discriminator，旧 payload 继续按原 discriminator 解码。

## 5. 需 Human 决策

- **已裁决——拆轮次：** Human 批准 `R5a=AC15`、`R5b=AC35`、`R5c=AC36`，且 R5b / R5c 均依赖 R5a。R5a 不能预先点亮后两条。
- **已裁决——分层方案 B′：** Human 批准类型归 exact immutable DataSource version；dataset 必须同质；FactSource 携带 exact ref；canonical quote v1/schema/hash 不变；legacy 未标型对象在 typed calculation 中失败关闭；内部曲线插值由算法标记，不伪造 DataSource；混合只标记、不降级、不阻断。
- **已裁决——公共加法：** Human 已授权本轮所有相关问题直至 R5a 开发闭环，包括新增 DataSource registry 公共服务、FactSource tag 4、Portfolio result tag 9、migration 0021、对应 descriptor / generated output 与生产组合；该授权不包含 R5b/R5c、税制、发布或版本 tag。
- **已裁决——计数与 hash：** 来源分布采用 §2 第 7 条消费次数语义并纳入 portfolio hash。旧 R4d 数值不变，但 R5a 后相同数值结果的内容哈希因新增真实语义而有意改变；不得保留旧 hash 冒充完整内容。
- **设计冻结前读穿修正：** 按既定权威顺序重读 SPEC、ACCEPTANCE、ADR 与路线图时，发现 acceptance sentence 曾误写为注册四种 DataSource，与同一 brief §2 第 2 条及路线图 B′ 的“曲线插值只由内部算法标记”冲突。实现前已收窄为三个可注册外部类型加一个内部算法类型；§6 写路径、AC15 行为 claim 与 canonical 冻结事实均未改变。
- **最终审计的同源文字更正：** 最终候选审计发现 §2“类型可查”行仍残留“registry 注册四种”的旧文字，而 acceptance sentence、§2 第 2 条、公共契约、RED-first 判据和实现均已按 Human 批准的 B′ 固定为“三种外部可注册 + 一种内部算法产生”。本次只把该行归一到既有上位裁决，不改实现、测试 expected 或 AC15 claim；这不是用改判据制造通过。
- **执行期事前扩权——R4d-b domain 回归构造器：** Human 已在 R5a 实现开始前批准本轮所有相关问题持续执行至开发闭环。应用层 RED 证明 `PortfolioKeyRateExposure` 的既有 futures 构造器必须接收真实消费的 active-quote record count，才能让来源分布进入 domain 不变量与 content hash；因此在首次写入前把 `crates/ficant-domain/tests/r4d_b_futures_krd_contracts.rs` 窄扩为允许写路径。只允许给既有 futures portfolio 构造调用补该计数并断言来源汇总，不得修改既有数值 expected、算法断言或其他测试语义。§6 冻结清单保持原样，本记录不改写冻结约束。
- **authority 前置：** agent 不在公共候选中改 authority 三件套。公共候选独立审查并 rebase merge 后，authority 以新 public SHA 重冻；Human 逐条确认 AC15 与限定边界后才点亮，并同步 MANUAL。R5b 只能从新的双 main 冻结。

## 6. 最终真实测试证据

**双 base 冻结：** 2026-08-02 在公共 worktree `C:\git\ficant-r5a-source-confidence` 执行 fetch 后亲自确认工作区干净、`HEAD == origin/main == 0a7b75a337c74f93ca968cd4807682fae8da1385`，branch 为 `codex/r5a-source-confidence`。authority worktree `C:\git\ficant-authority-r5a-base` 同样干净、detached `HEAD == origin/main == 9234962ccee81f2503dc0bafa0ded26270fe58bc`。以上双 base 自此固定不变。

**冻结允许写路径（精确文件；本节自此不得就地改写）：**

- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/composition.rs`
- `binaries/ficant-server/tests/data_source_registry_sit.rs`（新建）
- `binaries/ficant-server/tests/portfolio_risk_sit.rs`
- `crates/ficant-api/src/canonical_snapshot.rs`
- `crates/ficant-api/src/data_source_registry.rs`（新建）
- `crates/ficant-api/src/grpc_web.rs`
- `crates/ficant-api/src/lib.rs`
- `crates/ficant-api/src/portfolio_risk.rs`
- `crates/ficant-api/tests/data_source_registry_service.rs`（新建）
- `crates/ficant-api/tests/phase2e_sdk_live.rs`
- `crates/ficant-api/tests/portfolio_risk_service.rs`
- `crates/ficant-api/tests/rates_service.rs`
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/ports/canonical_snapshot.rs`
- `crates/ficant-application/src/ports/data_sources.rs`
- `crates/ficant-application/src/ports/facts.rs`
- `crates/ficant-application/src/ports/fingerprint.rs`
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/ports/rule_pack_resolution.rs`
- `crates/ficant-application/src/use_cases/data_sources.rs`（新建）
- `crates/ficant-application/src/use_cases/futures_delivery.rs`
- `crates/ficant-application/src/use_cases/mod.rs`
- `crates/ficant-application/src/use_cases/portfolio_risk.rs`
- `crates/ficant-application/tests/data_source_port.rs`
- `crates/ficant-application/tests/futures_delivery_input_bindings.rs`
- `crates/ficant-application/tests/r4d_b_futures_krd_contracts.rs`
- `crates/ficant-application/tests/review_round5.rs`
- `crates/ficant-application/tests/rule_pack_effective_proof.rs`
- `crates/ficant-application/tests/unit_semantic_proof.rs`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contracts/src/generated/ficant.market.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.market.v1.tonic.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.rs`
- `crates/ficant-data/src/snapshot.rs`
- `crates/ficant-data/tests/snapshot_codec.rs`
- `crates/ficant-domain/src/market/data_source.rs`
- `crates/ficant-domain/src/market/mod.rs`
- `crates/ficant-domain/src/research/exposure.rs`
- `crates/ficant-domain/src/research/mod.rs`
- `crates/ficant-domain/tests/data_source_contracts.rs`
- `crates/ficant-domain/tests/r5a_source_confidence_contracts.rs`（新建）
- `crates/ficant-storage/src/postgres/codec.rs`
- `crates/ficant-storage/src/postgres/data_sources.rs`
- `crates/ficant-storage/src/postgres/facts.rs`
- `crates/ficant-storage/tests/data_source_registry_sit.rs`
- `crates/ficant-storage/tests/migration_acceptance.rs`
- `crates/ficant-storage/tests/postgres_repository.rs`
- `docs/iterations/2026-08-r5a-source-confidence.md`（新建）
- `docs/iterations/README.md`
- `interface/README.md`
- `interface/buf.gen.yaml`
- `interface/proto/ficant/market/v1/data_source.proto`（新建）
- `interface/proto/ficant/market/v1/fact.proto`
- `interface/proto/ficant/research/v1/exposure.proto`
- `migrations/postgresql/0021_price_source_confidence.sql`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/data_source_pb2.py`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/data_source_pb2_grpc.py`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/fact_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/exposure_pb2.py`
- `web-dm/packages/contracts-generated/src/ficant/market/v1/data_source_pb.ts`（新建）
- `web-dm/packages/contracts-generated/src/ficant/market/v1/fact_pb.ts`
- `web-dm/packages/contracts-generated/src/ficant/research/v1/exposure_pb.ts`

**禁止写路径：** 所有未逐项列出的路径。特别禁止 authority 三件套与本地废副本、所有 ADR、`README.md`、既有 brief、`docs/architecture/layering-refactor.md`、除三个点名文件外的 proto / generated output、`crates/ficant-data/src/canonical.rs`、`crates/ficant-api/src/rates.rs`、C++ / C ABI / native crates、`domain-packs/**`、`scripts/**`、`tests/golden-cases/**`、`tests/oracle/**`、`tests/phase2c/**`、`tests/phase2d/**`、`Cargo.lock`、`.gitignore`、`.github/**`、`cicd.yml` 与 `deploy/**`。扩权只能由 Human 在首次写入前批准并新增 §5 记录；本节不得追认。

**受保护 base 事实（Git object ID，实施期必须保持不变）：**

- `scripts/layering-allowlist.json`：blob `fe51488c7066f6687ef680d6bfaa4f7768ef205c`，内容 `[]`
- `crates/ficant-data/src/canonical.rs`：blob `79e42b00c645710b8179d515ba02f79cd9d38fc4`
- `tests/golden-cases`：tree `11f981972612e617591de1c3daaa36d114a7cab9`
- `tests/oracle`：tree `539889f598c8118854ea679375695c9721696932`
- `tests/phase2c/acceptance-matrix.json`：blob `26e72186490a0ab2cae142c9d88436ae07cc8da8`
- `tests/phase2d/acceptance-matrix.json`：blob `d6feaed93a8df00176f2873d28d1e03d6d789f75`
- `cpp`：tree `e600f8de0a485d5db5edf7eac20e5ea89698716f`
- `crates/ficant-kernel-sys`：tree `3350c80bdc3c54159fa9b4e6bb4e26ca33218f0d`
- `crates/ficant-fixed-income-native`：tree `3e622b8a1e4786a8183530d63a4d3d41be8a953b`
- `interface/proto/ficant/rates/v1/analytics.proto`：blob `ae49a0b44959f7ec42b2639ae4a5fd29ece94335`
- `interface/proto/ficant/research/v1/position.proto`：blob `35ca895737cc0a19168fae0959b4dc7dac618acf`
- `interface/proto/ficant/research/v1/factor.proto`：blob `ed998a2a142836e8eb17e8861e0bbf1fc3bad1ac`
- `domain-packs/cgb-futures/cgb-futures-v1.json`：blob `1fe9db105d15f2f3924b8f488108311611ca7f07`
- `domain-packs/cgb-futures/cgb-futures-v1.bin`：blob `469445e4199020dae0a705be42a0569e72a73f05`
- `domain-packs/cgb-futures/cgb-futures-v2.json`：blob `6fbbc8ec9b38b90dcbeeebc1d776838098873268`
- `domain-packs/cgb-futures/cgb-futures-v2.bin`：blob `054ac57bdde54b3349adecf564ee10489b2efb21`

**RED-first 与 forward-only checkpoint：** 三组 RED 均在实现前取得，RED 本身未作为 checkpoint：

- domain / protobuf：`cargo test --offline --locked -p ficant-domain --test r5a_source_confidence_contracts` 首次 exit 101，首个真实错误为缺少 `PriceSourceType`、`PriceSourceCount`、`PriceSourceSummary` 及对应构造 / accessor；注入固定 Buf 1.56.0 后的 `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory` 同样 exit 101，首个真实错误为 descriptor 中不存在 `ficant.market.v1.PriceSourceType`。随后 domain `data_source_contracts` 2/2、`r5a_source_confidence_contracts` 3/3、既有 R4d-b domain 回归 3/3 与 descriptor 18/18 转绿，形成 contract checkpoint。
- application：`cargo test --offline --locked -p ficant-application --test r4d_b_futures_krd_contracts` 首次 exit 101，首个真实错误为缺少携带 exact DataSource ref 的 `DecodedCanonicalQuotes`、portfolio 构造器的实际 quote 消费计数和结果 `source_confidence`。随后该测试 6/6、`futures_delivery_input_bindings` 3/3、`data_source_port` 2/2 转绿，形成 application checkpoint；五个 missing / untyped / wrong type / owner / version 负向 fixture 均证明 parser、delivery、curve、bond 调用数为 0。
- transport / persistence / production composition：`cargo test --offline --locked -p ficant-api --test data_source_registry_service` 首次 exit 101，首个真实错误来自公共加法尚未贯通：既有 portfolio 生产构造缺少 `DataSourceRepository` 参数且结果缺少 `source_confidence`。随后 API registry 1/1、API portfolio 3/3、storage registry 1/1、typed Quote exact-ref round-trip 1/1、migration 4/4、server registry 1/1 与 server portfolio 2/2 转绿，形成 storage / transport / production checkpoint。
- GREEN 过程中还遇到并如实修复了纯夹具 / 编译问题：两个测试 ULID 含非法字符、migration helper 误作 `const fn`、typed quote fixture 的 move/borrow、server fixture 的 native digest 不匹配，以及严格 Clippy 的 rustdoc 反引号 / 单分支 match；均未修改 expected、Oracle、Golden、matrix、canonical、allowlist、数值算法或冻结语义。

**最终针对性命令（同一候选逐条执行并填真实结果）：**

- `cargo test --offline --locked -p ficant-domain --test data_source_contracts`
- `cargo test --offline --locked -p ficant-domain --test r5a_source_confidence_contracts`
- `cargo test --offline --locked -p ficant-application --test data_source_port`
- `cargo test --offline --locked -p ficant-application --test futures_delivery_input_bindings`
- `cargo test --offline --locked -p ficant-application --test r4d_b_futures_krd_contracts`
- `cargo test --offline --locked -p ficant-storage --test data_source_registry_sit`
- `cargo test --offline --locked -p ficant-storage --test postgres_repository`
- `cargo test --offline --locked -p ficant-storage --test migration_acceptance`（必须完整 4/4）
- `cargo test --offline --locked -p ficant-api --test data_source_registry_service`
- `cargo test --offline --locked -p ficant-api --test portfolio_risk_service`
- 注入固定 Buf 1.56.0 后 `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory`
- `cargo test --offline --locked -p ficant-server --test composition`
- `cargo test --offline --locked -p ficant-server --test data_source_registry_sit`
- `cargo test --offline --locked -p ficant-server --test portfolio_risk_sit`
- `pwsh -NoProfile -NonInteractive -File scripts/check-layering.ps1`
- `pwsh -NoProfile -NonInteractive -File scripts/test-layering-check.ps1`

**同一最终候选的针对性结果：**

| 命令组 | 真实结果 |
|---|---|
| domain | `data_source_contracts` exit 0，2/2；`r5a_source_confidence_contracts` exit 0，3/3 |
| application | `data_source_port` exit 0，2/2；`futures_delivery_input_bindings` exit 0，3/3；`r4d_b_futures_krd_contracts` exit 0，6/6 |
| storage | `data_source_registry_sit` exit 0，1/1；`postgres_repository` 在 `RUST_TEST_THREADS=1`（与仓库数据库集成入口相同的共享库串行约定）下 exit 0，13/13；`migration_acceptance` 同约定 exit 0，4/4 |
| API / descriptor | `data_source_registry_service` exit 0，1/1；`portfolio_risk_service` exit 0，3/3；固定 Buf descriptor exit 0，18/18 |
| production server | `composition` exit 0，3/3；`data_source_registry_sit` exit 0，1/1（启动真实 gRPC-Web mux，并由生成客户端命中 registry 路径取得 typed error）；`portfolio_risk_sit` exit 0，2/2 |
| layering | 主门禁 exit 0：`AC03=0`、`AC01=0`、C++/FFI=0、Funding=0、Tax=0、allowlist=0；负向 fixture exit 0，51 assertions |

`postgres_repository` 曾在默认并发调度下 exit 101（2 passed / 11 failed），首个错误为另一个并发测试在共享 PostgreSQL schema 中抢先写入导致 `AlreadyExists`，其余失败同样表现为跨测试 reset / count 串扰；没有据此改实现或断言。按仓库所有数据库集成入口的既有 `--test-threads=1` 约定串行重跑后 13/13，并在最终候选再次取得相同结果。

**完整本地检查（最终候选必须真实执行）：**

- `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`
- 锁定 Node 22.17.0、pnpm 10.12.4、Buf 1.56.0、uv / Python 3.12 后 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1`
- 从 Windows User scope 导入六个 `FICANT_TEST_*` 值且不输出值后，使用同一锁定工具链运行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration`

**完整检查真实结果：** 工具链为 rustc / cargo 1.96.1、Node 22.17.0、pnpm 10.12.4、Buf 1.56.0、uv 0.7.13、Python 3.12.11。

- `scripts/check-fast.ps1` exit 0。
- `scripts/check.ps1` 最终 exit 0：51 条分层 fixture、严格 Clippy、workspace build / tests、descriptor 18/18、C++ 8/8、Q-001..Q-036、Phase 2B / 2C / 2D matrix 与两组 Decimal Oracle、Phase 2E live SDK、canonical / snapshot、Web typecheck / build / 35 tests 全部通过。首次完整运行在严格 Clippy 因新增 rustdoc 缺反引号 exit 101；机械修复后 Clippy 单独 exit 0。第二次运行到 Web 因本 worktree 缺被忽略的 `node_modules` exit 1；随后 `corepack pnpm@10.12.4 install --offline --frozen-lockfile` exit 0（178 packages 全部 reused、downloaded 0、lockfile 未变），Web 三项单独通过，第三次完整运行取得上述 exit 0。
- `scripts/check.ps1 -IncludeIntegration` exit 0；六个 `FICANT_TEST_*` 从 Windows User scope 导入且值未输出。除完整非环境检查外，migration 4/4、lease queue 1/1、execution closure 3/3、production worker 1/1、Phase 1 1/1、negative invariants 13/13、Phase 2B / 2C / 2D 各 1/1、Phase 3A registry / parity 各 1/1、Phase 3B codec 3/3 与 publication 1/1 全部通过。

**changed-path 与 protected-object 审计：** 以冻结 execution base 对工作树和 untracked 文件取并集，共 56 个实际变更路径；冻结 §6 清单加 §5 事前单文件扩权共 64 个允许路径，越界 0，另有 8 个允许但未使用路径。`git diff --check` exit 0。三个 proto 变更精确为新增 `data_source.proto`、加法修改 `fact.proto` 与 `exposure.proto`；position / factor / health / constraint / policy 点名禁止 proto 变更数为 0，`Cargo.lock` 未变。

§6 冻结的 16 个 protected blob / tree 已逐项执行 base OID、working-tree diff 与 untracked 复核，全部保持原 OID且 diff / untracked 为 0：allowlist 仍为 `[]`，canonical.rs、Golden、Oracle、Phase 2C / 2D matrix、C++、kernel-sys、fixed-income-native、三个受保护 proto 与两版 CGB pack 均未变化。全量 matrix 入口同时报告 frozen assets unchanged。

**Acceptance sentence：成立。** 三种外部 DataSource 类型均可经生产 registry 注册并 exact 查询，内部曲线证据单独标为 `CURVE_INTERPOLATION`；bond-only 结果为单一来源且 `mixed=false`，加入 concrete futures 与 verified quote snapshot 后同时出现 `ACTIVE_QUOTE` / `CURVE_INTERPOLATION` 且 `mixed=true`。应用层 6/6 在响应外逐项重核 position / total Decimal 与 UnitRef，证明数值与加标前口径逐位相同；来源汇总和 exact ref 分别进入 content hash 与 lineage。missing / legacy untyped / wrong type / owner / version 及 protobuf enum 越界均在规定的数值与 repository write 边界前失败关闭。R5a 本地自测候选满足 AC15 的可执行判据；authority 点亮仍严格等待公共提交 rebase merge 后的独立绑定。

## 7. 残余风险

- B′ 要求一个 DataSource exact version 内数据集同质。若真实供应商文件混排成交、报价或估值，R5a 会拒绝或要求上游拆源；支持记录级覆盖必须另开 canonical v2 迭代，不能把类型猜进 v1。
- legacy 未标型 DataSource / FactSource 继续可读以支持迁移，但 typed calculation 必须失败。这是显式兼容边界，不是历史数据已具可信度；后续若补录只能发布新 immutable version或经独立、可审计的数据迁移授权。
- `CURVE_INTERPOLATION` 表示内部价格形成路径，不代表曲线输入本身健康或完整。R5b 的 CoverageDeclaration 与 R5c 的 DataHealthReport 仍需分别说明覆盖与健康，不得从 R5a 标记推导质量结论。
- 本轮对来源类型只做分类和混合标记，没有全局可信度序。若以后要排序、替换来源或阻断混合，必须冻结独立 Policy / algorithm identity，不能无痕改变 AC15 输出。
- `FactSource.data_source` 为将来 MarketFactService 实现提供 exact ref，但本轮不实现该 dormant transport。AC15 的生产可查证据由 DataSource registry 与现有 verified canonical / portfolio-risk 路径承担；后续启用 MarketFactService 时仍须复用同一 typed resolver，不能绕过。
