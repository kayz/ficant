# R5c 迭代 brief — 数据健康度查询

**迭代：** R5c · **承接条目：** AC36 · **execution base：** `2f84601f0b0558d60b79f630e8cdf03e3ff92311` · **authority base：** `577b107efa0e5fd8f272d115e8ea869ef2b93f21`

本 brief 是 R5c 面向 Human 的唯一设计与最终证据载体。R5 已拆为 `R5a（AC15）→ R5b（AC35）` 与 `R5a（AC15）→ R5c（AC36）`；R5a、R5b 均已完成公共 rebase merge、authority 精确绑定与 Human 批准。本轮只建立一个无状态、可查询、带完整输入绑定的 `DataHealthService`，把空持仓、大量 UNKNOWN 会计分类、陈旧快照及 R5a 价格来源风险转成具体预警；健康报告不进入任何既有计算调用链，不改变或替代既有失败关闭。本文冻结验收、非目标、公共契约、测试与逐文件写路径；2026-08-03 Human 已批准 §5 全部方向并授权开始实现。

## 1. 目标

把 ADR-0017 §三的数据健康度自检落成 ADR-0016 形态的一等 `AnalyticsService`：请求精确绑定一个不可变 `PositionSnapshot` 并可选绑定一个 verified `DataSnapshot`；平台按 snapshot owner 与 `evaluated_at` 从管理员发布的不可变配置中唯一解析 active 阈值 profile。响应列出机器可判的具体 issue、实际检查范围、request fingerprint、coverage 与 lineage。健康查询只读取现有 repository 与 verified blob，不写状态、不产生 journal、lease 或调用方可控的隐式覆盖；profile 发布是独立的管理员权限入口。

R5c 的首批检查精确限定为：空 `PositionSnapshot.positions`；UNKNOWN 会计分类的数量、比例与具体 position id；PositionSnapshot 陈旧度；可选 DataSnapshot 的陈旧度；R5a DataSource 未标 `PriceSourceType`；纯 `MODEL_VALUATION` 记录比例。无 data snapshot 时价格检查必须显式标成未执行，不能冒充健康；若提供，则必须走现有 verified-read、canonical decoder 与 exact DataSource 解析，完整性或 owner 漂移继续失败关闭。

`DataHealthReport` 会跨仓位统计数量与比例，按 R5b 已批准的“凡对多个仓位做聚合的响应都是组合级”定义，它是第 4 个 composition carrier，必须携带 `CoverageDeclaration`。显式空快照只允许健康报告使用 `0 / 0` coverage，并必须同时携带闭枚举正性证明 `VERIFIED_EMPTY`；默认构造的 coverage 或 `UNSPECIFIED` 状态不得被解释为空。既有 `PortfolioKeyRateExposure`、`PositionViews`、`CapitalUse` 的非空 coverage 不变量不变。R5b inventory 中零使用的 `SINGLE_POSITION` 不为本轮预留：R5c 删除该枚举成员，新增健康报告 success arm 归为 `Composition`，管理员发布 profile 的 success arm 归为 `ACK_OR_ECHO`，最终闭集为 68 个 success arm、4 个 composition carrier、64 个使用 3 个实际理由的非组合 arm。

**Acceptance sentence：**

> 给定精确绑定且可验证的 PositionSnapshot 与可选 verified DataSnapshot，平台按 owner 与 `evaluated_at` 唯一解析管理员发布、已验证且处于可见/有效区间的不可变阈值 profile；请求不能提交或选择 profile，缺项或多项命中均失败关闭。`GetDataHealthReport` 对空持仓、UNKNOWN 比例达到阈值、PositionSnapshot 陈旧、DataSnapshot 陈旧、DataSource 未标型或模型估值比例达到阈值逐项返回稳定排序的具名 issue，并指明具体 position id、exact DataSource ref、计数、比例或时长；缺席的可选价格快照明确标为未检查而不是健康。陈旧判定使用完整 `MarketTime` 精度，阈值加 `1ns` 即预警；向上取整的 `observed_age_seconds` 只作展示与 report-hash 输入。健康发现、position-set state 与 coverage 由同一次 snapshot evaluation 产生；合法空集合同时满足已验证空 snapshot hash、`VERIFIED_EMPTY` 与 `0 / 0` coverage，默认 coverage 注入必须失败。报告绑定 owner、Subject、评估时间、输入 hash、完整 profile、request fingerprint、content hash 与 lineage；profile 的 exact ref / hash 同时进入 fingerprint 与 LineageRef，相同输入逐位相同，平台换 active profile 必须换 fingerprint、report hash 与 lineage。对非空、100% UNKNOWN 且陈旧、但 Definition / 曲线 / RulePack / ActiveQuote 均合法的 Bond+Futures snapshot，健康报告返回 WARNING，`CalculateKeyRateDv01` 仍返回非 error、非 partial 的完整 exposure，而 `CalculateCapitalUse` 仍按 AC17 失败关闭。对同一字节级计算请求，健康查询前后的完整响应 bytes 逐位相同；与经济输入相同但分类完整、时间新鲜的健康对照相比，最差可计算 fixture 的 KRD 数值口径不变，algorithm identity 固定为 `ficant.fixed-income.portfolio-key-rate-yield` version 1、convention profile 固定为 `linear-ytm-fixed-base-ctd-v1`。空 snapshot 可被发布和报告为预警，但没有可计算仓位时既有聚合仍可按自身合同失败，这不是健康服务阻断。DataHealthReport 成为第 4 个 coverage carrier，descriptor 闭集为 68 / 4 / 64，删除其 coverage 或新增未分类 success arm 均使门禁 exit 1；零使用 `SINGLE_POSITION` 被删除。Golden、Oracle、Phase 2C/2D matrix、canonical quote v1/schema/hash、R5a 来源类型、既有数值公式、既有 migration `0001–0021`、allowlist、UI 与 authority 均不变；migration 只加 `0022` 平台 profile 元数据表。

## 2. 验收

| 条目 | R5c 可执行判据 |
|---|---|
| AC36 · 空持仓 | 允许发布 lineage 非空、positions 为空的 immutable PositionSnapshot；健康请求成功，状态为 WARNING，精确包含 `EMPTY_POSITIONS`，`position_set_state=VERIFIED_EMPTY`，coverage 为 imported `0` / participating `0`、两组 gross 列表为空。该状态只能由重新核验 content hash 后确含零仓位的 snapshot 产生。向健康路径注入默认 `CoverageDeclaration`、`UNSPECIFIED` 状态、非空 snapshot + `VERIFIED_EMPTY` 或空 snapshot + `NON_EMPTY` 均失败关闭。缺 lineage 仍失败。PositionViews / CapitalUse / Portfolio KRD 不因健康服务而获得零值或 partial success。 |
| AC36 · UNKNOWN | 构造 4 个仓位、其中 2 个 UNKNOWN，profile 阈值为 5000 bps；报告列出 2 个稳定排序的 position id、count `2`、ratio `5000` 并预警。阈值比较使用整数交叉乘法，等于阈值即触发，不做 Decimal 或浮点舍入。 |
| AC36 · 陈旧 | `evaluated_at - observed_at` 严格大于 profile 的秒级上限才触发 `STALE_POSITION_SNAPSHOT`；可选 DataSnapshot 对 `as_of` 使用独立上限。等于上限不触发，阈值加 `1ns` 即触发，早于 observed/as_of 的评估时间失败关闭。判定直接比较完整 `MarketTime` duration，不经过整数秒截断；`observed_age_seconds` 对正的亚秒余数向上取整且只作展示与 report-hash 输入。 |
| AC36 · 价格健康 | 未提供 DataSnapshot 时 `price_evidence_evaluated=false` 且不生成价格健康结论。提供时先验证 metadata/blob/hash/owner/visible_at，再解码 canonical quotes 并 exact 解析唯一 DataSource；legacy 未标型生成 `UNTYPED_PRICE_SOURCE`，模型估值比例达到 profile 阈值生成 `MODEL_VALUATION_SHARE`，issue 指明 exact VersionRef 与记录数。损坏 blob、hash 漂移、owner/version 不符仍返回 typed error，不降格为 warning。 |
| AC36 · 不阻断 | 构造“最差可计算 fixture”：非空 Bond+Futures snapshot 的会计分类 100% UNKNOWN 且陈旧，健康报告同时包含 UNKNOWN / STALE warning；Definition、曲线、Factor、RulePack、ActiveQuote 与其他计算必需输入保持合法。对它调用 `CalculateKeyRateDv01` 必须返回 success `exposure`，positions、totals 与 coverage 均完整，不能返回 error、空结果、跳过仓位或 partial。对同一 snapshot 调用 CapitalUse 仍按 AC17 失败关闭，证明健康预警不覆盖“缺了就一定算错”的既有边界。 |
| AC36 · 健康评估无副作用 | 对同一字节级 `CalculateKeyRateDv01Request` 先取完整响应，执行一次会返回 WARNING 的健康查询，再原样执行该计算请求；前后 response protobuf bytes、Decimal、UnitRef、algorithm、hash、lineage 与 coverage 逐位相同。调用计数证明健康查询未调用或包装任何计算 engine，且没有写状态、缓存污染或改变后续读视图。 |
| AC36 · 不静默降级 | 另建经济输入、Instrument / Factor / Curve / RulePack / quote 完全相同但分类完整、时间新鲜的健康对照 snapshot。健康与最差可计算 fixture 的 KRD position / totals 数值逐位相同（snapshot id、result hash 与 lineage 因输入身份不同而不作相等要求）；两边 `algorithm_id` 都必须精确为 `ficant.fixed-income.portfolio-key-rate-yield`、version `1`、`convention_profile` 都必须精确为 `linear-ytm-fixed-base-ctd-v1`。不得因 UNKNOWN 比例或陈旧度切换简化模型、不同 convention 或降级算法。 |
| AC35 · 回归与同源 | `DataHealthReport.coverage` 为非空消息；健康 issue、position-set state、count 与 coverage 必须由一次 snapshot 遍历生成的同一不可变 evaluation 值共同构造，禁止分别遍历。构造 issue count / position ids 与 coverage 分母不一致的 pair 必须失败。非空 snapshot 以全部 positions 作为 imported / participating，gross 按 exact UnitRef 稳定分组，missing critical 为 0。空 snapshot 只在 health carrier 且带 `VERIFIED_EMPTY` 时允许 0 / 0；默认 coverage 与既有三个 carrier 仍拒绝空分母。健康 coverage 的 `source_confidence` 缺席且 external source count 为 0，因为价格记录只被检查、不参与金融数值结论；可选价格证据另由 exact ref、issue 与 lineage 承载，不能生成第二份可信度结论。coverage 进入报告 content hash。 |
| R5b 门禁 · 闭集 | descriptor inventory 新增且只新增 `GetDataHealthReport → DataHealthReport`（Composition）与 `PublishDataHealthThresholdProfile → DataHealthThresholdProfile`（AckOrEcho）两个 success arm；总数精确 68 / 4 / 64。删除 `SINGLE_POSITION` 后三个非组合理由均至少有一个真实 arm。既有六个负向 fixture 全保留，新增“删除 health coverage”第七个 fixture；七项均真实 exit 1，已分类 base inventory exit 0。 |
| 生产组合 | 真实 server builder、gRPC-Web mux 与生成客户端可命中 `DataHealthService/PublishDataHealthThresholdProfile` 与 `/GetDataHealthReport`；管理员发布使用 `data-health:configure`，只读查询使用 `data-health:read`。服务使用 Postgres profile/PositionSnapshot/DataSource repository、verified S3/blob 与 canonical decoder，不以 test-only repository 冒充生产路由。 |
| I3 / I8 · 绑定与确定性 | profile 发布时先验核验 exact VersionRef、content hash、owner、双时间、有效区间与 lineage，经 verified blob/snapshot 持久化；数据库拒绝同一 tenant/version 绑定不同内容。查询按 owner/time 唯一解析 active profile，exact ref/hash 进入 request fingerprint、report content hash 与 LineageRef。issue 按 `(code, position_id, data_source id/version)` 排序；所有计数、比例、展示时长、输入引用、position-set state、coverage 与 issue 进入 report content hash。相同请求及存储事实产生逐位相同 protobuf 输出；平台换 active profile 必须同时改变 fingerprint、hash 与 lineage。 |

R5c 闸门：

1. RED-first 分三次取得：domain / public contract；application 读取与 AC17 非阻断；transport / production / coverage inventory。先只加判据并取得真实非零 exit code，记录首个真实错误；RED 不是 checkpoint。每层只有对应直接测试转绿后才形成 forward-only checkpoint。
2. 空 snapshot 是显式事实，不由“找不到 snapshot”或 coverage 零值推断。只放宽 `PositionSnapshot` 的 positions 非空约束，owner、Subject、双时间、hash 与 lineage 不变量全部保留；空快照仍须通过现有 publish / exact read / persistence 往返。domain 只从重新核验 hash 的真实空 snapshot 产出 `VERIFIED_EMPTY` capability，默认 coverage 没有该 capability。
3. `evaluated_at` 是本次报告唯一知识与评价时点，必须不早于被读取对象的 `visible_at`，也不得早于其 observed/as_of。R5c 不新增独立 `knowledge_at`，不以系统当前时间参与结果。
4. 阈值 profile 必须由平台管理员以 `data-health:configure` 发布，携带 snapshot id、owner、exact VersionRef、完整内容、可见/有效区间、lineage 与先验正确的 hash；健康请求不接受 profile 内容、ref、snapshot id、环境默认或逐字段 override。服务按 owner/time 唯一解析 active profile，零个或多个命中均失败。换阈值必须换 exact version / hash，报告回显完整 profile。profile 的规范 bytes 必须进入 OperationFingerprint，exact ref + hash 必须形成 LineageRef；同一 ref/version 不同内容失败关闭。比例以 basis points `1..=10000` 表示；数量乘 10000 与分母乘阈值做 checked integer 比较，等于阈值触发。陈旧度以秒为配置单位、完整 instant 比较，严格大于阈值触发。
5. 可选 DataSnapshot 只扩大本次报告的已检查范围。没有价格快照时不访问 DataSource；有快照时必须复用 R5a 已验证的 exact source 事实，不重算第二套来源分类，也不从 dataset/name/quote 内容猜类型。当前一个 canonical snapshot 只绑定一个同质 DataSource version。
6. 健康 warning 与 typed failure 的边界按 ADR-0017 §四：在报告自身声明范围内仍可正确识别的缺口转 warning；无法信任输入内容的 integrity、hash、owner、version、time 或 profile 错误失败关闭。既有 CapitalUse、KRD、rates 与 delivery 的失败关闭一行不改。
7. DataHealthReport 是 composition carrier。一次 `evaluate_position_snapshot` 必须同时产出 issue facts、`PositionSetState` 与 coverage；不得分两次遍历或让 transport 重算。health empty constructor 必须消费 `VerifiedEmptyPositionSnapshot` capability，只允许 `VERIFIED_EMPTY + 0 / 0 + empty gross + missing=0 + no source confidence + external source count=0`；默认构造与任何 state / count / hash 不一致均失败。不得放宽既有 `for_complete_positions`，非空报告使用既有 gross 派生逻辑。
8. “不阻断”必须由最差可计算 fixture 的真实计算 success arm 证明，不由依赖图或健康 RPC 自身成功推断。该 fixture 固定为非空、100% UNKNOWN、陈旧的 Bond+Futures snapshot，但所有 KRD 必需 Definition / Factor / Curve / RulePack / ActiveQuote 合法；KRD 返回完整 positions / totals / coverage，CapitalUse 对 UNKNOWN 继续失败关闭。
9. “健康评估无副作用”单独证明：DataHealthUseCase 不被注入 PositionViews、CapitalUse、PortfolioRisk 或 Rates；同一字节级计算请求在一次 WARNING 健康查询前后的完整 response bytes 逐位相同，不写状态、不污染缓存或读视图。
10. “不静默降级”单独证明：最差可计算 fixture 与经济输入相同的健康对照分别执行 Bond+Futures KRD；数值 position / totals 逐位相同，algorithm identity 均固定为 `ficant.fixed-income.portfolio-key-rate-yield` version 1，convention profile 均为 `linear-ytm-fixed-base-ctd-v1`。不得按 UNKNOWN 比例、陈旧度或 health state 选择另一算法。
11. coverage gate 的 expected / inventory 变化必须先由新增公共 RPC 使旧闭集真实 RED，再按本 brief 的 Human 授权精确新增一个 Composition arm；执行期获批的平台发布 RPC 再机械增加一个 AckOrEcho arm。不得删除旧 arm、改已有分类或放宽未知默认失败。`SINGLE_POSITION` 删除与 68 / 4 / 64 数字、七个负向 fixture须在同一可审阅 diff 呈现。
12. `interface/buf.gen.yaml` 只允许把新 `DataHealthService` 加入现有 Python gRPC `types` 闭集；插件、version、revision、out、option 及其他 service 条目不变。固定 Buf 1.56.0 必须在两个独立临时树生成并逐文件比较，再同步点名输出。
13. `grpc_web.rs`、server `lib.rs`、descriptor inventory、coverage gate 与 fixture 是自管门禁 / 生产入口，base-to-candidate diff 必须单独呈现且只做本轮加法或收紧。不得修改 expected、Oracle、Golden、matrix、canonical hash、allowlist、既有分层断言或容差制造通过。

## 3. 非目标

- ADR-0017 表中主体无状态快照、Instrument / 现金流条款缺失、行情源注册后无数据、规则包过期、多源分歧等其余检查；本轮不以空 issue 冒充已检查。尤其“有仓位无 Instrument / 有 Instrument 无现金流条款”的完整性类与“多源分歧超阈值”的一致类明确顺延到 v0.2 DataHealth 扩展，进入 v0.2 前必须在路线表落位，不能因 AC36 点亮而消失。
- 自动阻断、自动降级、自动选源、健康评分、严重度多级、告警持久化、通知、后台扫描、定时任务、RunJournal、lease、缓存或状态机；R5c 只有同步幂等查询。
- 环境变量默认、调用方选择 profile、按主体/租户的隐式 override、partial override 或 server 常量。平台 profile 只有管理员发布与 owner/time 唯一 active 解析；本轮不做可变配置、删除、覆盖或后台分发。
- WebApp、报告模板或 UI 警示呈现；本轮“可见”只指生成客户端可查询的 gRPC / gRPC-Web 成功响应。固定生成的 TypeScript contract 不等于 UI 已实现，ADR-0017 §三“结论必须可见”的呈现层义务在 UI 轮次前仍属未完全履行。
- 修改 PositionViews、CapitalUse、Portfolio KRD、Rates、CTD、税后、资本、会计、Factor、RulePack、canonical decoder 语义或任何数值公式；健康结论不能进入这些请求或结果 hash。
- 记录级混合 DataSource、canonical v2、额外 source type、DataSource migration 或修改 R5a 的单 source 同质约束。
- 除获批的 `0022_data_health_threshold_profiles.sql` 外的新 migration、policy / constraint / factor / position proto、C++ / C ABI / native、domain-packs、税制、AC09、AC37、authority 三件套、SPEC、ACCEPTANCE、MANUAL、任何 ADR、版本 tag、CI/CD 或发布。
- PR #12 后续“新门禁收紧判别测试”的 authority 治理补录；它应保持独立窄 PR，不能混进 R5c 公共候选或 AC36 验收。

## 4. 公共契约变化

- 新增 `interface/proto/ficant/research/v1/health.proto`，只 import core common/error、research coverage 与 market data-source 类型，不新建 `policy.proto`。新增 `DataHealthService.GetDataHealthReport`；请求 / 响应 envelope 与 service 方法均为加法。
- 新增完整、内容寻址的 `DataHealthThresholdProfile`：

| tag | 字段 | 类型与冻结语义 |
|---:|---|---|
| 1 | `profile_ref` | `ficant.core.v1.VersionRef`；exact immutable identity，version 必须大于 0。 |
| 2 | `max_position_snapshot_age_seconds` | `uint64`；完整 instant 差严格大于该值时预警。 |
| 3 | `unknown_accounting_warning_basis_points` | `uint32`；范围 `1..=10000`，比例等于阈值即预警。 |
| 4 | `max_data_snapshot_age_seconds` | `uint64`；只在绑定 DataSnapshot 时使用。 |
| 5 | `model_valuation_warning_basis_points` | `uint32`；范围 `1..=10000`，比例等于阈值即预警。 |
| 6 | `content_hash` | `ficant.core.v1.Sha256`；覆盖完整 profile 规范编码，必须先验精确。 |
| 7 | `profile_snapshot_id` | `ficant.core.v1.Ulid`；不可变配置 snapshot 身份。 |
| 8 | `owner` | `ficant.core.v1.OwnerRef`；决定 active profile 的平台所有者边界。 |
| 9 | `visible_at` | `ficant.core.v1.MarketTime`；配置成为平台可见事实的完整知识时点。 |
| 10–11 | `effective_from` / `effective_to` | `ficant.core.v1.MarketTime`；半开有效区间。 |
| 12 | `lineage` | `repeated ficant.core.v1.LineageRef`；按规范顺序进入 profile hash，报告回显并并入结果 lineage。 |

- 新增 `DataHealthState`：`UNSPECIFIED = 0` 仅为 wire 默认且请求 / 结果不可接受，`HEALTHY = 1`、`WARNING = 2`。新增 `PositionSetState`：`UNSPECIFIED = 0` 无效、`NON_EMPTY = 1`、`VERIFIED_EMPTY = 2`；它是合法空 coverage 的正性证明，不从计数反推。新增封闭 `DataHealthIssueCode`：`EMPTY_POSITIONS`、`UNKNOWN_ACCOUNTING_CLASSIFICATION`、`STALE_POSITION_SNAPSHOT`、`UNTYPED_PRICE_SOURCE`、`MODEL_VALUATION_SHARE`、`STALE_DATA_SNAPSHOT`；本轮不设 `OTHER`。
- 新增 `DataHealthIssue`：`code = 1`、`affected_position_ids = 2`、`data_source_ref = 3`、`record_count = 4`、`ratio_basis_points = 5`、`observed_age_seconds = 6`。不适用字段必须取规范空值；列表与 issue 全局稳定排序，不携带不可判定的自由文本结论。
- 新增 `GetDataHealthReportRequest`：`subject_ref = 1`、`position_snapshot_id = 2`、可选 `data_snapshot_id = 3`、`evaluated_at = 4`，并 `reserved 5`。请求不接受 profile 内容、ref、snapshot id、现成 issue、health score、计算结果或逐字段 override。
- 新增 `PublishDataHealthThresholdProfileRequest`（`idempotency_key = 1`、完整 `threshold_profile = 2`）、对应 response oneof 与 `PublishDataHealthThresholdProfile` RPC；发布固定要求 `data-health:configure`，读取固定要求 `data-health:read`。
- 新增 `DataHealthReport`：`owner = 1`、`subject_ref = 2`、`evaluated_at = 3`、`position_snapshot_id = 4`、`position_snapshot_hash = 5`、可选 `data_snapshot_id = 6`、可选 `data_snapshot_manifest_hash = 7`、可选 `data_source_ref = 8`、完整 `threshold_profile = 9`、`state = 10`、`issues = 11`、`price_evidence_evaluated = 12`、`position_set_state = 13`、`coverage = 14`、`request_fingerprint = 15`、`content_hash = 16`、`lineage = 17`。响应回显完整 profile 而非只有 hash，使单独保存的报告仍可解释；profile 自身携带 exact ref 与 hash。`GetDataHealthReportResponse` 的 oneof 为 `report = 1` / `error = 2`。
- domain 新增不可变 threshold、issue、snapshot evaluation、verified-empty capability 与 report 值对象；profile hash、fingerprint、完整时间比较、展示秒数、比例、排序、issue / coverage 同源、position-set state 与 report hash 在 domain 校验。`PositionSnapshot` 允许 positions 为空但继续要求非空 lineage；Coverage 新增只消费 verified-empty capability 的 health empty constructor，既有 complete constructor 与三个 carrier 不变量保持。profile 的 exact ref + hash 加入报告 lineage。
- application 新增管理员发布与只读 DataHealth use case，复用 verified blob/snapshot、`PositionSnapshotRepository`、`CanonicalSnapshotDecoder` 与 `DataSourceRepository`，并新增 active profile repository port。storage 增加 `SnapshotValue::DataHealthThresholdProfile`、专用 proof/blob role、codec 与 PostgreSQL append-only profile repository；migration `0022` 以 tenant/version、snapshot id 与 idempotency key 唯一约束持久化身份。API 只做解析 / 映射，production server 组合真实 repository、writable/verified blob、integrity sink 与 decoder。
- Rust flat package机械更新 `ficant.research.v1.rs` / `.tonic.rs`；Python 新增 `health_pb2.py` / `health_pb2_grpc.py`；TypeScript 新增 `health_pb.ts`。`interface/buf.gen.yaml` 只把 `ficant.research.v1.DataHealthService` 加入 Python gRPC service 闭集。
- R5b coverage inventory 只做与新服务严格对应的前进：新增健康报告 success arm 为 Composition、管理员发布 success arm 为 AckOrEcho，删除零使用 `SinglePosition` reason；三个既有 composition arm与 63 个既有非组合 arm的分类逐项不变。coverage descriptor 总数从 66 / 3 / 63 变为 68 / 4 / 64。

## 5. 需 Human 决策

- **已裁决——显式空 snapshot 与正性证明：** Human 批准 lineage 非空的 PositionSnapshot 持有空 positions，使“空持仓”成为可发布、可 exact read、可复现的事实；不得把 repository `NotFound` 或 coverage 零值推断为空。合法 `0 / 0` 必须同时携带由真实空 snapshot hash 派生的 `VERIFIED_EMPTY`；默认 Coverage 注入、`UNSPECIFIED` 或 state / count / hash 不一致均失败。既有聚合没有参与项时仍按自身合同失败，不得返回伪造零值。
- **已裁决——阈值来源与覆盖性（后由执行期裁决收紧）：** 冻结设计曾允许请求携带完整 `DataHealthThresholdProfile`；PR #57 合并前的独立审查证明该形状把平台健康政策授权给研究调用方，因此本项已由下方“阈值改为平台部署配置事实”裁决取代。旧形状没有作为 AC36 终态证据，也未被追认为合规。
- **已裁决——阈值运算：** Human 批准 ratio 用 basis points 与 checked integer 交叉乘法，`actual >= threshold` 触发；age 用完整 instant，`actual > max` 触发。测试中的 5000 bps 与秒数只是明确 fixture，不宣称产品级默认。
- **已裁决——价格检查可选：** Human 批准 DataSnapshot 为可选 exact binding；缺席时 `price_evidence_evaluated=false`，既不 warning 也不 healthy，报告仍可判断持仓项。有值时才执行 R5a source 与 freshness 检查；不得自动选“最新”快照。
- **已裁决——第 4 个 composition carrier：** Human 批准按 R5b 既有定义把 DataHealthReport 归为 Composition 并携带 coverage；仅 health carrier 新增窄 `0 / 0` 空 coverage，现有三个 carrier 的正分母不变量保持。执行期新增管理员发布 RPC 后，inventory 精确前进到 68 / 4 / 64，新增第七个删 health coverage 负向 fixture。
- **已裁决——健康 coverage 的来源口径：** Human 批准 health coverage 只描述被检查的 PositionSnapshot 仓位集合，`source_confidence` 缺席、external source count 为 0；可选 DataSnapshot / DataSource 是健康检查对象，不是形成金融数值结论的价格输入，必须由报告 exact ref、issue 与 lineage 独立承载。这样不伪造 typed summary，也避免 legacy untyped source 被静默记成“未消费”。
- **已裁决——健康发现与 coverage 同源：** Human 批准 domain 对 PositionSnapshot 只做一次 evaluation，并从同一不可变结果同时构造 issue、position-set state、count 与 coverage；API / transport 不得二次遍历或重算。构造 issue count / ids 与 coverage 分母不一致的 pair 必须失败，复用 R5b 的同源纪律。
- **已裁决——AC36 三分判据：** Human 批准把“不阻断”“健康评估无副作用”“不静默降级”作为三条独立判据。最差可计算 fixture 必须真实返回完整 KRD；同一计算请求在健康查询前后 bytes 相同只证明无副作用；健康 / 最差 snapshot 的 algorithm identity 与 convention profile 相同则证明没有按健康条件切换简化模型。三条不能相互替代，且 CapitalUse 的 AC17 失败关闭继续优先。
- **已裁决——删除零使用理由：** Human 批准删除 `NonCompositionReason::SinglePosition`。当前分布 `30 / 21 / 12 / 0` 证明它没有 arm 作证；健康报告不使用它。未来出现真正单仓位 success arm 时再以同一提交加回成员和实际分类，不能预造逃逸桶。
- **已裁决——首批检查与顺延边界：** Human 批准只承接 §1 六类 issue；ADR-0017 其余检查不得以 `price_evidence_evaluated=false` 或空列表声称已经检查。其中完整性类“有仓位无 Instrument / 有 Instrument 无现金流条款”和一致类“多源分歧超阈值”明确进入 v0.2 DataHealth 扩展，并须在 v0.2 execution base 前落入路线表。
- **已裁决——“结论可见”口径及未完全履行：** Human 批准本轮以 server / gRPC / gRPC-Web 成功响应作为 AC36 的可查询、可见证据；WebApp / 报告 UI 不在本轮。Human 必须在未来 AC36 authority 候选中以独立裁决项签署该限定，不得把它并入 AC36 验收句顺带批准；同一裁决须记录 ADR-0017 §三的“结论必须可见”在呈现层落地前尚未完全履行，不能因 AC36 点亮而把 UI 债务视为完成。UI 警示须由后续独立呈现层迭代承接。
- **已裁决——自管门禁精确变化：** Human 授权在新增 RPC 令旧闭集真实 RED 后，把 descriptor inventory 精确增加一个 Composition arm、删除零使用 reason并新增删 health coverage fixture；执行期批准的平台发布 RPC 再精确增加一个 AckOrEcho arm，最终数字为 68 / 4 / 64。既有 66 arm、六个负向 fixture、未知默认失败与其余分类保持；这不是因业务测试失败而 rebaseline。
- **执行期事前裁决——阈值改为平台部署配置事实：** PR #57 独立审查证明，请求内完整 profile 即使自校验 content hash，也允许研究调用方自行决定什么算不健康，并允许同一 `VersionRef` 跨请求绑定不同内容；这同时弱于 SPEC I10 的“平台自检”、SPEC §5 的管理员写入边界与本 brief 的 immutable identity 声明。Human 明确否决把身份放宽为 `(VersionRef, content_hash)`，批准在本轮合并前改为平台管理员发布、平台唯一解析。`GetDataHealthReportRequest` 删除并 `reserved` 原 `threshold_profile = 5`，不接受 profile 内容、ref、snapshot id 或逐字段 override，调用方因而既不能改阈值也不能挑选旧版本。服务按已验证 PositionSnapshot 的 exact owner 与 `evaluated_at` 唯一解析一个 `visible_at <= evaluated_at` 且 `effective_from <= evaluated_at < effective_to` 的 active profile；零个命中以缺项失败关闭，多个命中以 immutable/configuration violation 失败关闭，不按排序猜一个。
- **执行期事前裁决——profile 的分层与持久化形状：** `DataHealthThresholdProfile` 是 L2/L3/L4 之外、因部署治理而变化的平台配置事实；它不伪装成 `MarketRulePack`、Subject 或市场定义。实现复用既有 immutable verified-blob / snapshot / content-hash / lineage 基础设施，新增闭类型 `SnapshotValue::DataHealthThresholdProfile`、专用 proof kind/blob role、PostgreSQL append-only 元数据与 exact resolver，而不建立第二套 blob 或幂等机制。profile 增加 `profile_snapshot_id`、owner、`visible_at`、半开有效区间与 lineage；数据库以 `(tenant, profile id, profile version)` 唯一约束阻止同一版本重绑，snapshot id 另行唯一。闭 variant、proof kind 与 codec tag 已提供比自由字符串 type URL 更强的类型判别，因此本轮不新增可拼写错误的 type URL。平台管理员仅能经新增 `PublishDataHealthThresholdProfile` 与固定 `data-health:configure` scope 发布；健康读取仍使用 `data-health:read`。
- **执行期事前裁决——完整时间比较与展示秒数：** Human 批准陈旧判定直接比较完整 `MarketTime` duration 与整数秒阈值，不先调用 `num_seconds()`；`threshold + 1ns` 必须预警。`observed_age_seconds` 只作确定性展示与 report-hash 输入，对任何正的亚秒余数向上取整；该取整不得反馈到是否预警的判定路径。
- **执行期事前扩权——平台 profile 闭环的精确新增写路径：** Human 要求两项独立审查缺口均在 PR #57 合并前解决，并授权以下路径作为 §6 冻结清单之外的 forward-only 扩权；原 §6 清单保持原文不变。新增路径只用于 profile snapshot variant/proof/fingerprint、管理员发布、active resolver、PostgreSQL codec/持久化、migration 与针对性测试：`crates/ficant-application/src/ports/data_health_profiles.rs`（新建）、`crates/ficant-application/src/ports/mod.rs`、`crates/ficant-application/src/ports/snapshots.rs`、`crates/ficant-application/src/ports/fingerprint.rs`、`crates/ficant-application/src/ports/execution.rs`、`crates/ficant-application/src/ports/rule_pack_resolution.rs`、`crates/ficant-application/src/use_cases/data_snapshot.rs`、`crates/ficant-application/src/use_cases/phase1_business_loop.rs`、`crates/ficant-application/src/use_cases/position_views.rs`、`crates/ficant-storage/src/postgres/data_health_profiles.rs`（新建）、`crates/ficant-storage/src/postgres/mod.rs`、`crates/ficant-storage/src/postgres/snapshots.rs`、`crates/ficant-storage/src/postgres/codec.rs`、`crates/ficant-storage/src/postgres/positions.rs`、`crates/ficant-storage/src/postgres/runs.rs`、`crates/ficant-storage/src/postgres/common.rs`、`crates/ficant-storage/tests/data_health_profile_postgres.rs`（新建）、`crates/ficant-storage/tests/migration_acceptance.rs`、`migrations/postgresql/0022_data_health_threshold_profiles.sql`（新建）。`0001–0021` 必须逐 blob 不变；migration tree 只允许加 `0022`。若编译证明另有 exhaustive-match 路径必须机械接入新 variant，必须在首次写入前继续追加本条记录，不得先改后补。
- **authority 边界：** agent 不修改私有 authority。公共候选独立审查并 rebase merge 后，authority 必须以新 public SHA 重冻；Human 在同一 authority 候选中先独立签署“wire 可见且呈现层尚未完全履行”裁决，再复核 AC17 / AC35 回归、点亮 AC36 并同步 MANUAL。若批准，进度由 v0.1 `19 / 30` 变为 `20 / 30`、全表由 `19 / 36` 变为 `20 / 36`；该点亮只证明 AC36 三个点名场景，不宣称 ADR-0017 六类检查全部完成。

## 6. 最终真实测试证据

**双 base 冻结：** 2026-08-03 在专用公共 worktree `C:\git\ficant-r5c-data-health` 确认 branch 为 `codex/r5c-data-health`、工作区干净且 `HEAD == origin/main == 2f84601f0b0558d60b79f630e8cdf03e3ff92311`。authority worktree `C:\git\ficant-authority-r5c-base` 干净、detached，`HEAD == origin/main == 577b107efa0e5fd8f272d115e8ea869ef2b93f21`；`verify-authority.ps1 -ExpectedAuthorityCommit 577b107efa0e5fd8f272d115e8ea869ef2b93f21` exit 0，三件套哈希成立，manifest 精确绑定公共 `2f84601f0b0558d60b79f630e8cdf03e3ff92311`。以上双 base 自此固定不变；根主克隆不承担 execution base、审计或新 worktree 起点角色。

**冻结允许写路径（精确文件；本节自此不得就地改写）：**

- `binaries/ficant-server/src/lib.rs`
- `binaries/ficant-server/tests/data_health_sit.rs`（新建）
- `crates/ficant-api/src/data_health.rs`（新建）
- `crates/ficant-api/src/grpc_web.rs`
- `crates/ficant-api/src/lib.rs`
- `crates/ficant-api/tests/data_health_service.rs`（新建）
- `crates/ficant-application/src/lib.rs`
- `crates/ficant-application/src/use_cases/data_health.rs`（新建）
- `crates/ficant-application/src/use_cases/mod.rs`
- `crates/ficant-application/tests/r5c_data_health_contracts.rs`（新建）
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.tonic.rs`
- `crates/ficant-domain/src/research/coverage.rs`
- `crates/ficant-domain/src/research/data_health.rs`（新建）
- `crates/ficant-domain/src/research/mod.rs`
- `crates/ficant-domain/src/research/position_snapshot.rs`
- `crates/ficant-domain/tests/r5c_data_health_contracts.rs`（新建）
- `crates/ficant-storage/tests/position_snapshot_postgres.rs`
- `docs/iterations/2026-08-r5c-data-health.md`（新建）
- `docs/iterations/README.md`
- `interface/README.md`
- `interface/buf.gen.yaml`
- `interface/proto/ficant/research/v1/health.proto`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/health_pb2.py`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/health_pb2_grpc.py`（新建）
- `python/tests/test_contract_import.py`
- `scripts/check-coverage.ps1`
- `scripts/test-coverage-check.ps1`
- `web-dm/packages/contracts-generated/src/ficant/research/v1/health_pb.ts`（新建）

**禁止写路径：** 所有未逐项列出的路径。特别禁止 authority 三件套与公共根目录同名废副本、所有 ADR、`README.md`、既有 brief、`docs/architecture/layering-refactor.md`、除新 `health.proto` 外的 proto、除五个点名文件外的 generated output、application ports、storage product code、migration、canonical decoder、DataSource registry、PositionViews / CapitalUse / PortfolioRisk / Rates / delivery 实现、C++ / C ABI / native、`domain-packs/**`、除两个 coverage gate 外的 `scripts/**`、Golden、Oracle、Phase 2C/2D matrix、`Cargo.lock`、`.gitignore`、`.github/**`、`cicd.yml`、`deploy/**`、`web-dm/platform-shell/**` 与 `web-dm/webapps/**`。扩权只能由 Human 在首次写入前批准并新增 §5 记录；本节不得追认。

**受保护 base 事实（Git object ID，实施期必须保持不变）：**

- `scripts/layering-allowlist.json`：blob `fe51488c7066f6687ef680d6bfaa4f7768ef205c`，内容 `[]`
- `scripts/check-layering.ps1`：blob `2667711af71bc74634042c9707def0b79b402029`
- `scripts/test-layering-check.ps1`：blob `7b4fefbde138ed2985afe20159dfc8bc48c98d2e`
- `crates/ficant-data/src/canonical.rs`：blob `79e42b00c645710b8179d515ba02f79cd9d38fc4`
- `tests/golden-cases`：tree `11f981972612e617591de1c3daaa36d114a7cab9`
- `tests/oracle`：tree `539889f598c8118854ea679375695c9721696932`
- `tests/phase2c/acceptance-matrix.json`：blob `26e72186490a0ab2cae142c9d88436ae07cc8da8`
- `tests/phase2d/acceptance-matrix.json`：blob `d6feaed93a8df00176f2873d28d1e03d6d789f75`
- `cpp`：tree `e600f8de0a485d5db5edf7eac20e5ea89698716f`
- `crates/ficant-kernel-sys`：tree `3350c80bdc3c54159fa9b4e6bb4e26ca33218f0d`
- `crates/ficant-fixed-income-native`：tree `3e622b8a1e4786a8183530d63a4d3d41be8a953b`
- `interface/proto/ficant/rates/v1/analytics.proto`：blob `ae49a0b44959f7ec42b2639ae4a5fd29ece94335`
- `interface/proto/ficant/research/v1/position.proto`：blob `529627703e3b4471e356a39020317a93cbbd4ed4`
- `interface/proto/ficant/research/v1/exposure.proto`：blob `eb4d3c8f1a0b07ef6fe67ce09919d5d0397a9c84`
- `interface/proto/ficant/research/v1/coverage.proto`：blob `d312abde89437a98a71c5aec2cd121866dea9ab3`
- `interface/proto/ficant/research/v1/factor.proto`：blob `ed998a2a142836e8eb17e8861e0bbf1fc3bad1ac`
- `interface/proto/ficant/market/v1/data_source.proto`：blob `b839db1346970060661d38ac17ff3d2e3b0a0c7f`
- `migrations/postgresql`：tree `b5940862a877e3e238128080c3478c6011b378a0`
- `domain-packs`：tree `96c67aeb92182260b65359d633481e826039e40e`
- `Cargo.lock`：blob `ec46fd45d980de5cad3f66c3dcd0e3d5ff6880ea`
- `docs/architecture/adr`：tree `09e21e9d7a3097757f2037ca0c3c9919763af683`
- `docs/architecture/layering-refactor.md`：blob `97a6f475dd7a4b9fcf8cbdd0ca1bcc4311a8bcff`
- `web-dm/platform-shell`：tree `7e77f86e9282489b8fc43a11e5b983978555783c`
- `web-dm/webapps`：tree `d7305d23a247ff010ec9fd34b5fa763dff717fd5`

**RED-first 与 forward-only checkpoint 计划：**

- domain / contract RED：只新增 domain 与 descriptor 判据，预期因 health 类型 / service 不存在、PositionSnapshot 拒绝空 positions、Coverage 不允许带正性证明的 0 / 0 而非零。转绿后必须证明 profile exact ref / hash、fingerprint / lineage、阈值边界、稳定 issue、单次 evaluation 同源、空 / 非空 coverage 与 report hash；默认 Coverage 注入及四类 state / count / hash 不一致全部失败，才形成 contract checkpoint。
- application RED：用 mocks 先断言六类 warning、verified DataSnapshot / exact DataSource 路径及 AC17 同一 snapshot 双调用，预期因 use case 不存在而非零。转绿后必须证明坏 hash / owner / time / profile ref-content drift 失败关闭，并分别取得：最差可计算 fixture 的完整 KRD success；WARNING 查询前后同一计算 response bytes 相同；健康 / 最差 pair 的 KRD 数值、R4d-b algorithm identity 与 convention profile 不变。三项与 AC17 回归全部通过后才形成 application checkpoint。
- transport / gate RED：先新增未带 coverage、未进 inventory 的 health success arm，固定 Buf descriptor 与 coverage gate 必须真实非零；不得先改 expected。冻结实现先按当时授权形成 67 / 4 / 63；执行期平台发布 RPC 获批后，再以同一闭集规则增加一个 AckOrEcho arm，最终生成输出、API mapping、生产 mux、68 / 4 / 64 闭集和七个负向 fixture全部转绿后才形成 transport / production checkpoint。

**最终针对性命令（实现获单独授权后，必须在同一候选逐条执行并填真实结果）：**

- `cargo test --offline --locked -p ficant-domain --test r5c_data_health_contracts`
- `cargo test --offline --locked -p ficant-domain --test r5b_coverage_contracts`
- `cargo test --offline --locked -p ficant-application --test r5c_data_health_contracts`
- `cargo test --offline --locked -p ficant-application --test position_snapshot_contracts`
- `cargo test --offline --locked -p ficant-storage --test position_snapshot_postgres -- --test-threads=1`
- `cargo test --offline --locked -p ficant-storage --test data_health_profile_postgres -- --test-threads=1`
- `cargo test --offline --locked -p ficant-storage --test migration_acceptance -- --test-threads=1`（仍须完整 4/4；`0001–0021` blob 不变，只新增 `0022`）
- `cargo test --offline --locked -p ficant-api --test data_health_service`
- `cargo test --offline --locked -p ficant-api --test position_snapshot_service`
- `cargo test --offline --locked -p ficant-server --test data_health_sit`
- 注入固定 Buf 1.56.0 后 `buf format --diff --exit-code interface` 与 `buf lint interface`
- 注入固定 Buf 1.56.0 后 `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory`（目标 20/20）
- `uv run --offline --locked --project python python -m pytest python/tests/test_contract_import.py -q`
- `pwsh -NoProfile -NonInteractive -File scripts/check-coverage.ps1`
- `pwsh -NoProfile -NonInteractive -File scripts/test-coverage-check.ps1`
- `pwsh -NoProfile -NonInteractive -File scripts/check-layering.ps1`
- `pwsh -NoProfile -NonInteractive -File scripts/test-layering-check.ps1`

固定 Buf 1.56.0 必须按 `interface/README.md` 在两个独立临时输出树执行完整生成并比较规范化 SHA-256；两次文件集合与内容完全一致后才机械同步 §6 点名的 Rust / Python / TypeScript 输出。`buf.gen.yaml` diff 必须单独证明只新增一个 Python gRPC service type。

**完整本地检查（最终候选必须真实执行）：**

- `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`
- 使用仓库锁定 Node 22.17.0、pnpm 10.12.4、Buf 1.56.0、uv / Python 3.12 执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1`
- 导入六个 Windows User 级 `FICANT_TEST_*` 变量且不输出值后，执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration`

**RED-first 真实结果：**

- domain / contract RED：先加入 `r5c_data_health_contracts`，执行 `cargo test --offline --locked -p ficant-domain --test r5c_data_health_contracts` exit 1；首个真实错误是 health 类型及构造入口尚不存在。固定 Buf 1.56.0 后先加入 descriptor 精确断言，`cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory` exit 1，首个失败指向缺失的 DataHealth enum / message / service；生成 service 后、闭集尚未更新时全量 descriptor 为 `18 / 20`。两次 RED 均未作为 checkpoint。
- application RED：先加入读取、同源、最差可计算 fixture 与 AC17 判据，执行 `cargo test --offline --locked -p ficant-application --test r5c_data_health_contracts` exit 1；首个真实错误是 DataHealth use case 尚不存在。RED 未作为 checkpoint。
- transport / gate RED：新 `GetDataHealthReport` success arm 已进入 descriptor、但尚未修改 R5b inventory 时执行 `pwsh -NoProfile -NonInteractive -File scripts/check-coverage.ps1`，真实 exit 1，精确报告该 success arm 不在闭集。随后只按 §5 授权新增一个 Composition arm并删除零使用理由；没有删除旧 arm、改写既有分类或放宽未知默认失败。该 RED 未作为 checkpoint。

**forward-only checkpoints：**

- contract checkpoint：profile exact ref / hash、整数阈值、完整 MarketTime、`threshold + 1ns`、向上取整展示秒数、稳定 issue、一次 evaluation、verified-empty capability、coverage / fingerprint / report hash / lineage 全部由 domain 直接测试覆盖；R5c `5 / 5` 与 R5b coverage 回归 `2 / 2` 转绿后成立。
- application checkpoint：exact PositionSnapshot、可选 verified DataSnapshot、exact DataSource 与平台 active profile 解析转绿；应用层 `12 / 12` 还证明缺少平台 profile 失败关闭、最差可计算 Bond+Futures fixture 的 KRD 完整 success、健康/最差 pair 的 position / totals 数值与算法口径不变，以及 AC17 CapitalUse 继续失败关闭；既有 position snapshot `2 / 2` 回归通过。API 层 `10 / 10` 中的独立 wire 判据再以同一组已编码计算请求 bytes 在 WARNING 查询前后调用真实 adapter，比较完整 `CalculateKeyRateDv01Response.encode_to_vec()`，重复健康请求还比较完整 `GetDataHealthReportResponse.encode_to_vec()`，并证明 curve / bond / RulePack parser / delivery 四类 engine call count 均未因健康查询变化；两层同时转绿后 checkpoint 才成立。
- persistence checkpoint：平台 profile 经专用 verified blob/snapshot role 与 PostgreSQL `0022` 往返，exact 与 active lookup 一致；同一 VersionRef 不同内容、重叠 active profile 均失败关闭。针对性 profile persistence `1 / 1`、完整 migration acceptance `4 / 4` 转绿后成立。
- transport / production checkpoint：生成输出、API、gRPC-Web 与真实 server builder 完成后，API health `10 / 10`、server SIT `1 / 1`、descriptor `20 / 20`、68 / 4 / 64 闭集、七个负向 fixture与 51 条分层 fixture全部转绿后成立。

**最终针对性结果（同一工作树最终代码候选）：**

| 命令 | 结果 |
|---|---|
| `cargo test --offline --locked -p ficant-domain --test r5c_data_health_contracts` | exit 0；`5 / 5` |
| `cargo test --offline --locked -p ficant-domain --test r5b_coverage_contracts` | exit 0；`2 / 2` |
| `cargo test --offline --locked -p ficant-application --test r5c_data_health_contracts` | exit 0；`12 / 12`；含平台 active profile 唯一解析与缺项失败关闭 |
| `cargo test --offline --locked -p ficant-application --test position_snapshot_contracts` | exit 0；`2 / 2` |
| `cargo test --offline --locked -p ficant-storage --test position_snapshot_postgres -- --test-threads=1` | exit 0；`2 / 2`；含 verified-empty publish / exact read / resolve 往返 |
| `cargo test --offline --locked -p ficant-storage --test data_health_profile_postgres -- --test-threads=1` | exit 0；`1 / 1`；exact/active 往返、同一 VersionRef 不同内容拒绝、重叠 active profile 失败关闭 |
| `cargo test --offline --locked -p ficant-storage --test migration_acceptance -- --test-threads=1` | exit 0；`4 / 4`；精确验证 PostgreSQL migration `0001–0022`，`0022` 只登记一次并保留原子回滚、legacy 与 FK 判据 |
| `cargo test --offline --locked -p ficant-api --test data_health_service` | exit 0；`10 / 10`；wire 子用例 `1 / 1` 把同一个 `CalculateKeyRateDv01Request.encode_to_vec()` 解码后用于前后两次真实 adapter 调用，并精确比较完整 `CalculateKeyRateDv01Response.encode_to_vec()`；同一个 `GetDataHealthReportRequest.encode_to_vec()` 的两次响应精确比较完整 `GetDataHealthReportResponse.encode_to_vec()`；curve / bond / RulePack parser / delivery 四类 engine call count 在两次健康查询后均保持不变 |
| `cargo test --offline --locked -p ficant-api --test position_snapshot_service` | exit 0；`2 / 2` |
| `cargo test --offline --locked -p ficant-server --test data_health_sit` | exit 0；`1 / 1` |
| 固定 Buf 1.56.0：`buf format --diff --exit-code interface`；`buf lint interface` | 两条均 exit 0 |
| 固定 Buf 1.56.0：`cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory` | exit 0；`20 / 20` |
| `uv run --offline --locked --project python python -m pytest python/tests/test_contract_import.py -q` | exit 0；`1 / 1` |
| `pwsh -NoProfile -NonInteractive -File scripts/check-coverage.ps1` | exit 0；目标 descriptor 判据 `1 / 1`（其余 19 filtered）；68 个 success arm 全部显式分类，4 个 composition carrier、64 个使用 3 个实际理由的非组合 arm |
| `pwsh -NoProfile -NonInteractive -File scripts/test-coverage-check.ps1` | exit 0；删除 portfolio / views / capital / health coverage、新裸 composition、标量裸组合及 inventory 外未知 arm 共七项均真实 exit 1；六个 pre-R5c 负向 fixture全部保持，已分类 base inventory exit 0 |
| `pwsh -NoProfile -NonInteractive -File scripts/check-layering.ps1` | exit 0；AC03、AC01、C++/FFI、funding、tax 与 allowlist 计数均为 0 |
| `pwsh -NoProfile -NonInteractive -File scripts/test-layering-check.ps1` | exit 0；`51` assertions |

**强措辞定向复核：** 提交前按 `逐位 / bytes / exact / 精确 / 完全相等` 对 R5a、R5b、R5c 已点亮判据做代码—断言对象核对。R5a 的来源扩展仍由 exact `FixedDecimal`、`UnitRef`、逐仓位 KRD 与 totals 断言承载，无容差；R5b 的 gross / source pair、KRD 与 totals 仍使用 exact equality，descriptor 的 wire 冻结由精确 FQN、field number 与 wire type 断言承载，没有把 domain equality 冒充 wire bytes。直接回归结果为 R5a domain `3 / 3`、R5b domain `2 / 2`、R4d-a application `4 / 4`、R4d-b application `6 / 6`、position application `2 / 2`、portfolio API `3 / 3` 与修正后的 DataHealth API `10 / 10`，全部 exit 0。扫描只发现 R5c 闸门 9 的强标签弱断言，以及同形的 R5c I3 / I8 健康响应 domain-only equality；二者均已在上述 API wire 子用例中收紧到完整 protobuf 编码 bytes，未发现需要把扫描扩大到 R4 的第二个缺口。

**生成确定性：** 按 `interface/README.md` 使用固定 Buf 1.56.0 在两个独立临时输出树执行完整生成；两棵树文件集合均为 `74`，规范化 SHA-256 比较 `74 / 74` 相同、mismatch `0`，两棵树与仓库生成输出比较同为 mismatch `0`。§6 点名的五个候选生成文件（Rust flat package两份、Python两份、TypeScript一份）均与两次生成结果逐文件相同。`interface/buf.gen.yaml` base-to-candidate diff 只有把 `ficant.research.v1.DataHealthService` 加入既有 Python gRPC `types` 闭集；插件、version、revision、out、option 与其他 service 条目均未改。

**完整本地检查：**

- 使用固定 Node 22.17.0、pnpm 10.12.4、Buf 1.56.0、uv / Python 3.12，在最终代码候选执行 `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`，exit 0。
- 使用相同固定工具链执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1`，exit 0。分层 fixture `51` assertions、coverage 七个负向 fixture与 68 / 4 / 64 闭集、严格 Clippy、Rust 全量测试（含 DataHealth API wire `10 / 10` 与平台 profile persistence）、descriptor `20 / 20`、C++ `8 / 8`、主 matrix `36 / 36`、Phase 2B `16 / 16`、Phase 2C `18 / 18` 与 Oracle `3 / 3`、Phase 2D `18 / 18` 与 Oracle `3 / 3`、Python、Phase 2E live、Phase 3A、Web typecheck / build 与 Web `35 / 35` 全部通过。
- 在同一最终代码候选上静默导入六个 Windows User 级 `FICANT_TEST_*` 变量后执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration`，exit 0。migration `4 / 4`、lease queue `1 / 1`、execution closure `3 / 3`、worker `1 / 1`、Phase 1 `1 / 1`、negative invariants `13 / 13`、Phase 2B / 2C / 2D 各 `1 / 1`、Phase 3A registry / dual-source 各 `1 / 1`、Phase 3B codec `3 / 3` 与 publication `1 / 1` 全部通过；变量值未输出。
- 平台 profile 扩展后的首次完整检查先后因 strict Clippy 的相同 match arm、缺失 error docs 与 snapshot insert `too_many_lines` 停止；通过拆分纯函数、收紧签名与补齐文档 forward-only 修复，未新增 lint allow、未改语义、expected、Oracle、Golden、matrix、canonical、allowlist、门禁断言或容差。Web 首次执行因专用 worktree 无 `node_modules`、找不到锁定工具而停止；执行 `corepack pnpm@10.12.4 install --offline --frozen-lockfile` exit 0，只从离线 store 恢复依赖、无 tracked write。首次离线编译只作为测试前置，未记为通过证据。所有受影响套件及三条统一入口随后均在最终代码候选重跑通过。
- 提交前逐项核对闸门 8 / 9 / 10 时发现闸门 9 的 application 断言消息写“bytes”，实际只比较完整 domain exposure；候选在 commit 前停止，未把该结果作为 wire 证据。正确承载层 `crates/ficant-api/tests/data_health_service.rs` 已在冻结的 30 条写路径内，无需 §5 扩权；forward-only 新增真实 protobuf adapter 判据，没有修改 brief 判据、断言方向、容差或 expected。该 wire 子用例首次执行因 Subject fixture 误绑 profile id 为 `0 / 1`，修正为 snapshot 的 exact Subject 后再次因复用的 R4d-b 测试 mock 按跨调用累计序号选 CTD 而为 `0 / 1`；只在 API 测试中换成按 exact bond id 选择 IRR 的无状态 engine，随后 `1 / 1`。修正后的首次完整入口在 strict Clippy 因该测试函数 `117 / 100` 行而停止；只抽取断言辅助函数、未加 lint allow，目标 Clippy 与 API `10 / 10` 后转绿。最终 `check.ps1` 及 `-IncludeIntegration` 均从头重跑。
- 平台 profile 的首次 targeted PostgreSQL 测试在测试逻辑前以 `PoolTimedOut` 报告数据库不可达；只读核验确认 Docker Desktop Linux engine 未运行。启动既有本地 engine 后 PostgreSQL 与 Ceph RGW 恢复 healthy；随后该测试因新 fixture 的 ULID 含禁用字符 `O` 失败，只修正新测试 id 映射，重取 profile `1 / 1` 与 migration `4 / 4`。最终 `-IncludeIntegration` 从头重跑并 exit 0。外部前置失败与错误 fixture 均未记为候选通过，变量值全程未输出。

**范围、受保护事实与停机条件复核：** execution base 始终为 `2f84601f0b0558d60b79f630e8cdf03e3ff92311`；首个 forward-only checkpoint 为 `132d875c0d84e6f4788c98a9a6af92557f99bc8f`，最终候选在其上继续收紧平台 profile 授权与完整时间语义，不回退已验证结果。§6 冻结清单保持 `30` 项原文不变，§5 精确扩权 `19` 项；两者并集 `49` 项，base-to-candidate 实际 tracked + untracked changed-path 集合为 `45` 项，全部在授权并集内，unauthorized `0`，`git diff --check` exit 0。§6 受保护事实除获批 migration tree 外的 `23 / 23` 个 blob / tree OID 与 base 一致；`0001–0021` 逐文件 blob 不变，migration 只新增 `0022_data_health_threshold_profiles.sql`。`scripts/layering-allowlist.json` 内容仍精确为 `[]`。Acceptance sentence、平台阈值授权、完整时间判定、AC17 与 AC35 回归均由上述机械判据满足；公共候选未改 authority。R5c 因此已形成完整本地自测候选，但在公共候选独立审查、rebase merge、authority 精确绑定，以及 Human 单独签署 wire 可见限定并逐条批准前，不宣称 AC36 已正式点亮。

## 7. 残余风险

- 平台 profile 已具备管理员发布、不可变持久化与 owner/time 唯一 active 解析，但本轮没有删除、停用、审批流或 UI；运营侧变更只能 forward-only 发布新 VersionRef 与有效区间。数据库会把重叠 active 配置暴露为失败而不会自动选择，部署治理必须避免该冲突窗口。
- 本轮只检查一个可选 DataSnapshot；当前 canonical v1 / DataSource 同质约束使来源比例通常为 0% 或 100%。多源、记录级类型与分歧检测需要 canonical v2 或多快照合同，不能从单源结果推断。
- `price_evidence_evaluated=false` 是“未检查”，不是“无风险”。任何 UI 或报告未来呈现健康状态时必须同时呈现检查范围；R5c 不点亮该呈现层行为。
- 显式空 PositionSnapshot 提供可审计的空事实；`VERIFIED_EMPTY` 只解决默认 coverage 与合法 0 / 0 的 wire 歧义，不能证明组织真实没有持仓。Coverage 仍只描述已导入数据，I10 的“不猜测”边界保持。
- 健康服务覆盖 AC36 的首批可执行三类核心场景与 R5a 来源扩展，不代表 ADR-0017 检查表全部实现。完整性类与一致类已明确顺延到 v0.2 DataHealth 扩展，进入 v0.2 前必须在路线表落位；后续新增 issue 必须是加法契约并为“未检查 / 健康”差异提供判据。
- Human 即使按 wire 可见口径点亮 AC36，ADR-0017 §三的呈现层义务仍未完全履行；后续 UI 迭代必须主动展示结论与未检查范围，不能要求用户知道并手工调用 RPC 才看到预警。
- R5b 零使用 `SINGLE_POSITION` 在 R5c 删除后，未来若出现真实单仓位 success arm，可以加性恢复；必须与实际 arm 同提交且经闭集 fixture 审查，不能再次预留。
- R5c 两个新 RPC 会按设计触发 R5b coverage 闭集门禁。这是预期 RED，不得被当成意外失败后静默加入 inventory；§5 的精确 Human 授权与 68 / 4 / 64、七个负向 fixture是唯一允许的前进路径。
- authority 的“新门禁收紧四条判别测试”与 SPEC 税制 / 发行身份条款仍是独立治理债务，不由 R5c 公共候选代偿，也不进入 AC36 点亮证据。
