# R4a 迭代 brief — CTD 双时间与具体合约输入

**迭代：** R4a · **本轮目标条目：** AC27、AC28 · **execution base：** `074ae852a4c8ede90155500a8bad2fb3edeb12f1` · **authority commit：** `d0915ef7eaffd0f07020c58c52eebb9d83543e8c`

本 brief 是 R4a 面向 Human 的唯一已批准设计。它只收紧 `AnalyzeFuturesDelivery` 的历史 CTD 输入边界：交割价格与可交割券清单必须由服务端从与估值点精确匹配的、已验证的双时间 `DataSnapshot` 内容派生，不能只相信调用方提供的 snapshot binding；所选不可变 snapshot 的 `visible_at` 是这一次请求固定的知识时点。基差输入必须解析为已注册的具体 `FuturesContract`。R4b 的 PositionSnapshot / 会计三态和 R4c 的 Factor / Exposure 仍是独立迭代，不得借本轮提前定义其契约。Human 于 2026-07-30 批准本设计，并于 2026-07-31 明确授权开始实施。

## 1. 目标

让 `AnalyzeFuturesDelivery` 不再把任意调用方提供的 `ObjectBinding` 当作历史事实。服务在进入 RulePack 解析和 native engine 前，必须解析两个现有的持久化对象：

- `context.data_snapshot` 必须是精确的 `research.v1.DataSnapshot`，其内容 hash、授权 owner、`as_of` 与 `visible_at` 均可核验，且服务端必须验证并解码其 Parquet / Manifest；
- `futures_contract` 必须是精确的、已注册的 `market.v1.FuturesContract`，而不是连续或拼接序列、普通 Instrument、Artifact 或其他可绑定对象。

**Acceptance sentence：**

> 给定一个 `as_of` 与 `valuation_at` 完全相同、且 `visible_at` 不早于 `as_of` 的已持久化 DataSnapshot（该 `visible_at` 即本次请求的知识时点），以及一个绑定同一 cgb-futures RulePack 的具体注册 FuturesContract，服务端验证并解码该快照，以其中当日、满足交割规则的债券报价导出完整可交割集合，并只接受与这些报价相符的请求价格；`AnalyzeFuturesDelivery` 随后返回既有的可交割券、基差、IRR 与 CTD。把同一历史估值请求换成 `as_of` 更晚的当前 DataSnapshot、把历史快照配上当前价格、或把 futures 输入换成连续 / 拼接序列，服务均在 RulePack parser 与 native engine 调用前以不可重试错误失败关闭；缺少由精确 RulePack 导出的任一可交割券时，在一次规则解析后、native engine 调用前失败关闭。具有相同历史 `as_of` 但更晚 `visible_at` 的新 snapshot 是一个不同的、较晚知识时点视图，不得被误称为“当前清单回算”。现有 C++ / C ABI、数值公式、Golden、Oracle、canonical schema/hash、Phase 2C/2D matrix 与已点亮条目均保持不变。

## 2. 验收

| 条目 | R4a 可执行判据 |
|---|---|
| AC27 | `AnalyzeFuturesDelivery` 以实际 `SnapshotVerifiedReadMetadataRepository` 与 `VerifiedBlobReader` 的精确 `DataSnapshot` 为事实源：binding id / content hash / owner 必须匹配，且 snapshot 必须是 Data 而非 Universe；`snapshot.as_of == valuation_at` 按完整 `MarketTime`（UTC instant、IANA 时区、交易日）相等，`visible_at.instant() >= as_of.instant()`，该不可变 `snapshot.visible_at` 是这次请求唯一的知识时点。服务必须先验证并解码 Parquet / Manifest；只使用 snapshot 中 `local_trading_date == valuation_at.local_trading_date`、`observed_at <= as_of`、`quote.visible_at <= snapshot.visible_at`，且 unit 精确等于 `context.units.price_per_100` 的报价。每个请求 bond 必须解析为同 owner 的精确注册 Bond version；服务以该 Bond 的首发日 / 到期日和已解析 RulePack 导出可交割集合，请求 candidates 必须是该集合的精确无重复排列。每个 `spot_clean_price` 必须等于该 Bond exact version 的同日 quote bid 或 ask，`futures_clean_price` 必须等于具体 FuturesContract exact version 的同日 quote bid 或 ask；请求中可由已注册 Bond 表达的首发日、本期发行日、到期日与面值也必须相等。以 `as_of` 更晚的当前快照回算历史估值、把当前价格配上历史 snapshot 时，RulePack parser 与 engine call count 均为零；缺少预期集合中的任一券时，必须先解析一次精确 RulePack 才能导出预期集合，因此 parser call count 为一、engine call count 为零。相同 `as_of` 而较晚 `visible_at` 的 snapshot 只能作为较晚知识时点的独立输入。 |
| AC28 | `AnalyzeFuturesDelivery.futures_contract` 以实际 `DefinitionRepository` 的精确 `DefinitionValue::Instrument(InstrumentKind::Futures, FuturesContract)` 为事实源：Instrument 版本与请求 binding 相同、owner 可访问、其 `rule_pack` version ref 与 `context.rule_pack` 相同。绑定普通 Instrument、无 subtype、连续 / 拼接序列或任何非 FuturesContract 定义时，在 RulePack parser 与 native engine 前失败，二者 call count 均为零；具体合约才到达 engine。 |

R4a 闸门：

1. 先只加入 AC27 / AC28 的直接 application 与 gRPC service 判据：`cargo test --offline --locked -p ficant-application --test futures_delivery_input_bindings ac27_verified_snapshot_owns_delivery_candidates_and_prices -- --exact` 与 `cargo test --offline --locked -p ficant-api --test rates_service ac28_only_concrete_futures_contract_reaches_delivery_engine -- --exact` 都必须亲眼取得非零 exit code；两个判据的失败分支必须同时断言 RulePack parser 与 native engine call count 均为零。这个 RED 不构成 checkpoint。snapshot / contract resolver、生产组合和 Phase 2E live fixture 不得早于该 RED。
2. `DataSnapshot.as_of` 与 `visible_at` 都是完整 `MarketTime`，不得降级为 ISO 日期字符串或只比较本地日期。`as_of > visible_at` 仍由领域构造拒绝；R4a 在消费边界再次核验，历史计算不得接受 `as_of != valuation_at` 的 snapshot。每条真正进入计算的 canonical quote 还必须由 verified decoder 证明 `observed_at <= as_of` 且 `visible_at <= snapshot.visible_at`；只比较 snapshot metadata 不构成完成。
3. `futures_contract` 的具体性来自已注册 `FuturesContract` subtype 和其不可变 instrument version，不新增由调用方自行声明的 `continuous = false`、产品月份猜测或字符串正则旁路。连续 / 拼接数据若没有具体 FuturesContract 定义，必须失败关闭。
4. `context.data_snapshot` 与 `futures_contract` 的新解析只在 `AnalyzeFuturesDelivery` 消费；不得把它们扩散到其他四个 Rates RPC，亦不得让未消费 binding 只进 metadata 伪装为血缘。DataSnapshot 的 metadata lookup、verified blob read、canonical decode、集合导出与价格核验是一个不可拆开的消费链；任何只保留 metadata lookup 的实现均为失败。
5. 不得修改 `.proto`、生成输出、C++、C ABI、RulePack 内容、Golden、Oracle、容差、canonical schema/hash、Phase 2C/2D matrix、matrix guarded hash、selector、command 或路径风格。`layering-allowlist.json` 必须继续为 `[]`；脚本不在写路径内。
6. 最终候选必须单独呈现 `Cargo.lock`、`rates.rs`、application resolver、server composition、两组针对性测试、Phase 2E live fixture 与 `docs/iterations/README.md` 的 base-to-candidate diff；任何未列路径在首次写入前停止并返回 Human，不得修改本 brief 的 §6 清单追认。

## 3. 非目标

- R4b 的 `Position` / `PositionSnapshot`、会计三态、额度或资本占用，以及 AC14、AC17–AC19。
- R4c 的 `Factor` / `Exposure` / KRD、全局 FactorId 或 AC05、AC16。
- 新增或修改任何 `.proto`、Python / TypeScript generated contract、Python SDK 形状、C++ / FFI、Arrow schema、Artifact codec、PostgreSQL migration、RulePack 内容、RulePack parser 或交割数值公式。
- 真实行情接入、实时交割篮子、价格可信度等级、CoverageDeclaration、连续合约的构建或研究方法；本轮只拒绝其作为基差输入。
- 修改 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、ADR、`.gitignore`、`README.md`、`.github/**`、`cicd.yml`、`deploy/**`、Golden、Oracle、matrix 或 R3a/R3b brief。

## 4. 公共契约变化

- Protobuf wire shape 保持不变；R4a 不新增字段或 message。`AnalyzeFuturesDelivery` 的既有 `context.data_snapshot` 从“只携带的 ObjectBinding”收紧为必须解析、verified-read 并 canonical-decode 的精确 DataSnapshot binding：id / content hash / owner 匹配，且该快照以完整双时间事实满足 `as_of == valuation_at` 与 `visible_at >= as_of`。该不可变 snapshot 的 `visible_at` 明确是本次请求的知识时点；R4a 不虚构独立的 `knowledge_at` 入参，也不把它默认为 `valuation_at`。不再允许 metadata binding 与内联数值彼此独立。
- `AnalyzeFuturesDelivery.futures_contract` 从“任意 ObjectBinding”收紧为必须解析的具体 FuturesContract definition。解析器只接受 exact Instrument version 的 `FuturesContract` subtype，并要求其 RulePack version ref 与 `context.rule_pack` 一致；没有该类型事实的连续或拼接序列不能进入基差 / CTD 数值路径。
- 应用层定义窄的 canonical-quote decoder port，并以既有的 `SnapshotVerifiedReadMetadataRepository`、`VerifiedBlobReader`、`IntegrityEventSink` 和 `DefinitionRepository` materialize 交割输入；API adapter 使用 `ficant-data` 的 verified codec 实现该 port。`ficant-data` 只从已验证 batch 暴露 schema 不变的 quote projection（exact instrument version、双时间、交易日、bid / ask 与 unit），不改变 Canonical schema/hash。materializer 从已验证的 canonical snapshot 取出同日、同 unit quote，以已注册 Bond 的首发日 / 到期日调用从既有规则提取的 dates-only domain eligibility helper；API 只把 materializer 已核验的集合和价格交给 native input，不在 adapter 重写 RulePack 条件。
- 这不向 `ficant-domain` 注入存储依赖、不改变 native input、C ABI、Arrow Artifact 或既有结果字段。成功结果继续由既有 input 的 data snapshot、futures contract、rule pack、Subject 与估值时点组成血缘；失败结果不产生部分计算。

## 5. 需 Human 决策

- **已裁决——严格时间语义：** `snapshot.as_of` 必须与 `valuation_at` 的完整 `MarketTime` 精确相等；所选不可变 `snapshot.visible_at` 是本次请求唯一的知识时点。同一历史 `as_of`、更晚 `visible_at` 的 snapshot 合法表示“后来获知的修订视图”，本轮不新增独立 `knowledge_at`。只比较日期、把 `valuation_at` 偷换为知识时点，或引入未批准的时间窗均不属于本轮。
- **已裁决——具体合约：** `futures_contract` 只接受已注册的精确 `DefinitionValue::Instrument(Futures, FuturesContract)` subtype，并校验 owner、version 与 RulePack ref；连续合约、拼接序列及无 subtype 的 Instrument 在 RulePack parser 和 engine 前失败关闭。调用方自报 `continuous = false`、根据代码猜测产品月份或用字符串正则推断合约均不是替代证据。
- **已裁决——AC27 窄静态边界：** 候选集合和价格由服务端从 verified snapshot 派生；首发日、本期发行日、到期日和面值与注册 Bond 核验。coupon、frequency、day-count、business-day 暂保留为既有请求输入并进入 fingerprint；R4a 不宣称全部 CTD 静态条款已注册。若以后要求完整持久化，另开 Bond 契约扩展迭代；不得在 R4a 暗中扩展市场 Definition proto / generated contract、Domain Bond 或 PostgreSQL definition codec。
- **已裁决并据此拆分：** R4a 只点亮 AC27 / AC28；R4b 承接 AC14、AC17–AC19，R4c 承接 AC05、AC16。R4a 不提前定义 Position / Factor 契约。
- **实施授权已给出：** Human 于 2026-07-31 明确授权开始；R4a 据此执行 RED-first、建立生产组合并形成当前本地候选。该授权不包含 push、Pull Request、merge、authority 批准或发布。
- **实施证据更正——缺券集合的 parser 计数：** 冻结判据曾要求“缺少预期集合中的任一券”在 RulePack parser 前失败。实现中确认预期集合本身依赖精确 RulePack 的交割期限数值，未解析规则无法诚实判断哪只注册 Bond 应属于集合；因此该分支的真实边界是 parser 一次、native engine 零次。snapshot 时间 / hash / owner、quote 时间 / unit / price、FuturesContract subtype / owner / version / RulePack ref 等无需导出 eligibility 即可判断的错误仍保持 parser 与 engine 均为零。此更正不删除负向判据、不改 expected 制造通过，也不削弱 AC27“禁止用当前清单回算历史”的语义。
- **authority 后续动作：** 公开候选经 Human rebase merge 后，私有 authority 必须在新的独立提交中更新 MANUAL 的“现在可调用能力”与 ACCEPTANCE 的 AC27 / AC28 批准状态，并由 `verify-authority.ps1` 在干净 authority worktree 复核。该后续 binding 不得倒写为本轮 execution freeze。

## 6. 最终真实测试证据

**候选状态：** Human 已批准 R4a §5 并授权实施。实现由冻结的 execution base 前向形成；没有 push、Pull Request、merge、authority 批准或发布动作。

**冻结允许写路径（随 execution base 与 2026-07-30 Human 裁决一并冻结；本清单不是当前实施授权）：**

- `Cargo.lock`
- `binaries/ficant-server/src/lib.rs`
- `crates/ficant-api/Cargo.toml`
- `crates/ficant-api/src/canonical_snapshot.rs`（新建）
- `crates/ficant-api/src/lib.rs`
- `crates/ficant-api/src/rates.rs`
- `crates/ficant-api/tests/phase2e_sdk_live.rs`
- `crates/ficant-api/tests/rates_service.rs`
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/ports/canonical_snapshot.rs`（新建）
- `crates/ficant-application/src/ports/mod.rs`
- `crates/ficant-application/src/use_cases/futures_delivery.rs`
- `crates/ficant-application/src/use_cases/verified_reads.rs`
- `crates/ficant-application/tests/futures_delivery_input_bindings.rs`（新建）
- `crates/ficant-data/src/lib.rs`
- `crates/ficant-data/src/snapshot.rs`
- `crates/ficant-data/tests/snapshot_codec.rs`
- `crates/ficant-domain/src/futures_delivery.rs`
- `crates/ficant-domain/tests/futures_delivery_contracts.rs`
- `docs/iterations/2026-08-r4a-ctd-time-contract.md`
- `docs/iterations/README.md`

**禁止写路径：** 一切未逐项列出的路径，特别是 `SPEC.md`、`ACCEPTANCE.md`、`MANUAL.md`、`README.md`、`docs/architecture/adr/**`、`interface/**`、所有 `.proto` 与 generated contract、`cpp/**`、`crates/ficant-storage/**`、`scripts/**`、`domain-packs/**`、`tests/golden-cases/**`、`tests/oracle/**`、`tests/phase2c/**`、`tests/phase2d/**`、`crates/ficant-data/src/canonical.rs`、`.gitignore`、`.github/**`、`cicd.yml`、`deploy/**`、R3a/R3b brief。

**规定的针对性命令（须在最终候选实际取得结果）：**

- `cargo test --offline --locked -p ficant-application --test futures_delivery_input_bindings`
- `cargo test --offline --locked -p ficant-domain --test futures_delivery_contracts`
- `cargo test --offline --locked -p ficant-data --test snapshot_codec`
- `cargo test --offline --locked -p ficant-api --test rates_service`
- `pwsh -NoProfile -File scripts/check-phase2e-sdk.ps1`
- `pwsh -NoProfile -File scripts/check-layering.ps1`
- `pwsh -NoProfile -File scripts/test-layering-check.ps1`
- `pwsh -NoProfile -File scripts/check-fast.ps1`
- `pwsh -NoProfile -File scripts/check.ps1`
- `pwsh -NoProfile -File scripts/check.ps1 -IncludeIntegration`
- `git diff --check`

**RED-first 实证：**

- AC27 application 判据在 materializer 存在前以 exit `101` 编译失败。
- AC28 gRPC 判据在生产解析器接入前以 exit `1` 失败；失败分支实测 RulePack parser / native engine call count 为 `1 / 1`，不是 checkpoint。

**forward-only checkpoint：**

1. `89b2d95` 冻结 R4a 设计、execution base 与逐文件写路径。
2. `4846369` 只提取 provider-neutral 的 dates-only 交割资格 helper。
3. `70c713f` 从 verified canonical batch 暴露 schema 不变的窄 quote projection。
4. `70cefc5` 建立 application materializer、精确 FuturesContract / Bond 解析与 AC27 失败关闭边界。
5. `ee58aa1` 完成 API adapter、server 生产组合、gRPC 判据与 Phase 2E live fixture。
6. `36df64f` 前向收紧 snapshot projection 测试以满足严格 Clippy。
7. `b3c1e3d` 恢复被 Phase 2C matrix 守卫的领域测试原字节，不更新 matrix 或 guarded hash。
8. `8a6109c` 增补后来修订 snapshot 与缺券集合负向判据。

**最终候选真实证据：**

| 检查 | 结果 |
|---|---|
| `cargo test --offline --locked -p ficant-application --test futures_delivery_input_bindings` | exit `0`，3 passed |
| `cargo test --offline --locked -p ficant-domain --test futures_delivery_contracts` | exit `0`，2 passed |
| `cargo test --offline --locked -p ficant-data --test snapshot_codec` | exit `0`，3 passed |
| `cargo test --offline --locked -p ficant-api --test rates_service` | exit `0`，15 passed |
| `scripts/check-phase2e-sdk.ps1` | exit `0`，1 passed |
| `scripts/check-layering.ps1` | exit `0`；AC03、AC01、C++ / FFI、R3a funding、R3b tax 与 allowlist 均为 `0` |
| `scripts/test-layering-check.ps1` | exit `0`，51 assertions |
| `scripts/check-fast.ps1` | exit `0` |
| `scripts/check.ps1` | exit `0` |
| `scripts/check.ps1 -IncludeIntegration` | exit `0`；含 migration 4、Phase 3B codec 3 及其余既有集成切片 |
| `git diff --check` | exit `0` |

本地锁定工具链为 Rust / Cargo `1.96.1`、Node `22.17.0`、pnpm `10.12.4`、uv `0.7.13` 与 Python `3.12.11`。第一次完整门禁预检发现当前进程的 Node 24 不符合脚本冻结的 Node 22；最终检查仅在进程内前置可信 Node `22.17.0`，未改脚本或仓库。完整集成首轮还发现 Docker Desktop 未运行；启动既有本地 Linux engine、导入 Windows User scope 的六个 `FICANT_TEST_*` 变量后，最终 `-IncludeIntegration` 取得上述 exit `0`，密钥未写入 brief 或命令输出。

base-to-candidate 审计只包含 §6 逐项允许的路径。`crates/ficant-data/src/canonical.rs`、`scripts/layering-allowlist.json`、Phase 2C / 2D matrix 的 Git blob 与 execution base 相同；Golden / Oracle 无差异；没有 `.proto`、generated contract、C++ / C ABI、RulePack、schema 或 matrix rebaseline。曾改动的 `crates/ficant-domain/tests/futures_delivery_contracts.rs` 已恢复为 base 原字节，Phase 2C verifier 仍为 18 / 18。

## 7. 残余风险

- R4a 建立的是“请求必须绑定、验证并消费哪一份已持久化事实”的失败关闭边界，不是新的外部行情接入、交割篮子抓取或价格来源可信度体系；R5 仍负责 I9 / coverage 相关能力。snapshot 内的报价集合是本次计算的显式范围，不是对未导入市场部分的完整性主张。
- 单纯以不可变 snapshot id / hash 和 metadata 绑定调用方提供的数值是不足的，且不允许作为 R4a 实现；服务端必须 verified-read 并 decode 内容后导出集合、核验价格。管理员错误发布的 snapshot 仍是上游数据治理问题，不能由 CTD adapter 猜测或改写。
- R4a 不新增可独立选择的 `knowledge_at`；一次请求的知识边界就是其精确 immutable `DataSnapshot.visible_at`。因此相同经济 `as_of` 在不同可见时点的修订会形成不同 snapshot、不同 input fingerprint 与不同可追溯结果。若产品需要在一个 snapshot 之外按任意知识时点查询，必须由后续契约显式引入该选择，不能把 `valuation_at` 偷换为知识时点。
- Human 已批准 §5 的窄静态边界：coupon / frequency / day-count / business-day 等尚未注册为 Bond Definition 的静态 CTD 条款仍由既有请求提供；它们会进入 input fingerprint，但不是新的持久化事实源。这个边界不能被描述为“全部 CTD 输入均由 snapshot / Definition 派生”；若未来要求该更强主张，须单独冻结 Bond 契约扩展。
- `FuturesContract` subtype 是具体合约的类型证据；它不构建、校正或解释任何连续合约序列。连续序列未来若成为研究对象，必须在独立迭代定义其对象与用途，不能绕过本轮的基差入口。
- 现有 Instrument definition 没有与 `ObjectBinding.content_hash` 对等的公开内容哈希合同。R4a 因而核验 FuturesContract 的 identity / version / owner / subtype / RulePack ref，而不虚构一个无法由现有 SDK 重算的定义哈希；若将来需要定义内容级别的 cross-boundary binding，必须独立冻结其哈希契约。
- 冻结判据里“缺少预期集合中的任一券时 parser 为零”在实现上不可满足：交割期限是 RulePack 内容，必须先解析才能导出 expected eligible set。最终负向判据保留并证明 parser 为一、engine 为零；其余能在规则解析前判定的 snapshot、quote 和 contract 错误仍是零 / 零。Human 审阅候选时应按 §5 的证据更正确认该观测边界，而不能把 parser 零伪装成通过。
- 本轮已获实施授权并形成自测候选，但 AC27 / AC28 的 authority 点亮仍依赖公开代码候选经 Human rebase merge，以及随后在私有 authority 仓库进行独立 ACCEPTANCE / MANUAL binding 与 `verify-authority.ps1` 复核。
