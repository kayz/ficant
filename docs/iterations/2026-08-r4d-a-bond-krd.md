# R4d-a 迭代 brief — 可验证风险输入与债券 KRD

**迭代：** R4d-a · **承接条目：** AC16 前置，本轮不点亮 · **execution base：** `4e472a8993b5d2a5c4a5c69bf078c9659d19e2de` · **authority base：** `d99680a5551d07e8740907c88fa996df2ba499eb`

本 brief 是 R4d-a 面向 Human 的唯一设计。Human 已批准把原 R4d 拆为严格顺序的 `R4d-a → R4d-b`：本轮补齐注册 Bond 定价条款、双时间且可验证的曲线点载荷、Factor convention 的可执行语义，以及债券逐仓位 KRD 与债券子组合聚合；任何纳入敞口的非债券仓位均失败关闭。R4d-b 才复用 R4a 的具体期货 / CTD materializer，加入期货 KRD 并点亮 AC16。本文冻结设计、测试和逐文件写路径；Human 已授权在该边界内完成本地自测候选，但未授权 commit、push、Pull Request、merge、authority binding 或发布。

## 1. 目标

新增独立的 `PortfolioRiskService.CalculateKeyRateDv01`。调用方只提交精确 PositionSnapshot、CurveSnapshot、查询时点和经 registry 验证的结果 UnitRef；服务端解析 Position、Instrument / Bond / Calendar definition、verified curve-point blob、CurveNodeDefinition、FactorDefinition 与双向 binding，按 FactorDefinition 中冻结的 bump / direction / rebuild / second-order convention 调用既有线性 YTM curve primitive 与 fixed-rate bond native engine，生成逐债券仓位 × FactorId 的 KRD 和债券子组合机械求和结果。

R4d-a 不接受调用方提交 `BondTerms`、曲线点、收益率、价格、DV01、KRD、权重或 Factor convention；也不修补现有 Rates 请求的历史行为。它只支持已注册完整定价条款的 fixed-rate annual / semiannual Bond。PositionSnapshot 中只要存在一个 `included_in_position_exposure == true` 的 Futures 或其他非债券仓位，整次请求就在任何重定价前失败，不返回 partial portfolio。

**Acceptance sentence：**

> 注册两只完整 fixed-rate Bond、对应 Instrument / Calendar / Unit definition、三个稳定 CurveNodeDefinition 及其 FactorDefinition / binding；发布一份 `as_of == valuation_at`、`visible_at <= knowledge_at` 且 blob hash / size / canonical bytes 全部验证的 CurveSnapshot，并用同 owner / subject 的精确 PositionSnapshot 持有两只债券。一次 `CalculateKeyRateDv01` 返回稳定排序的两项逐仓位三因子 KRD 和三项债券子组合总值，每个总值与同 FactorId 的逐仓位 exact Decimal / UnitRef 之和完全相等；改变任一 Factor bump 或 direction 会按冻结公式改变结果。删除或漂移任一 Position、Instrument / Bond / Calendar / Unit、curve point、Factor / CurveNode definition 或 binding，改变 owner / version / hash / 时点 / 单位，传入不完整或不支持 Bond，或加入任一纳入敞口的非 Bond 仓位，均在返回任何 partial exposure 前失败关闭。调用方不能提交现成风险数值；既有 Rates 结果、C++ / C ABI、Golden、Oracle、canonical Arrow schema/hash、Phase 2C/2D matrix 与 allowlist 均不变。

## 2. 验收

| 条目 | R4d-a 可执行判据 |
|---|---|
| AC16 前置 | 至少两个有符号债券名义仓位和三个 stable FactorId；服务返回完整 position × factor 向量及 bond-only totals。测试必须在响应外按相同 UnitRef 重算每个 total 并 exact 相等；至少两个相邻期限为非零，改 Factor bump / direction 后按公式改变。响应与 MANUAL 只能称“债券子组合 KRD”，不得把本轮记为 AC16 通过。 |
| 非债券关闭 | 在同一 PositionSnapshot 增加一项 `included_in_position_exposure == true` 的 exact FuturesContract、普通 Instrument 或其他 subtype，必须在 curve interpolation 与 bond engine call count 均为 0 时整单失败；逆回购质押券仍按 R4b 规则排除，不触发 unsupported。 |
| 输入完整性 | 缺或改任一 Bond pricing term、Calendar business-day、CurveSnapshot visible/family/schema/hash/size、curve point / node hash、FactorDefinition / binding、Unit dimension、owner / version / knowledge boundary 时不返回 partial result。blob 缺失、hash / size / canonical re-encode 不符必须产生既有 IntegrityEvent。 |

R4d-a 闸门：

1. RED-first 分三次取得：domain pricing / curve / exposure contract；application materializer / aggregation contract；transport route / rejection contract。每次先只加判据并取得非零 exit code；RED 不是 checkpoint。domain、application、storage、transport 分别在直接测试通过后才成为 forward-only checkpoint。
2. `Bond` 字段 1–8 保持原号；新增定价字段只用 9–12。legacy payload 仍可读取，但没有全部定价字段的 Bond 不能进入 R4d-a。新增 enum 的 `UNSPECIFIED = 0` 一律拒绝。
3. `CurveSnapshot.as_of` 必须与 `valuation_at` 的完整 `MarketTime` 精确相等；`visible_at <= knowledge_at`，且 PositionSnapshot 也必须在同一 `knowledge_at` 可见。不得以最新 CurveSnapshot、日期相等、时区转换后的近似时点或调用方节点替代。
4. curve-point blob 只接受冻结 schema `ficant.yield-curve-points.protobuf.v1`。points 按 `curve_node_id` 严格升序且唯一；metadata family、payload family、CurveNodeDefinition family / tenor / hash、Factor binding 和 yield UnitRef 全部一致。decoder 必须拒绝 unknown fields、非 canonical 顺序、重复项和 decode 后 re-encode 不等于原 bytes 的载荷。
5. 本轮算法固定为既有 linear-YTM interpolation 加既有 fixed-rate bond native engine；不得新增或修改 C++ / C ABI。对每个 Factor 只 bump 对应 canonical yield node，用完整节点集重新插值 Bond maturity YTM，并按 direction 运行 base / up / down 所需重定价。
6. 公式固定为：CENTRAL=`(P_down - P_up) / (2 × bump_bp)`；UP=`(P_base - P_up) / bump_bp`；DOWN=`(P_down - P_base) / bump_bp`，均归一为每 1 bp。仅支持 `REBUILD + EXCLUDE`；`HOLD` 或 `INCLUDE` 失败关闭，不能解释为零或忽略。结算日等于 `valuation_at.local_trading_date`，且必须是 exact Calendar 的 open business day。
7. curve snapshot 每个 node 的 exact CurveNode target → FactorDefinition 定义本次完整因子轴。每个仓位稳定返回完整轴，包括 exact zero；任何非零 KRD 还必须存在该 Instrument target → Factor binding，否则整单失败。额外静态 Instrument binding 不凭空产生数值，不属于本次 curve family 的 Factor 不进入结果。
8. Bond `Position.quantity` 必须是有符号、非零、dimension=`notional` 且 UnitRef 与 Instrument currency 一致的名义面额；逐仓位 KRD=`每 100 面额 KRD × quantity / 100`。请求的 `dv01_unit` 必须解析为 registry 中 exact dimension=`dv01` 的 Unit；所有逐仓位值与 totals 使用同一 UnitRef 和 exact Decimal。跨币种、单位 scale / precision 不兼容或隐式 1:1 换算全部失败。
9. 每个 Position exposure content hash / lineage 覆盖 PositionSnapshot / Position、Instrument / Bond / Calendar、CurveSnapshot / verified point payload、CurveNodeDefinition / FactorDefinition / 双向 binding、Unit 与算法 identity；portfolio hash 还覆盖完整、稳定排序的 position exposure hash 集合。相同输入重跑必须逐字节同值。
10. 新 protobuf service 只能扩大 descriptor / gRPC-Web / production service inventory，不能改变既有方法、tag 或语义。`RatesAnalyticsService`、`AnalyzeBond`、`YieldCurveBinding` 和 `AnalyzeFuturesHedge` 均不在写路径；当前 `rates.rs` 对 day-count / business-day 的历史硬编码只记录为债务，本轮不得顺手修复。
11. 不得修改 expected、Oracle、断言、容差、Golden、Phase 2C/2D matrix、guarded hash、selector、command、canonical Arrow schema/hash、`scripts/layering-allowlist.json` 或分层门禁断言。allowlist 必须保持 `[]`。任何冻结清单外路径在首次写入前停止并取得 Human 明确授权，不得编辑本节追认。

## 3. 非目标

- R4d-b 的期货逐仓位 KRD、CTD / conversion-factor 冲击、债券 + 期货全组合聚合及 AC16 点亮；两轮不得并行。
- 调用方提交 `BondTerms`、curve nodes、Factor convention、价格、收益率、DV01、KRD、权重或 totals；现有 Rates 输入不得成为可信捷径。
- Cashflow schedule 双时间化、逐现金流 zero-curve discounting、浮息、不规则、含权、信用、税后或二阶风险；本轮是 curve-implied maturity YTM 的 fixed-rate 一期算法。
- 多币种 / FX、partial coverage、CoverageDeclaration、DataHealthReport、scenario、VaR、PnL explain、Constraint 或 ShadowPrice。
- 新市场、市场 / symbol / 产品条件分支、连续合约、自动 Factor 发现或修改 R4c FactorId / CurveNode wire shape。
- 修改 `analytics.proto`、`position.proto`、`factor.proto`、既有 Rates / R4a–R4c 行为、C++ / C ABI、`ficant-data/src/canonical.rs`、Golden、Oracle、matrix、allowlist、scripts、authority 三件套、任何 ADR、`.github/**`、`cicd.yml` 或 `deploy/**`。

## 4. 公共契约变化

- `ficant.market.v1.Bond` 保留字段 1–8；新增 `coupon_rate = 9`、`coupon_frequency = 10`、`day_count = 11`、`business_day = 12`。新增 `BondCouponFrequency { UNSPECIFIED=0, ANNUAL=1, SEMIANNUAL=2 }`、`BondDayCountConvention { UNSPECIFIED=0, ACT_ACT_BOND_ISMA=1 }`、`BondBusinessDayConvention { UNSPECIFIED=0, FOLLOWING=1 }`。完整四项进入 definition fingerprint、PostgreSQL payload / normalized all-or-none columns和 R4d lineage；旧 definition 仍可读取但风险请求拒绝。
- `ficant.market.v1.CurveSnapshot` 新增 `visible_at = 11` 与 `curve_family_id = 12`。新增 blob message `CurvePoint { curve_node_id=1, curve_node_content_hash=2, yield_to_maturity=3 }` 与 `CurvePointSet { curve_family_id=1, points=2 }`。新发布物必须使用 `ficant.yield-curve-points.protobuf.v1`；既有缺字段 snapshot 仍可读取 metadata，但不能进入 R4d-a。
- 新增 `ficant.research.v1.exposure.proto` 与 `PortfolioRiskService.CalculateKeyRateDv01`。请求固定为 `position_snapshot_id=1`、`knowledge_at=2`、`valuation_at=3`、`curve_snapshot_id=4`、`dv01_unit=5`；R4d-a 不提前加入仅供 R4d-b 使用的 DataSnapshot / CTD 字段。
- `RiskAlgorithmBinding` 固定 `algorithm_id=1`、`algorithm_version=2`、`convention_profile=3`。`FactorDv01` 固定 `factor_id=1`、`factor_definition_hash=2`、`dv01=3`。`PositionKeyRateExposure` 固定 `position_id=1`、`instrument=2`、`exposures=3`、`content_hash=4`、`lineage=5`。`PortfolioKeyRateExposure` 固定 `position_snapshot_id=1`、`curve_snapshot_id=2`、`positions=3`、`totals=4`、`algorithm=5`、`content_hash=6`、`lineage=7`。响应使用 `oneof { exposure=1, ErrorDetail error=2 }`。
- service 使用既有 `rates:analyze` scope 与 trusted AccessScope，不新增角色或市场权限。结果 algorithm identity 固定为 `ficant.fixed-rate-bond.key-rate-yield` version 1、convention profile `linear-ytm-registered-bond-v1`；调用方不能选择或覆盖。
- application 新增 transport-neutral `CurvePointSetDecoder` port；API 的 protobuf adapter 执行 decode、unknown-field / canonical re-encode 检查并投影 domain values。application / domain 不依赖 generated contract，`ficant-data` canonical Arrow schema 不变。
- PostgreSQL forward-only migration 0019 为 Bond pricing terms 与 CurveSnapshot visible/family 增加 nullable、all-or-none 约束；不改写 0001–0018。新写入必须完整，legacy row 保留但 R4d-a fail-closed。required verified-read 增加 `CurveSnapshot / CurvePoints` resource-role pair及 IntegrityEvent 名称。

## 5. 需 Human 决策

- **已裁决——拆分：** Human 批准 `R4d-a → R4d-b`，不建 umbrella brief。R4d-a 只交付完整 Bond 定价定义、verified curve points、逐债券 KRD 与债券子组合 totals，不点亮 AC16；R4d-b 在 R4d-a 公共与 authority 闭环后加入具体期货 KRD 和全组合聚合，才申请 AC16。
- **已裁决——Bond Definition：** 落实 R4a §5 / §7 已记录的后续债券契约债务，字段 9–12 如 §4。现有 Cashflow 缺 `observed_at / visible_at` 和可证明 schedule 完整性的总数，不作为 R4d-a 事实源。`rates.rs::parse_bond_terms` 写死 `ActActBondIsma / Following` 而 R4a brief 曾称二者由请求提供，这是继承的记录 / 契约不一致；R4d-a 不复制也不顺手修复它。
- **已裁决——曲线双时间与载荷：** `CurveSnapshot.as_of == valuation_at` 完整精确相等，`visible_at <= knowledge_at`；同一 `as_of` 的更晚 `visible_at` 可表示修订视图。payload 只含 stable node id / hash + YTM，节点日期由 `as_of + canonical tenor` 生成，不能由调用方给出。
- **已裁决——首个算法：** 采用 §2 的 linear-YTM + fixed-rate native engine、三种 direction 公式、`REBUILD + EXCLUDE`、same-day open-calendar settlement；`HOLD / INCLUDE` 失败关闭。R4d-a 不改 C++ / C ABI。
- **已裁决——数量与单位：** Bond quantity 是 dimension=`notional` 的有符号名义面额；结果为 exact dimension=`dv01`；按每 100 面额缩放。仓库既有 Unit registry / AnalysisUnits 已能表达这些通用 dimension，不新增市场单位。
- **已裁决——Factor axis：** CurveSnapshot node bindings 定义完整轴并返回 zero；非零 exposure 必须有 Instrument → Factor binding。静态 topology 不等于权重，额外 binding 不产生数值。
- **已裁决——R4d-b 期货边界：** 后续每次请求先用 R4a verified DataSnapshot / RulePack 取得 base CTD 与 conversion factor，shock 内固定二者并重定价 CTD Bond；不处理 CTD switching。本裁定不授权 R4d-a 写任何期货实现路径。
- **已裁决——完整性与币种：** R4d-a 遇到任何纳入敞口的非 Bond 或跨币种仓位整单失败；不得用 partial coverage 冒充 AC16。R4d-b 也只有全部 Bond / Futures 仓位可内部重定价时才成功。
- **已批准的 ADR-0015 建议 diff，暂不写 ADR：** 在 §四补充：“一期关键期限 DV01 由 FactorDefinition 冻结的 bump / direction / rebuild / second-order convention 驱动；REBUILD 表示冲击一个已注册曲线节点后用同一完整节点集和冻结算法重新插值并重定价，HOLD 不等价于忽略重建，INCLUDE 不等价于在一阶结果中静默加入二阶项；实现不支持的 convention 必须失败关闭。调用方不得选择或覆盖 convention。”ADR 不在本轮写路径；由 Human 决定单独修订时点。
- **实现授权状态：** Human 在逐文件写路径冻结后明确授权开始实现，并进一步授权在冻结目标、公共契约、非目标与写路径内持续自主执行到完整本地自测候选；普通编译 / 测试失败和机械修复无需中途请求确认。该授权不包含 commit、push、Pull Request、merge、authority binding 或发布。
- **authority 前置：** R4d-a 不点亮 AC；但其公共候选合并后仍须以新的 authority main 精确绑定公共提交并同步 MANUAL 的“债券子组合、非全组合”边界，完成后才能冻结 R4d-b 双 base。agent 不改 authority 三件套。

## 6. 最终真实测试证据

**冻结状态：** 冻结前把设计草案临时 stash，使公共 worktree 真正干净；随后 `git fetch --prune origin` 并亲自确认 `HEAD == origin/main == 4e472a8993b5d2a5c4a5c69bf078c9659d19e2de`。authority worktree `C:\git\ficant-authority-r4d-base-v2` 同样先确认干净、fetch 后 `HEAD == origin/main == d99680a5551d07e8740907c88fa996df2ba499eb`；`verify-authority.ps1 -ExpectedAuthorityCommit d99680a5551d07e8740907c88fa996df2ba499eb` exit 0，manifest 精确绑定公共 base。草案随后原样恢复。以上双 base 自此固定不变。

**冻结允许写路径（实现授权后也只能逐项写入）：**

- `binaries/ficant-server/src/integrity_event.rs`
- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/composition.rs`
- `binaries/ficant-server/tests/portfolio_risk_sit.rs`（新建）
- `crates/ficant-api/src/curve_points.rs`（新建）
- `crates/ficant-api/src/grpc_web.rs`
- `crates/ficant-api/src/lib.rs`
- `crates/ficant-api/src/portfolio_risk.rs`（新建）
- `crates/ficant-api/tests/factor_registry_service.rs`
- `crates/ficant-api/tests/grpc_web_boundary.rs`
- `crates/ficant-api/tests/portfolio_risk_service.rs`（新建）
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/ports/curve_points.rs`（新建）
- `crates/ficant-application/src/ports/facts.rs`
- `crates/ficant-application/src/ports/factor_topology.rs`
- `crates/ficant-application/src/ports/fingerprint.rs`
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/ports/required_reads.rs`
- `crates/ficant-application/src/use_cases/factor_topology.rs`
- `crates/ficant-application/src/use_cases/mod.rs`
- `crates/ficant-application/src/use_cases/portfolio_risk.rs`（新建）
- `crates/ficant-application/src/use_cases/verified_reads.rs`
- `crates/ficant-application/tests/curve_snapshot_port.rs`
- `crates/ficant-application/tests/definition_aggregate.rs`
- `crates/ficant-application/tests/factor_topology_contracts.rs`
- `crates/ficant-application/tests/r4d_a_bond_krd_contracts.rs`（新建）
- `crates/ficant-application/tests/required_verified_reads.rs`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contracts/src/generated/ficant.market.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.tonic.rs`
- `crates/ficant-domain/src/key_rate.rs`（新建）
- `crates/ficant-domain/src/lib.rs`
- `crates/ficant-domain/src/market/bond.rs`
- `crates/ficant-domain/src/market/curve_snapshot.rs`
- `crates/ficant-domain/src/market/mod.rs`
- `crates/ficant-domain/src/research/exposure.rs`（新建）
- `crates/ficant-domain/src/research/mod.rs`
- `crates/ficant-domain/tests/r4d_a_bond_krd_contracts.rs`（新建）
- `crates/ficant-storage/src/postgres/codec.rs`
- `crates/ficant-storage/src/postgres/definitions.rs`
- `crates/ficant-storage/src/postgres/factor_topology.rs`
- `crates/ficant-storage/src/postgres/facts.rs`
- `crates/ficant-storage/src/s3/staging.rs`
- `crates/ficant-storage/tests/factor_topology_postgres.rs`
- `crates/ficant-storage/tests/migration_acceptance.rs`
- `crates/ficant-storage/tests/r4d_a_bond_krd_postgres.rs`（新建）
- `docs/architecture/layering-refactor.md`
- `docs/iterations/2026-08-r4d-a-bond-krd.md`（新建）
- `docs/iterations/README.md`
- `interface/buf.gen.yaml`
- `interface/proto/ficant/market/v1/definition.proto`
- `interface/proto/ficant/market/v1/fact.proto`
- `interface/proto/ficant/research/v1/exposure.proto`（新建）
- `interface/README.md`
- `migrations/postgresql/0019_r4d_a_bond_curve_inputs.sql`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/definition_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/fact_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/exposure_pb2.py`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/exposure_pb2_grpc.py`（新建）
- `web-dm/packages/contracts-generated/src/ficant/market/v1/definition_pb.ts`
- `web-dm/packages/contracts-generated/src/ficant/market/v1/fact_pb.ts`
- `web-dm/packages/contracts-generated/src/ficant/research/v1/exposure_pb.ts`（新建）

**禁止写路径：** 所有未逐项列出的路径。特别禁止 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、所有 ADR、`README.md`、既有 R1–R4c brief、`analytics.proto`、`position.proto`、`factor.proto`、其他 `.proto` / generated output、`crates/ficant-api/src/rates.rs`、`crates/ficant-domain/src/analytics.rs`、`crates/ficant-domain/src/curves.rs`、`crates/ficant-domain/src/futures_hedge.rs`、`crates/ficant-fixed-income-native/**`、`crates/ficant-kernel-sys/**`、`cpp/**`、`domain-packs/**`、`scripts/**`、`tests/golden-cases/**`、`tests/oracle/**`、`tests/phase2c/**`、`tests/phase2d/**`、`crates/ficant-data/src/canonical.rs`、`Cargo.lock`、`.gitignore`、`.github/**`、`cicd.yml` 与 `deploy/**`。本清单随 base 冻结；扩权只能由 Human 在首次写入前批准并新增 §5 记录，本节不得就地改写。

**受保护 base 事实（Git object ID，实施期只能保持不变）：**

- `scripts/layering-allowlist.json`：blob `fe51488c7066f6687ef680d6bfaa4f7768ef205c`，内容为 `[]`
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
- `crates/ficant-api/src/rates.rs`：blob `3d6249b0c317a225aa2a6c6af2893b1d6aaa9930`

**RED-first 与 forward-only checkpoint：**

- domain 判据先落地；`cargo test --offline --locked -p ficant-domain --test r4d_a_bond_krd_contracts` 非零退出（exit 1），首个真实错误是新增 Bond pricing / CurveSnapshot / exposure API 不存在。实现后同一命令 3/3，通过，形成 domain checkpoint。
- application 判据随后单独落地；`cargo test --offline --locked -p ficant-application --test r4d_a_bond_krd_contracts` 非零退出（exit 1），首个真实错误是 `CalculateBondKeyRateDv01` materializer / ports 不存在。实现后同一命令 2/2，通过，形成 application checkpoint。
- public contract 判据再单独落地；`cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory` 非零退出（exit 1），首个真实错误是 `ficant.market.v1.CurvePoint` 与新 service descriptor 不存在。fixture 最初把 CurvePoint 写成 research package，首次编译前按已冻结 §4 机械更正为 market package；未改 expected 语义。实现后 descriptor 17/17，API 2/2，production route 1/1，通过，形成 storage / transport checkpoint。RED 本身均未作为 checkpoint。

**最终同一候选的针对性证据（全部 exit 0）：**

- `cargo test --offline --locked -p ficant-domain --test r4d_a_bond_krd_contracts`：3/3。
- `cargo test --offline --locked -p ficant-application --test r4d_a_bond_krd_contracts`：3/3；两只 Bond × 三个 Factor 的完整向量与 exact totals 成立，bump / direction 改变结果；加入纳入敞口的非 Bond 时 curve 与 bond engine call count 均为 0。Root 终态静态复核另发现 UnitRef 已校验但 registry scale / precision 尚未承重，先只加入低精度 DV01、低 precision notional 与错误 rate dimension 三类负向 fixture，同一命令 exit 1（2/3，低精度 DV01 被错误接受）；随后在 application 边界补齐 exact owner / dimension / scale / precision 校验并把 rate Unit definition hash 纳入逐仓位 lineage，重跑 3/3。该 RED 不是 checkpoint，未修改既有 expected。
- `cargo test --offline --locked -p ficant-application --test required_verified_reads`：8/8。
- 注入 Windows User 级 `FICANT_TEST_DATABASE_URL` 与 Ceph/S3 变量后，`cargo test --offline --locked -p ficant-storage --test r4d_a_bond_krd_postgres`：1/1；覆盖 PostgreSQL definition / fact round-trip、Ceph verified curve-point read 与 legacy 字段不默认填充。
- 同一 PostgreSQL 环境执行 `cargo test --offline --locked -p ficant-storage --test migration_acceptance`：4/4；精确覆盖 forward migrations `0001–0019`、0019 单次登记、重复执行不变、legacy / FK 判据与失败原子回滚。
- `cargo test --offline --locked -p ficant-api --test portfolio_risk_service`：2/2；覆盖 generated service 与 curve-point unknown-field / 非 canonical bytes 拒绝。
- 注入固定 Buf 1.56.0 后，`cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory`：17/17。
- `cargo test --offline --locked -p ficant-server --test portfolio_risk_sit`：1/1；production composition 暴露独立路由，畸形请求在 I/O 前返回 typed error。
- `pwsh -NoProfile -NonInteractive -File scripts/check-layering.ps1`：exit 0，`AC03=0`、`AC01=0`、production C++/FFI=0、Funding=0、Tax=0、allowlist=0。
- `pwsh -NoProfile -NonInteractive -File scripts/test-layering-check.ps1`：exit 0，51 assertions。

**完整本地检查（全部 exit 0）：**

- `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`：exit 0。
- 使用仓库既有受信 Node 22.17.0 与固定 Buf 1.56.0 执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1`：exit 0；包括 strict Clippy、workspace build / non-environment tests、descriptor 17/17、C++ 8/8、Phase 2C / 2D Oracle 各 3/3、Python generated 1 pass / 1 skip、Phase 2E live 1/1、Phase 3A 5/5、Phase 3B codec 3/3，以及 Web 5 files / 35 tests。专用 worktree 首次到达 Web 时发现 ignored `node_modules` 不存在，使用 `corepack pnpm@10.12.4 install --offline --frozen-lockfile` 恢复本地依赖（178 reused、0 downloaded、无 tracked diff）后重新从入口完整运行；此前失败不作为通过证据。
- 导入六个 Windows User 级 `FICANT_TEST_*` 变量（未输出密钥）执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration`：exit 0。除上述完整检查外，真实 PostgreSQL / Ceph 范围包括 migration 4/4、Phase 4C lease 1/1、execution closure 3/3、production Worker 1/1、Phase 1 business loop 1/1、negative invariants 13/13、Phase 2B / 2C / 2D 各 1/1、Phase 3A registry / parity 各 1/1、Phase 3B publication 1/1。
- 当前进程初始 Node 24.18.0 已由 Human 纳入可信范围，但现有 `check.ps1` 仍精确锁定 22.17.0；因此完整检查使用本机预装的精确 22.17.0，仅调整当前进程 PATH，未修改门禁、expected 或仓库文件。首次被 Node preflight 拒绝的运行不作为证据。

**终态审计：** `git diff --check` exit 0。`git status --porcelain=v1 -uall` 共 51 个变更路径，逐项属于冻结的 63 项允许清单，范围外写入为 0。受保护的 allowlist、canonical schema、Golden / Oracle、Phase 2C / 2D matrix、C++ / C ABI、既有 rates / position / factor proto 与 `rates.rs` 对 base 的 diff 均为空；allowlist 内容仍为 `[]`。变更的 proto 只有获准的 `market/v1/definition.proto`、`market/v1/fact.proto` 与新建 `research/v1/exposure.proto`，点名禁止的 position / factor / health / constraint / policy proto 为 0。`HEAD == origin/main == 4e472a8993b5d2a5c4a5c69bf078c9659d19e2de`，本地未 commit。

**验收结论：** R4d-a acceptance sentence 已在本地候选成立：服务端只从 verified PositionSnapshot、CurveSnapshot / canonical points、完整 registered Bond / Calendar / Unit 与 R4c Factor topology 生成稳定的逐债券三因子 KRD 和 bond-only totals；缺失、漂移、不支持 convention / Bond 或任何纳入敞口的非 Bond 均失败关闭且不返回 partial exposure。由于尚无期货重定价与全组合聚合，本轮诚实保持 AC16 未点亮，不能以本候选替代 R4d-b。

## 7. 残余风险

- R4d-a 返回的是债券子组合结果；即使所有本轮测试通过，也不能点亮 AC16 或声称“全组合”。R4d-b 仍须在新的公共 / authority 双 base 上独立冻结。
- 首个算法是 curve-implied maturity YTM 重定价，不是逐现金流 zero-curve discounting。未来引入 zero curve、浮息、不规则、含权或税后现金流时必须发新算法版本，不能保持相同 metadata 偷换方法。
- legacy Bond / CurveSnapshot 可继续被仓储读取，但缺新增字段时永远不能进入 R4d-a。这是 forward-only 兼容边界，不是用默认值修补历史。
- R4c 可以注册 `HOLD` / `INCLUDE` definition，但本轮计算明确拒绝；definition 可注册不等于本算法可执行，MANUAL 必须如实区分。
- complete axis 中 exact zero 会进入响应和 hash；非零 Instrument binding 要求可能随着债券剩余期限迁移而需要追加静态关系。R4d-a 不自动写 topology，缺 binding 时失败关闭。
- same-day business-date settlement 是一期算法口径，不代表市场标准结算；若未来引入 settlement lag，必须进入新算法 / RulePack 与 fingerprint，不能无痕改变。
- ADR-0015 的推荐补充已获 Human 语义批准但不在本轮写路径。公共实现候选审阅时仍须核对其 metadata 与该建议完全一致；R4d-b / AC16 结束前 ADR-0015 的数值敞口要求仍未全部落实。
