# R7A 迭代 brief — 分层与双时间重取证

**迭代：** R7A · **点亮目标：** AC04、AC11–AC13 · **execution base：** `d836f0c384a82d8f392dd4c2f9241e06a1c3a3c6`

本 brief 是 R7A 面向 Human 的唯一设计与最终证据载体。R6B 已由公共 PR #61 以 rebase merge 进入 `main`，合并后的公共提交已经在同一 HEAD 通过 `scripts/check-fast.ps1`。R7A 只裁决核心分层能否由新市场包扩展、所有显式知识时点查询是否遵守可见时间、非法双时间行能否带安全行证据失败关闭、不可变快照能否在外部源消失后精确重读，以及两个冻结 Clang 工具链是否产生逐位相同的原始数值。它不承担完整血缘、恢复、MANUAL 或版本发布；这些属于 R7B 或 CICD。

## 1. 目标

R7A 交付一个可机械复验的架构结论：加入一个仓库外形的虚构市场 RulePack 与 Subject 后，既有通用定价核心无需任何 L0/L1/L2 生产源改动即可完成现金流与债券估值；若这一硬条件成立，本轮不拆分 domain crates。所有带显式知识时点的生产查询均同时在存储过滤和解码后验证两道边界上保证 `visible_at <= knowledge_at`。Canonical ingest 对 `observed_at > visible_at` 返回规范化、客户端安全的原因与确切 `source_record_id`，并在任何快照 stage、blob 或数据库写入前失败。发布后的 Canonical Snapshot 在外部源被明确销毁后，仍只依赖 Snapshot ID、正式 metadata 与 immutable blobs 重建完全相同的 Arrow RecordBatch。Windows Clang 19.1.5 与 Ubuntu Clang 18.1.8 对同一 C ABI fixture 输出的每个正式 `double` 以 IEEE-754 bit pattern 精确相等。

**Acceptance sentence：**

> 给定一个新增的、与中国市场无关的虚构市场 RulePack、Subject 和债券输入，外部扩展夹具必须通过既有 Application/Native 边界解析规则与主体并完成确定现金流和估值，同时冻结清单中的 L0 primitives、L1 research/Subject、L2 analytics/curve/futures/native/kernel 生产源逐文件 SHA-256 与 R7A execution base 完全相同；给定任一显式知识时点 `T`，Fact window、Curve get、Position、DataHealth profile 与 SubjectState 查询不得返回 `visible_at > T` 的值，SQL 过滤被绕过时解码后校验仍须失败关闭；给定一条 `observed_at > visible_at` 的源记录，import 必须在所有写入前返回包含规范化原因与原始安全行标识的 typed field violation；给定一个已发布 Snapshot，在外部 DataSource 内容被销毁且服务重启后，只凭 Snapshot ID 必须从正式 immutable blobs 重建 schema、列、顺序、值和原始 RecordBatch 完全相同的结果；Windows Clang 19.1.5 与 Ubuntu Clang 18.1.8 的冻结 runner 对全部正式浮点输出必须逐位相等，任何缺项、非有限值、状态漂移或 bit drift 均使门禁失败。

## 2. 验收

| 条目 | R7A 可执行判据 |
|---|---|
| AC04 虚构市场 | 新 fixture 使用独立 market code、RulePack payload、Subject profile、Bond/Calendar/Unit/Snapshot identity；RulePack 与 Subject 共同决定进入计算的 coupon treatment，计算返回完整 cashflows、price、YTM、duration、convexity 与 DV01。RulePack/Subject 任何一方漂移须在 engine 前失败或改变已验证的计算结果/身份，不允许只进血缘。 |
| AC04 零核心改动 | 独立 manifest 枚举并 SHA-256 绑定 L0/L1/L2 生产源；门禁同时验证文件集合和内容，带一个被篡改的真实反例。R7A 只能新增外部扩展 fixture、测试和门禁，不得改受保护生产源。门禁通过即裁决“不拆 crate”；只有门禁或真实扩展失败才允许另开 Human 决策，不得在本轮预防性大拆分。 |
| AC11 Fact 查询 | `QueryInstrumentFactsRequest` 增加必填 `knowledge_at`。Application query fingerprint、AEAD cursor、SQL keyset 与 payload decode 都绑定该时点；同一 instrument/observed window 内只返回 `visible_at <= knowledge_at` 的事实。缺失、timezone 漂移、cursor 跨知识时点均失败关闭。 |
| AC11 Curve 查询 | `GetCurveSnapshotRequest` 增加必填 `knowledge_at`。只有带 exact `visible_at` 且不晚于该时点的 CurveSnapshot 可返回；未知可见时点或未来可见内容不得作为历史知识返回。内部 required-read 的 exact replay 能力不被公共历史查询替代。 |
| AC11 既有矩阵 | Position exact/resolve、DataHealth profile exact/active、SubjectState scoped/global 的现有知识时点边界以真实 PostgreSQL 正负矩阵重取证；每一路都同时验证早一纳秒不可见、边界时刻可见、晚一纳秒可见，以及 payload/SQL visible drift 的 fail-close。 |
| AC12 行级拒绝 | `DataError` 保留分类同时携带受限长度、无凭据/路径的 `source_record_id` 与封闭原因枚举。`observed_at_after_visible_at` 映射到 `ApplicationErrorDetail`，再映射到现有 `ErrorDetail.field_violations`；字段路径和描述稳定。含该行的整批失败，adapter read 可发生，但 snapshot/blob/stage/repository counters 全为零。 |
| AC13 源消失重读 | production-style PostgreSQL + Ceph-compatible SIT 完成授权导入、发布、重启；随后显式销毁/禁用源并证明再次访问会失败。`VerifiedSnapshotReader` 只从正式 metadata/blob reference 读取 Parquet+manifest，`CanonicalSnapshotCodec::decode_verified` 产出的 Arrow schema、row/column count、array values、row order 与发布前 RecordBatch 精确相等，source call count 不增加。 |
| Clang 原始位裁决 | 一个无容差 runner 覆盖 bond result + cashflows、curve interpolation、carry/roll、futures delivery、futures hedge 的全部正式 `double` 字段及 status/count/integer identity；输出 canonical text 中每个 double 只以 16 位十六进制 bit pattern 表达。固定 Windows Clang 19.1.5 和 Ubuntu Clang 18.1.8 分别从同一源构建运行，比较器要求 key 集合、顺序和值完全一致，并带单 bit 漂移反例。 |
| 回归与门禁 | Buf format/lint、双临时生成树、descriptor、Rust/Python/TypeScript consumer、R5D layer gate、R7A core manifest、R7A bitemporal/row/source-destruction、现有 R4/R5/R6 回归及三个统一入口全部转绿；不得调整既有 expected、Oracle、数值容差或生产算法来迎合门禁。 |

RED-first 子循环按以下顺序执行；首次真实非零命令、exit code、首个失败测试和首错在发生时保留，最终 §6 只记录最终候选上的真实命令：

1. **AC04 RED：** 先让虚构 market/RulePack/Subject fixture 在既有边界上编译或语义失败，并让 core-manifest 反例确实拒绝一个 production source drift；随后只补外部扩展与门禁。
2. **AC11 contract/storage RED：** 先增加 `knowledge_at` descriptor 断言与未来可见 Fact/Curve fixtures，使旧 API/port/SQL 失败；再贯穿指纹、cursor、SQL 和 decode，并重取证既有三类仓储。
3. **AC12 RED：** 先证明旧 `DataError` 与 API response 丢失 `source_record_id`/原因且 import 写计数无法表达；再增加封闭 typed detail 与零写入矩阵。
4. **AC13 RED：** 先把外部源改成可销毁且销毁后访问必失败，随后证明 existing verified reader 不访问源并精确重建 RecordBatch；不新增公共下载 API。
5. **Clang RED：** 先用冻结 runner 的单 bit 反例验证比较器，再真实运行 19.1.5/18.1.8。任何真实差异先定位并作为 blocker，不以 tolerance、rounding 或删除字段消除。
6. **回归：** 恢复 generated consumers、fast/full/integration 入口并核对受保护源 manifest；禁止通过跳过 WSL、PostgreSQL/Ceph 或放宽断言转绿。

## 3. 非目标

- 不实施 R7B 的 AC30–AC33、完整递归血缘、部署镜像身份、outbox/crash recovery、灾备、MANUAL 或全量运行手册重写。
- 不创建 Portfolio360/COGA WebApp，不接入 `cogawork`，不增加 Portfolio/Book/NAV/P&L/归因/VaR/优化等产品域。
- 不新增 generic Artifact payload upload/download、presigned URL、client streaming 或第二 Snapshot authority；AC13 只走内部 required verified-reader。
- 不补 FundingRulePack Decimal Oracle、DataHealth property suite、平行 convention enum、`rates.rs` 拆分、DMQuant、AI/GeneratedNode、Python node runtime、Policy/Constraint 或 v0.2 范围。
- 不预防性拆分 `ficant-domain`、`ficant-application` 或 native crates；AC04 硬门禁通过时本轮结论就是保持现有 crate 边界。
- 不修改 private authority、公共根目录 ignored authority、原工作树的未跟踪审计报告、既有 Golden/Oracle/expected/容差或 L0/L1/L2 受保护生产源。
- 不修改 GitHub 远端权限/安全/branch protection，不创建 version/tag，不发布镜像或部署；无 Human 版本号，因此不进入 CICD。

## 4. 公共契约变化

R7A 只对现有 MarketFact 查询补齐缺失的第二时间维度，不建立新 service：

- `QueryInstrumentFactsRequest` 新增必填 `ficant.core.v1.MarketTime knowledge_at = 5`。`from/to` 继续表示 observed/effective 查询窗口，不得兼作知识时点；旧请求缺少该字段失败关闭，不提供默认“当前时间” shim。
- `GetCurveSnapshotRequest` 新增必填 `ficant.core.v1.MarketTime knowledge_at = 2`。该 RPC 是历史知识查询；server-internal exact required-read 仍可为重放读取已知 ID，两者不可混用。
- `ErrorDetail` 不改 schema。AC12 使用既有 `field_violations`：字段采用 `source_rows[id=<canonical-id>].observed_at`，description 采用固定客户端安全文案；不回传源路径、连接串、原始行内容或 adapter 错误。
- 生成物只来自 fixed Buf；Rust/Python/TypeScript consumer 同步证明两个字段存在且必填语义由 service 负向测试见证。

## 5. 需 Human 决策

Human 已在确认 R7 工作内容后指令“完成此任务”，下列 R7A 选择据此冻结。语义、公共契约、路径或受保护事实若需变化，必须在首次相关写入前重新取得 Human 明确授权。

| 决策 | 冻结选择 | 排除边界 |
|---|---|---|
| D1 · AC04 判据 | 硬零改动：外部 fixture + 完整生产源 SHA manifest；通过则不拆 crate。 | 不接受“改动少于 N 行”、allowlist 或事后排除已改文件。 |
| D2 · AC11 契约 | Fact window 与 public Curve get 各携独立必填 `knowledge_at`，贯穿 fingerprint/cursor/SQL/decode。 | 不把 `to`、`as_of`、server clock 或 request arrival time冒充知识时点。 |
| D3 · AC12 证据 | 使用封闭内部 reason + 安全行 id，并映射到既有 `FieldViolation`。 | 不新增泄漏原始行的 debug string，也不只在日志中记录。 |
| D4 · AC13 读取边界 | 只用内部 `VerifiedSnapshotReader` + codec 证明；外部源显式销毁。 | 不新增通用 blob 下载接口或测试 fake 绕过正式 metadata/blob reference。 |
| D5 · 跨编译器 | Windows VS LLVM Clang 19.1.5 对 Ubuntu 24.04 locked Clang 18.1.8；逐字段原始 bit exact。 | 不以数值 tolerance、格式化十进制或“同 major”替代，也不比较不同 fixture。 |
| D6 · 交付边界 | R7A 完成并由 authority 独立绑定 AC04/11–13 后才开始 R7B；本轮无版本发布。 | 不把规划命令写成通过，不在 R7A 提前点亮 AC30–33。 |

## 6. 最终真实测试证据

**R7A execution base：** public `d836f0c384a82d8f392dd4c2f9241e06a1c3a3c6`。R6B PR #61 已于 `2026-08-19T12:32:14Z` rebase merge；同一 R6B head 在合并前通过 `git diff --check` 与 `scripts/check-fast.ps1`。R7A 在独立工作树 `C:\git\ficant-release`、分支 `agent/r7a-layering-bitemporal` 开始，原 `C:\git\ficant` 的未跟踪审计报告保持不读写。

**实施允许写路径（冻结闭集）：**

- `docs/iterations/2026-08-r7a-layering-bitemporal.md`（本文件；实施后只允许在本节追加真实最终证据和第 7 节残余风险，不得改 execution base、允许路径或决策）
- `docs/iterations/README.md`（只更新当前迭代指针/阶段）
- `Cargo.lock`
- `interface/proto/ficant/market/v1/fact.proto`
- `crates/ficant-contracts/src/generated/**`
- `python/node-contracts/src/ficant_contracts/generated/**`
- `web-dm/packages/contracts-generated/src/**`
- `crates/ficant-contract-tests/Cargo.toml`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contract-tests/tests/r5d_layer_dependencies.rs`
- `crates/ficant-contract-tests/tests/r7a_core_extension.rs`（新建）
- `crates/ficant-contract-tests/tests/fixtures/r7a-core-extension/**`（新建）
- `python/tests/test_contract_import.py`
- `web-dm/platform-shell/tests/contracts-consumer.test.ts`
- `domain-packs/fictional-rates/**`（新建；只含虚构 fixture）
- `crates/ficant-fixed-income-native/tests/r7a_fictional_market_extension.rs`（新建）
- `crates/ficant-data/src/error.rs`
- `crates/ficant-data/src/canonical.rs`
- `crates/ficant-data/tests/canonical_ingestion.rs`
- `crates/ficant-data/tests/snapshot_publication_sit.rs`
- `crates/ficant-application/src/error.rs`
- `crates/ficant-application/src/ports/facts.rs`
- `crates/ficant-application/tests/review_round5.rs`
- `crates/ficant-storage/src/postgres/facts.rs`
- `crates/ficant-storage/src/postgres/positions.rs`
- `crates/ficant-storage/src/postgres/data_health_profiles.rs`
- `crates/ficant-storage/src/postgres/subjects.rs`
- `crates/ficant-storage/tests/postgres_repository.rs`
- `crates/ficant-storage/tests/r6a_governed_input_postgres.rs`
- `crates/ficant-storage/tests/r7a_bitemporal_postgres.rs`（新建）
- `crates/ficant-api/src/core_error.rs`
- `crates/ficant-api/src/market_fact.rs`
- `crates/ficant-api/src/snapshot.rs`
- `crates/ficant-api/tests/core_business_error.rs`
- `crates/ficant-api/tests/market_fact_service.rs`
- `crates/ficant-api/tests/snapshot_service.rs`
- `crates/ficant-acceptance/tests/phase1_business_loop.rs`
- `cpp/fixed-income-kernel/CMakeLists.txt`
- `cpp/fixed-income-kernel/tests/r7a_raw_numeric.cpp`（新建）
- `scripts/check-cross-clang.ps1`（新建）
- `scripts/test-cross-clang-check.ps1`（新建）
- `scripts/check-fast.ps1`
- `scripts/check.ps1`

**受保护事实：** `crates/ficant-domain/src/primitives/**`、`crates/ficant-domain/src/research/**`、`crates/ficant-domain/src/subject.rs`、`crates/ficant-domain/src/analytics.rs`、`curves.rs`、`futures_delivery.rs`、`futures_hedge.rs`、`crates/ficant-fixed-income-native/src/**`、`crates/ficant-kernel-sys/build.rs`、`src/**`、`cpp/fixed-income-kernel/include/**` 与 `src/**` 全部只读并由 R7A manifest 绑定；private authority、ignored `SPEC.md`/`ACCEPTANCE.md`/`MANUAL.md`、审计报告、Golden/Oracle/expected/容差、CI/CD/部署/版本文件不修改。

本节以下位置只在最终 R7A 候选完成后追加真实命令、exit code、test count、compiler identity、bit manifest digest 与候选 Git identity。任何尚未运行的命令都不得记为通过。

**实施期精确扩权（2026-08-19）：** Human 在两个既有机械门禁对 R7A 真实源码变化失败关闭后，明确批准且仅批准新增 `.github/scripts/license-inventory.lock.json` 与 `.github/scripts/verify-contract-generation.sh`。前者只以仓库既有 `refresh-bindings` 刷新 9 个受影响一方包的 `source_integrity`、输入树摘要与 inventory 摘要；648 个受控包、18 个 Cargo package 加 Python SDK 共 19 个一方包、许可证、来源、例外与 policy 均未改变。后者只把已锁定 descriptor SHA 更新为 R7A 最终 descriptor，既有 breaking baseline `6c805930f201b3d82bbcbee9030b791e48fb08e7`、Buf 版本、生成规则、consumer 与 CI 行为均未改变。原冻结清单保持原文，扩权不及于 workflow、supply-chain policy、版本、部署或 private authority。

**固定契约与消费者：** fixed Buf `1.56.0` 的 format/lint 均 exit `0`。两棵全新输出树各含 Rust `10`、Python `46`、TypeScript `28`，合计 `84` 个生成源文件；两树以及 tracked 生成树逐路径、规范化 bytes mismatch 均为 `0`。descriptor 两次均为 `183086` bytes、SHA-256 `e532049c3ad0651a2d28da699fd1582768ecf3f3c9bda79a441832401f18e184`。Rust descriptor inventory `20 / 20`、layer gate `3 / 3`、R7A core contract `2 / 2`，Python contract import `1 / 1`，固定 Node `22.17.0` 与 pnpm `10.12.4` 的 TypeScript focused consumer `1 / 1`，全部 exit `0`。

**AC04 与跨编译器裁决：** `cargo test --offline --locked -p ficant-fixed-income-native --test r7a_fictional_market_extension` exit `0`，`2 / 2`；虚构 market、RulePack、Subject 与 Bond/Calendar/Unit/Snapshot identity 经既有边界产生完整现金流和债券分析，RulePack/Subject 漂移均被消费。`r7a_core_extension` exit `0`，`2 / 2`：manifest 精确绑定 `47` 个 L0/L1/L2/native/kernel 生产源，文件集合和 SHA-256 与 execution base 相同，单 bit 反例被拒绝；受保护生产源实际 diff 为 `0`，因此 R7A 裁决保持现有 crate 边界，不实施预防性拆分。`scripts/check-cross-clang.ps1` 与其单 bit 反例门禁均 exit `0`；Windows VS LLVM Clang `19.1.5` 与 Ubuntu 24.04 locked Clang `18.1.8` 对 bond/cashflows、curve、carry/roll、delivery、hedge 共 `71` 行正式状态、计数、整数与全部 `double` 的 key、顺序和 IEEE-754 bits 完全一致，canonical manifest SHA-256 为 `9d8699f60ab92943f8339ec2485f09396794c602b23d1835eae31eecb718929b`。

**AC11–AC13 聚焦证据：** MarketFact 与 Curve API focused suites 分别 exit `0`，`3 / 3` 与 `4 / 4`，覆盖缺失/未来 `knowledge_at`、timezone、跨知识时点 cursor、未知或漂移 visible time 与 SQL/payload/blob 篡改的 fail-close；`core_business_error` exit `0`，`5 / 5`，证明非法源行映射为既有 typed field violation。`canonical_ingestion` exit `0`，`6 / 6`，`observed_at_after_visible_at` 携规范化安全行 id，且 snapshot/stage/blob/repository 写计数均为 `0`。最终 PostgreSQL focused 命令中，Fact window、Curve publication、`r7a_bitemporal_postgres` 均 exit `0`、各 `1 / 1`；后者逐路验证 Position、DataHealth 与 SubjectState 的早一纳秒、边界、晚一纳秒以及 SQL/payload visible drift。`snapshot_publication_sit` exit `0`，`1 / 1`：真实 PostgreSQL + Ceph-compatible 存储完成授权导入、发布和重启，外部源销毁后再次访问确实失败，而 `VerifiedSnapshotReader` + codec 不再读取源并重建 schema、列、顺序、值与原始 RecordBatch 完全相同的结果。

**供应链与统一入口：** 强绑定命令 `verify-license-inventory.py verify-bindings ... --require-first-party --require-native-lf` exit `0`，最终 inventory SHA-256 为 `99ec10f814d1dfe835f9a647fce424973c4b8f8a86e608e12c7a4ce4fcea08a9`，input-tree SHA-256 为 `cede1165c9fa971b6d8231768ec7f134cc59a5cc68e8a9aa9b73c565097de4ed`。`scripts/check-fast.ps1`、`scripts/check.ps1`、`scripts/check.ps1 -IncludeIntegration` 均 exit `0`；完整入口同次通过现有 R4D/R5D/R5E Oracle、Rust/Python/TypeScript/C++ 回归、Clang `71` 行逐位比较及 R6 生产面。Integration 部分实际执行 `37` 个用例，包括 migration `7 / 7`、negative invariants `13 / 13`、Phase 3B source-destruction `1 / 1`、R6A input-plane `1 / 1` 与 R6B Artifact production SIT `1 / 1`。

**范围与交付状态：** execution base 到候选的 tracked/untracked 并集为 `40` 个路径，全部位于冻结清单与上述两项 Human 扩权的并集，unauthorized `0`；`Cargo.lock` 与 `crates/ficant-data/src/lib.rs` 实际内容 diff 为 `0`，47 个受保护生产源、Golden/Oracle/expected/容差、private authority、ignored authority 与审计报告均无改动。`git diff --check` exit `0`（仅报告既有 Windows LF→CRLF 工作树提示）。R7A acceptance sentence 已由公共技术候选满足；在该候选进入公共 Git 精确提交并由 private authority 独立绑定前，不宣称 AC04、AC11–AC13 正式点亮。未创建版本/tag、镜像或部署，未触发远端 CI/CD，也未开始 R7B。

## 7. 残余风险

- R7A 只完成 AC04 与 AC11–AC13 的公共技术实现和重取证；正式点亮仍要求精确公共提交进入 `main`，随后由 private authority 独立复核并绑定。该动作完成前不得开始 R7B。
- R7B 的 AC30–AC33 仍全部未实施：完整 code/image/input 递归血缘、确定性身份、outbox/crash/orphan/灾备恢复、clean-environment `MANUAL.md` 与运行手册都不是本轮能力。
- 通用 Python contract suite 中的 live-server 用例继续按环境门禁 skip；统一入口另行执行固定 Phase 2E live SDK `1 / 1`，本轮没有把 skip 冒充 live 覆盖。
- PostgreSQL 时间列保持既有微秒存储边界；公开 `MarketTime`、fingerprint、cursor 与解码后校验仍绑定 seconds、nanos、timezone 和 local date，R7A 没有另行迁移数据库时间精度。
- AC04 硬零改动门禁已经通过，因此“不拆 crate”是本轮机械裁决；这不排除未来由真实依赖或部署证据触发新的 Human 架构决策。
