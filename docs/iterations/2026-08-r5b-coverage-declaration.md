# R5b 迭代 brief — 组合覆盖度声明

**迭代：** R5b · **承接条目：** AC35 · **execution base：** `1b5e2661de4616e8ccc80822acc8e116be3433ea` · **authority base：** `473c40da0a1259db3fd7660b3be80007982e6fff`

本 brief 是 R5b 面向 Human 的唯一设计与最终证据载体。R5 已拆为 `R5a（AC15）→ R5b（AC35）` 与 `R5a（AC15）→ R5c（AC36）`；R5a 已在公共提交 `1b5e2661de4616e8ccc80822acc8e116be3433ea` 和 authority 提交 `473c40da0a1259db3fd7660b3be80007982e6fff` 闭环。本轮只让所有现存多仓位聚合结果携带同一强类型 `CoverageDeclaration`，并以 descriptor inventory 和真实负向 fixture 禁止裸组合输出。本文冻结验收、非目标、公共契约、测试与逐文件写路径，并记录同一候选上的真实实现与自测证据；不创建状态页、子任务 brief、治理 checklist 或进度副本。

## 1. 目标

把 SPEC I10 与 ADR-0017 的“只按已导入数据评估”落成可机械判定的服务端合同。凡一个成功结果对多个 `PositionSnapshot.positions` 做投影或聚合，无论实际请求中恰有一个还是多个仓位，都必须携带非空覆盖声明。本轮精确承载点只有：

- `ficant.research.v1.PortfolioKeyRateExposure`：对可进入持仓敞口的仓位生成逐仓位 KRD 并汇总 `totals`；
- `ficant.research.v1.PositionViews`：在同一结果中投影多个逐仓位视图；
- `ficant.research.v1.CapitalUse`：对多个逐仓位 `capital_requirement` 求和。

逐仓位 `PositionKeyRateExposure`、`PositionView` 与原始事实 `PositionSnapshot` 不携带声明；它们不是组合级结论。`GetPositionViewsResponse` 即使同时承载逐仓位明细，其成功 payload `PositionViews` 仍是多仓位投影，因此必须携带声明。声明只描述已导入 PositionSnapshot 的可见边界，不猜测组织内未导入的仓位或外部已占用。

**Acceptance sentence：**

> 对同一 verified PositionSnapshot，`CalculateKeyRateDv01`、`GetPositionViews` 与成功的 `CalculateCapitalUse` 均返回非空 `CoverageDeclaration`：声明给出已导入分母、实际参与数、按 exact UnitRef 稳定分组的已导入与参与仓位毛经济价值、缺失关键字段记录数、实际消费的 R5a 价格来源分布及不同外部 DataSource exact version 数。组合 KRD 的 coverage 来源分布与顶层 `source_confidence` 逐项相同且来自同一次解析；bond-only 为内部曲线来源且外部源数为 0，Bond + exact Futures 为 `ACTIVE_QUOTE + CURVE_INTERPOLATION` 且外部源数为 1。全部 66 个现存 RPC success arm 均在闭集 inventory 中显式分类为组合级或非组合级；63 个非组合 arm 必须分别选择 `SINGLE_POSITION`、`NO_NUMERIC_AGGREGATE`、`REGISTRY_METADATA`、`ACK_OR_ECHO` 四个封闭理由之一，不存在 `OTHER` 或 `UNSPECIFIED`。删除任一现存组合 carrier 的 coverage，或新增任一未分类 success arm，机械门禁均 exit 1。显式分类的非组合输出不被误报。任一会让已声明范围内结果错误的关键字段缺失仍在聚合前失败关闭，AC17 的 UNKNOWN 会计分类仍不返回 `CapitalUse` 或覆盖声明。Golden、Oracle、Phase 2C/2D matrix、canonical quote v1/schema/hash、R5a `PriceSourceType`、C++/C ABI、定价公式、迁移集合、allowlist 与 UI 均不变。

## 2. 验收

| 条目 | R5b 可执行判据 |
|---|---|
| AC35 · 三个承载点 | descriptor 精确证明 `PortfolioKeyRateExposure.coverage = 10`、`PositionViews.coverage = 5`、`CapitalUse.coverage = 5` 均为 `ficant.research.v1.CoverageDeclaration`；成功 transport 映射三处均非空。 |
| AC35 · 分母与总额 | 由同一 verified PositionSnapshot 派生 `imported_position_count` 与 `participating_position_count`。两组 gross economic value 对每只仓位的 `economic_value` 取绝对值后按 exact UnitRef 分组精确相加，按 `(unit_id, version)` 排序；不得跨 UnitRef 相加、做 FX、用净额抵消或猜测未导入分母。 |
| AC35 · 关键字段 | “关键”只由当前 carrier 实际消费的合同推导：缺失会令该仓位无法正确进入本次聚合的字段即关键。现有成功路径不建立 partial-result 语义；关键字段缺失仍失败关闭，所以成功声明的 `missing_critical_field_record_count` 为 0。合法业务排除（例如 reverse-repo collateral 不进入 position exposure）不是缺失。 |
| AC35 · 可信度同源 | `PortfolioKeyRateExposure.coverage.source_confidence` 与既有字段 9 `source_confidence` 逐项相同，计数与 `mixed` 只解析一次；构造不一致 pair 必须失败。`distinct_external_data_source_version_count` 对实际消费的 external DataSource exact VersionRef 去重，内部 `CURVE_INTERPOLATION` 不计入。 |
| AC35 · 裸值全称 | coverage 门禁闭集枚举全部 66 个现存 RPC success arm，并逐项显式分类为 3 个组合级或 63 个非组合级；未知 arm 默认失败。每个非组合 arm 必须选择四成员封闭 `NonCompositionReason`，无自由文本、`OTHER` 或 `UNSPECIFIED` 逃逸口。删除任一 coverage、加入标量裸组合或加入任意 inventory 外 success arm 均 exit 1；当前已分类 inventory exit 0。 |
| AC17 · 回归 | 同一含 UNKNOWN 会计分类的 snapshot：`CalculateCapitalUse` 仍返回既有 typed failure，不能返回部分金额、零金额或带 coverage 的成功值；`GetPositionViews` 的非资本视图行为保持。 |

R5b 闸门：

1. RED-first 分三次取得：domain / protobuf coverage 不变量；application 三条派生路径及 AC15 同源；transport / descriptor / coverage gate。每次先只加判据并取得真实非零 exit code，记录首个真实错误；RED 不是 checkpoint。domain、application、transport 与 gate 只有对应直接测试转绿后才能成为 forward-only checkpoint。
2. “组合级”是消息语义而非本次仓位数量：上列三个 carrier 即使只返回一个 position 也必须带 coverage。RPC envelope 不复制声明，声明只挂在成功 payload；逐仓位明细不挂声明。
3. `imported_position_count` 是 verified PositionSnapshot 的完整仓位数；`participating_position_count` 是本 carrier 实际进入投影或数值聚合的仓位数，必须大于 0 且不大于分母。`PositionViews` 为全部仓位，`CapitalUse` 成功时为全部仓位，Portfolio KRD 为 `includes_position_exposure()` 的仓位。
4. “总额”冻结为 gross economic value，不是 quantity、KRD、capital requirement 或组织总资产。对每个 exact UnitRef 分别将 `abs(Position.economic_value)` 精确求和；同一 UnitRef 只出现一次，列表稳定排序，参与值不得大于对应已导入值。任何溢出、非规范 Decimal 或 UnitRef 不一致均失败关闭。
5. 关键字段清单不得另立 JSON、常量表或 allowlist。当前 protobuf / domain 构造已使三条成功路径所需字段必填；R5b 不以 `missing_critical_field_record_count > 0` 放行原本会失败的请求，也不把持仓形态导致的合法排除记为缺失。
6. R5a 的 `PriceSourceSummary` 是组合 KRD 唯一可信度事实源。domain 仍保留既有顶层 marker 以维持 AC15 wire 合同，同时 coverage 接受同一解析结果；构造器必须拒绝两者不一致。`PositionViews` 与 `CapitalUse` 不消费价格记录，其 coverage 中 `source_confidence` 必须缺席且 external source count 为 0，不得把导入金额猜成 `MODEL_VALUATION`。
7. external source count 按实际消费的 `VersionRef(id, version)` 去重，不按来源类型数、quote 行数、DataSnapshot 数或 lineage 总数计数。bond-only 的内部曲线插值为 0；当前一个 verified futures snapshot 绑定一个 external DataSource exact version，因此混合 KRD 为 1。
8. coverage、两组金额、来源分布与 external source count 全部进入组合结果 content hash。`PortfolioKeyRateExposure` 继续使用结果 hash；`PositionViews` 与 `CapitalUse` 的既有字段 2 从“直接复用 snapshot hash”收紧为各自结果 hash，包含 snapshot hash、输出内容与 coverage。lineage 仍来自同一已验证输入，不为派生 coverage 伪造新外部引用。
9. `PriceSourceCount` 与 `PriceSourceSummary` 的 FQN、字段号、枚举类型和 wire bytes 全部冻结。为解除 `exposure.proto ↔ coverage.proto` 的循环依赖，它们只做同 package 源文件迁移至 `coverage.proto`；不得复制第二套 message、改 FQN 或改 tag。descriptor 必须证明 R5a 字段 9 的类型仍为 `.ficant.research.v1.PriceSourceSummary`。
10. coverage gate 必须从 descriptor 而不是文件名、字段形状启发式或散文推断合同；全部现存 RPC success arm 组成闭集 inventory，每一项显式分类为组合级或非组合级，未知项默认失败。非组合分类必须选择封闭枚举 `SINGLE_POSITION`、`NO_NUMERIC_AGGREGATE`、`REGISTRY_METADATA` 或 `ACK_OR_ECHO`，不得提供自由文本、`OTHER`、`UNSPECIFIED` 或默认理由。三个组合 carrier 集合精确冻结。配套 fixture 覆盖三个“删除 coverage → exit 1”、原有“新增裸组合成功 payload → exit 1”、标量裸组合回归、inventory 外全新 success arm，以及已分类 base inventory 正向通过。不得增加跳过项、例外 allowlist 或靠改 expected 删除 carrier。
11. AC17 的 UNKNOWN regression 必须在 coverage 形成前返回既有错误；不得把失败改写成 `missing_critical_field_record_count = 1` 的部分成功。CapitalUse 的 Decimal 与 UnitRef 结果、PositionViews 三类视图、Portfolio KRD / totals 与 R5a marker 均须在响应外逐位复核，证明 coverage 不进入数值公式。
12. descriptor、`check-coverage.ps1`、`test-coverage-check.ps1`、`check-fast.ps1` 与 `check.ps1` 都是自管门禁，base-to-candidate diff 必须单独呈现且只能扩大覆盖。不得修改 expected、Oracle、Golden、matrix、canonical hash、allowlist、既有分层断言或容差制造通过。

## 3. 非目标

- R5c `DataHealthReport`、`health.proto`、健康阈值、预警等级、自动降级、阻断、质量评分或来源选择；R5c 必须在 R5b 合并后重新冻结双 base。
- ADR-0017 的 Web/UI/报告呈现规则；本轮不修改 `web-dm/platform-shell/**` 或 `web-dm/webapps/**`。仅由固定 Buf 机械更新 generated contracts 不等于实现 UI。
- 猜测未导入仓位数、组织总资产、外部已占用、额度余量或 Coverage 百分比；声明只给已导入分母与参与分子，不生成看似精确的组织覆盖率。
- partial aggregation、best-effort 跳过坏仓位、用非零 missing count 替代既有失败关闭，或削弱 AC17、I4、I7 的拒绝语义。
- 为 PositionSnapshot 的导入金额补造价格来源类型，或把没有 DataSource exact ref 的字段推断为真实成交、活跃报价、模型估值或曲线插值。
- canonical v2、记录级来源类型、DataSource registry / migration 变更、PositionSnapshot persistence 变更或新 migration；coverage 是响应期派生结果。
- 修改 KRD、CTD、midpoint、资本占用、回购视图、会计分类、Factor、RulePack、Tax、Funding、C++ / C ABI 或任何既有定价公式。
- Constraint、ShadowPrice、Policy、AC09 税制、AC36 健康度、AC37 角色白名单、authority 三件套、SPEC、ACCEPTANCE、MANUAL、任何 ADR、版本 tag、CI/CD 或发布。

## 4. 公共契约变化

- 新增 `interface/proto/ficant/research/v1/coverage.proto`。同 package 的既有 `PriceSourceCount` 与 `PriceSourceSummary` 从 `exposure.proto` 移入该文件，但 FQN 与字段保持：`PriceSourceCount { source_type = 1; record_count = 2; }`，`PriceSourceSummary { counts = 1; mixed = 2; }`。这是 wire identity 保持的源文件迁移，不得产生平行类型。
- 新增 `ficant.research.v1.CoverageDeclaration`：

| tag | 字段 | 类型与冻结语义 |
|---:|---|---|
| 1 | `imported_position_count` | `uint64`；verified PositionSnapshot 完整分母，必须大于 0。 |
| 2 | `participating_position_count` | `uint64`；实际进入本 carrier 的仓位数，范围 `1..=imported_position_count`。 |
| 3 | `imported_gross_economic_value_by_unit` | `repeated ficant.core.v1.DecimalValue`；所有已导入仓位 `abs(economic_value)` 按 exact UnitRef 分组。 |
| 4 | `participating_gross_economic_value_by_unit` | 同型；只含参与仓位，每个 UnitRef 的值不得超过字段 3。 |
| 5 | `missing_critical_field_record_count` | `uint64`；按被消费合同推导；现有三个成功 carrier 固定为 0。 |
| 6 | `source_confidence` | `PriceSourceSummary`；只在 carrier 实际消费 typed price evidence 时存在。Portfolio 必须与顶层字段 9 完全一致；PositionViews / CapitalUse 缺席。 |
| 7 | `distinct_external_data_source_version_count` | `uint64`；实际消费的 external DataSource exact VersionRef 去重数，内部曲线不计。 |

- `PortfolioKeyRateExposure` 保留字段 1–9，新增 `coverage = 10`；既有 `source_confidence = 9` 不删除、不改型、不改语义。两处 summary 由同一 domain 值生成，不允许独立计算。
- `PositionViews` 保留字段 1–4，新增 `coverage = 5`；逐仓位 `PositionView` 保持字段 1–7。`CapitalUse` 保留字段 1–4，新增 `coverage = 5`。三个 RPC request、response oneof、service 方法与错误 arm 均不变。
- domain 新增不可变 Coverage 值对象，校验计数、稳定 UnitRef 分组、gross 精确加法、missing 范围、source presence 与 external count；组合 result 构造器必须把 coverage 纳入 content hash。应用层只从同一 verified snapshot 和既有 R5a materialization 派生，transport 只映射，不重新核算。
- registered-futures materialization 只新增内部 accessor 以暴露它已经验证并写入 lineage 的 DataSource exact VersionRef，供 portfolio 层去重计数；不改变 decoder、RulePack parser、delivery / curve / bond engine 输入或调用顺序。
- Python 与 TypeScript 固定生成树新增 `coverage_pb2.py` / `coverage_pb.ts`，Python gRPC 插件还原样生成无 service 的 `coverage_pb2_grpc.py`，并机械更新 exposure / position 模块的 import 与字段。Rust flat package 继续只生成 `ficant.research.v1.rs`。`interface/buf.gen.yaml`、插件版本、revision 与参数不变。
- `CoverageDeclaration` 位于 `ficant.research.v1`，未来 R5c 的 `health.proto` 若需要可直接 import；R5b 不创建或预占 `health.proto`。

## 5. 需 Human 决策

- **已裁决——组合级精确范围：** Human 批准“凡对多个仓位做聚合的响应都是组合级”。R5b 精确承载 `PortfolioKeyRateExposure`、`PositionViews`、`CapitalUse`；逐仓位明细不携带。`GetPositionViews` 即使返回明细，只要结果投影多个仓位，外层 `PositionViews` 就必须携带。
- **已裁决——UI / 报告退出本轮：** Human 批准本轮只建立 server / wire 合同，不承接 ADR-0017 §二的界面与报告呈现规则；`web-dm/**` 只允许固定生成树的机械变化，应用 UI 树冻结。
- **已裁决——关键字段由消费合同推导：** Human 批准“缺失会使该仓位无法进入本次聚合的字段即关键”，不维护独立清单。由持仓形态或 carrier 语义造成的合法排除不是缺失；会让声明范围内结果错误的缺失继续失败关闭。
- **已裁决——package 归属：** Human 批准 `CoverageDeclaration` 位于新 `interface/proto/ficant/research/v1/coverage.proto`，供未来 R5c `health.proto` 复用；本轮不创建 health contract。
- **已裁决——分布必须带分母：** Human 批准 Coverage 同时携带 imported / participating count 与按 exact UnitRef 分组的 imported / participating gross economic value。不同 UnitRef 不换算；不从已导入分母推断组织分母。
- **已裁决——AC15 单一事实源：** Coverage 吸收 R5a 已产出的 `PriceSourceSummary`，不另算第二份结论；同时携带消费到的不同 external DataSource exact version 数。顶层 AC15 marker 与 coverage 不一致的构造必须失败。
- **执行期事前授权——补齐生成树：** 2026-08-02 Human 在候选首次写入前授权精确新增 `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/coverage_pb2_grpc.py`。边界仅为固定 Buf Python gRPC 插件在两棵确定性证明树中逐位相同的 159 字节无 service stub；不得手写 service、修改 `interface/buf.gen.yaml`、插件版本、revision、参数或其他生成路径。原 §6 冻结清单保持原文，本条是唯一扩权事实。
- **执行期授权——收紧新门禁 inventory：** 2026-08-03 Human 授权修改本轮新建 coverage gate 的 expected / inventory 与负向 fixture。触发事实是旧门禁对对抗性标量裸组合 success arm 错误 exit 0，而非被测业务代码或正向测试失败；变更把 success arm 判据从形状识别改成 66 项显式分类闭集，未知默认失败，接受集合严格收缩。三个删 coverage 与原裸组合四个既有负向 fixture 全部保留并继续 exit 1；同一 forward-only 补提交新增标量回归与 inventory 外未知 arm 两项负向 fixture，其他业务、descriptor 字段断言与期望均不变。这不是 rebaseline。
- **执行期授权——非组合分类理由封闭化：** 2026-08-03 Human 在 PR #56 ready 前单独授权把 63 个沉默的 `NonComposition` 分类升级为四成员封闭 `NonCompositionReason`。触发原因是自管 inventory 对错误语义分类缺少可审阅守卫，不是被测业务代码失败；每个 arm 现在必须在 `SINGLE_POSITION`、`NO_NUMERIC_AGGREGATE`、`REGISTRY_METADATA`、`ACK_OR_ECHO` 中明确择一，不设自由文本、`OTHER` 或 `UNSPECIFIED`。变更使错误分类从沉默布尔值收缩为具名可争论断言；既有六个负向 fixture 必须全部保持 exit 1，其他断言逐项保持。本条是第三次扩权，并采用“推迟会增加回填成本或留下错误状态窗口才留本轮”的止损判据；UI 裸值防护仍不回流本轮。
- **待 authority 固化——新门禁收紧判别测试：** 后续 authority 应与 PR #12 两款并列记录：仅当负向 fixture 证明门禁漏报、变更后接受集合严格收缩、漏报 fixture 与修复同一提交且其余断言保持时，才允许修改本轮新建门禁的 expected / inventory；每次收紧须机械重跑全部既有负向 fixture。本公共 brief 只记录 Human 建议与本轮事实，不代替 authority 裁决。
- **待 authority 单独签字——AC35“输出”口径：** AC35 的“不带声明的裸数值不出现在任何输出中”在本轮只声明服务端成功响应；WebApp / 报告呈现层的裸值防护不在 R5b，须另行冻结与判断。该限定语必须在 AC35 authority 批准时由 Human 单独签字并写入裁决记录，不得与实现候选的验收动作合并批准；在此之前公共 brief 只记录待决口径，不能冒充已批准。
- **authority 边界：** agent 不修改私有 authority。公共候选未来 rebase merge 后，authority 必须以新 public SHA 重新冻结，Human 单独签署上述“输出”限定语后才能逐条批准 AC35，并把 MANUAL 中“尚无 CoverageDeclaration”改为本轮真实边界。若批准，进度由 v0.1 `18 / 30` 变为 `19 / 30`、全表由 `18 / 36` 变为 `19 / 36`；该动作不点亮 AC36。

## 6. 最终真实测试证据

**双 base 冻结：** 2026-08-02 在专用公共 worktree `C:\git\ficant-r5b-coverage-declaration` 执行 `git fetch --prune origin` 后，亲自确认 branch 为 `codex/r5b-coverage-declaration`、工作区干净且 `HEAD == origin/main == 1b5e2661de4616e8ccc80822acc8e116be3433ea`。该公共 base 已包含 R5a 和导航修复；PR #53 独立核验为 merged，merge commit `ff49a2218c14d6b4e867fb158294b13b7b409f1b`。authority worktree `C:\git\ficant-authority-r5a-base` 干净、detached，`HEAD == origin/main == 473c40da0a1259db3fd7660b3be80007982e6fff`；`verify-authority.ps1 -ExpectedAuthorityCommit 473c40da0a1259db3fd7660b3be80007982e6fff` exit 0，三件套哈希成立，manifest 精确绑定公共 `1b5e2661de4616e8ccc80822acc8e116be3433ea`。以上双 base 自此固定不变。根主克隆 `C:\git\ficant` 不再承担 execution base、审计或新 worktree 起点角色，其本地 `main` 是否前移不影响本轮；后续 worktree 一律从 fetch 后的 `origin/main` 精确创建。

**冻结允许写路径（精确文件；本节自此不得就地改写）：**

- `crates/ficant-api/src/portfolio_risk.rs`
- `crates/ficant-api/src/position_snapshot.rs`
- `crates/ficant-api/tests/portfolio_risk_service.rs`
- `crates/ficant-api/tests/position_snapshot_service.rs`
- `crates/ficant-application/src/use_cases/futures_delivery.rs`
- `crates/ficant-application/src/use_cases/portfolio_risk.rs`
- `crates/ficant-application/src/use_cases/position_views.rs`
- `crates/ficant-application/tests/position_snapshot_contracts.rs`
- `crates/ficant-application/tests/r4d_a_bond_krd_contracts.rs`
- `crates/ficant-application/tests/r4d_b_futures_krd_contracts.rs`
- `crates/ficant-contract-tests/tests/descriptor_inventory.rs`
- `crates/ficant-contracts/src/generated/ficant.research.v1.rs`
- `crates/ficant-domain/src/research/coverage.rs`（新建）
- `crates/ficant-domain/src/research/exposure.rs`
- `crates/ficant-domain/src/research/mod.rs`
- `crates/ficant-domain/tests/r4d_a_bond_krd_contracts.rs`
- `crates/ficant-domain/tests/r4d_b_futures_krd_contracts.rs`
- `crates/ficant-domain/tests/r5b_coverage_contracts.rs`（新建）
- `docs/iterations/2026-08-r5b-coverage-declaration.md`（新建）
- `docs/iterations/README.md`
- `interface/README.md`
- `interface/proto/ficant/research/v1/coverage.proto`（新建）
- `interface/proto/ficant/research/v1/exposure.proto`
- `interface/proto/ficant/research/v1/position.proto`
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/coverage_pb2.py`（新建）
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/exposure_pb2.py`
- `python/node-contracts/src/ficant_contracts/generated/ficant/research/v1/position_pb2.py`
- `python/tests/test_contract_import.py`
- `scripts/check-coverage.ps1`（新建）
- `scripts/test-coverage-check.ps1`（新建）
- `scripts/check-fast.ps1`
- `scripts/check.ps1`
- `web-dm/packages/contracts-generated/src/ficant/research/v1/coverage_pb.ts`（新建）
- `web-dm/packages/contracts-generated/src/ficant/research/v1/exposure_pb.ts`
- `web-dm/packages/contracts-generated/src/ficant/research/v1/position_pb.ts`

**禁止写路径：** 所有未逐项列出的路径。特别禁止 authority 三件套与公共根目录同名废副本、所有 ADR、`README.md`、既有 brief、`docs/architecture/layering-refactor.md`、除三个点名文件外的 proto / generated output、`interface/buf.gen.yaml`、DataSource proto / registry / migration、PositionSnapshot persistence、`crates/ficant-data/src/canonical.rs`、`crates/ficant-api/src/rates.rs`、`binaries/**`、storage、C++ / C ABI / native crates、`domain-packs/**`、除四个点名入口外的 `scripts/**`、`tests/golden-cases/**`、`tests/oracle/**`、`tests/phase2c/**`、`tests/phase2d/**`、`Cargo.lock`、`.gitignore`、`.github/**`、`cicd.yml`、`deploy/**`、`web-dm/platform-shell/**` 与 `web-dm/webapps/**`。扩权只能由 Human 在首次写入前批准并新增 §5 记录；本节不得追认。

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
- `interface/proto/ficant/research/v1/factor.proto`：blob `ed998a2a142836e8eb17e8861e0bbf1fc3bad1ac`
- `interface/proto/ficant/market/v1/data_source.proto`：blob `b839db1346970060661d38ac17ff3d2e3b0a0c7f`
- `interface/buf.gen.yaml`：blob `fcb566700cf743af7a22a6662a9bf5aa8486d96d`
- `migrations/postgresql`：tree `b5940862a877e3e238128080c3478c6011b378a0`
- `domain-packs/cgb-futures/cgb-futures-v1.json`：blob `1fe9db105d15f2f3924b8f488108311611ca7f07`
- `domain-packs/cgb-futures/cgb-futures-v1.bin`：blob `469445e4199020dae0a705be42a0569e72a73f05`
- `domain-packs/cgb-futures/cgb-futures-v2.json`：blob `6fbbc8ec9b38b90dcbeeebc1d776838098873268`
- `domain-packs/cgb-futures/cgb-futures-v2.bin`：blob `054ac57bdde54b3349adecf564ee10489b2efb21`
- `Cargo.lock`：blob `ec46fd45d980de5cad3f66c3dcd0e3d5ff6880ea`
- `docs/architecture/adr`：tree `09e21e9d7a3097757f2037ca0c3c9919763af683`
- `docs/architecture/layering-refactor.md`：blob `97a6f475dd7a4b9fcf8cbdd0ca1bcc4311a8bcff`
- `web-dm/platform-shell`：tree `7e77f86e9282489b8fc43a11e5b983978555783c`
- `web-dm/webapps`：tree `d7305d23a247ff010ec9fd34b5fa763dff717fd5`

**RED-first 与 forward-only checkpoint 计划：**

- domain / contract RED：只加入 `r5b_coverage_contracts` 与 descriptor 断言，预期因不存在 Coverage 类型及三个字段而非零；转绿后必须同时证明计数、gross 分组、排序、source mismatch 拒绝与 content hash 变化，才形成 contract checkpoint。
- application RED：在既有 position / R4d-a / R4d-b fixture 上先断言三条 coverage，预期因 use case 未派生而非零；转绿后必须证明三 carrier、合法排除、external exact source 去重及 AC17 UNKNOWN 拒绝，才形成 application checkpoint。
- transport / gate RED：先加入 wire mapping、descriptor inventory 与初始五类 fixture；预期因字段未生成且裸 carrier 可通过而非零。审查期又以标量裸组合 fixture 真实证明旧门禁漏报（exit 0），其修复不是 checkpoint；生成输出、API 映射、66 项闭集 inventory、六个负向 fixture与两个统一入口全部转绿后才形成最终 transport / gate checkpoint。

**最终针对性命令（实现获单独授权后，必须在同一候选逐条执行并填真实结果）：**

- `cargo test --offline --locked -p ficant-domain --test r5b_coverage_contracts`
- `cargo test --offline --locked -p ficant-domain --test r4d_a_bond_krd_contracts`
- `cargo test --offline --locked -p ficant-domain --test r4d_b_futures_krd_contracts`
- `cargo test --offline --locked -p ficant-domain --test r5a_source_confidence_contracts`
- `cargo test --offline --locked -p ficant-application --test position_snapshot_contracts`
- `cargo test --offline --locked -p ficant-application --test r4d_a_bond_krd_contracts`
- `cargo test --offline --locked -p ficant-application --test r4d_b_futures_krd_contracts`
- `cargo test --offline --locked -p ficant-api --test position_snapshot_service`
- `cargo test --offline --locked -p ficant-api --test portfolio_risk_service`
- 注入固定 Buf 1.56.0 后 `buf format --diff --exit-code interface` 与 `buf lint interface`
- 注入固定 Buf 1.56.0 后 `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory`
- `uv run --offline --locked --project python python -m pytest python/tests/test_contract_import.py -q`
- `pwsh -NoProfile -NonInteractive -File scripts/check-coverage.ps1`
- `pwsh -NoProfile -NonInteractive -File scripts/test-coverage-check.ps1`
- `pwsh -NoProfile -NonInteractive -File scripts/check-layering.ps1`
- `pwsh -NoProfile -NonInteractive -File scripts/test-layering-check.ps1`
- `cargo test --offline --locked -p ficant-storage --test migration_acceptance`（集合仍须完整 4/4，且 migration tree OID 不变）

固定 Buf 生成须按 `interface/README.md` 在两个临时输出树执行并比较规范化 SHA-256；两次一致后才机械同步 Rust / Python / TypeScript 点名生成文件。不得直接以工作树输出冒充确定性证据。

**完整本地检查（最终候选必须真实执行）：**

- `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`
- 使用仓库锁定工具链执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1`
- 导入六个 Windows User 级 `FICANT_TEST_*` 变量且不输出值后，执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration`

**RED-first 真实结果：**

- domain / contract RED：`cargo test --offline --locked -p ficant-domain --test r5b_coverage_contracts` exit 1；首个真实错误为 `CoverageDeclaration` 尚不存在，且既有组合构造器缺少新参数。RED 未作为 checkpoint。
- descriptor RED：在实现字段前注入固定 Buf 1.56.0 执行 `cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory`，exit 1，`15 / 18` 通过；三个失败精确指向不存在的 `CoverageDeclaration` 与三个 carrier 字段。RED 未作为 checkpoint。
- application RED：先把 coverage 断言加入既有应用层 fixture，`cargo test --offline --locked -p ficant-application --test r4d_a_bond_krd_contracts` exit 1；首个真实错误为既有 application 构造调用缺少 source / coverage。RED 未作为 checkpoint。
- 审查期门禁漏报 RED：在提交 `9d94ed4baa152d075c87a45afb0490723c66efe4` 的门禁上，向临时 descriptor 注入可达 `BareScalarAggregate { DecimalValue aggregate_risk = 1; }` 且不带 coverage；`check-coverage.ps1` 错误 exit 0（期望 1）。该对抗 fixture 触发 Human 授权收紧 expected / inventory，不是被测代码失败，也未作为 checkpoint。修复后同形 fixture 与完全未知 success arm 均真实 exit 1。

**forward-only checkpoints：**

- contract checkpoint：domain 值对象及 protobuf 字段完成后，`r5b_coverage_contracts` `2 / 2`；计数、exact UnitRef gross 分组、稳定顺序、source mismatch 拒绝及 hash 变化均转绿。
- application checkpoint：三条 use case 从同一 verified snapshot 派生 coverage；bond-only 与 Bond + exact Futures 的 source / external exact VersionRef 计数、合法排除及 AC17 UNKNOWN 失败关闭均在既有 R4d fixture 上转绿。
- transport / gate checkpoint：生成输出、三处 API 映射、descriptor 全量 inventory 与 coverage gate 转绿；66 个 success arm 明确分类为 3 个组合级、63 个带封闭理由的非组合级，未知默认失败。理由分布为 `ACK_OR_ECHO=30`、`REGISTRY_METADATA=21`、`NO_NUMERIC_AGGREGATE=12`、`SINGLE_POSITION=0`；后者保留为封闭词汇但不以单一 instrument 分析冒充单一仓位。六个负向 fixture 全部 exit 1，已分类 base inventory exit 0。

**最终针对性结果（同一工作树当前候选）：**

| 命令 | 结果 |
|---|---|
| `cargo test --offline --locked -p ficant-domain --test r5b_coverage_contracts` | exit 0；`2 / 2` |
| `cargo test --offline --locked -p ficant-domain --test r4d_a_bond_krd_contracts` | exit 0；`5 / 5` |
| `cargo test --offline --locked -p ficant-domain --test r4d_b_futures_krd_contracts` | exit 0；`3 / 3` |
| `cargo test --offline --locked -p ficant-domain --test r5a_source_confidence_contracts` | exit 0；`3 / 3` |
| `cargo test --offline --locked -p ficant-application --test position_snapshot_contracts` | exit 0；`2 / 2` |
| `cargo test --offline --locked -p ficant-application --test r4d_a_bond_krd_contracts` | exit 0；`4 / 4` |
| `cargo test --offline --locked -p ficant-application --test r4d_b_futures_krd_contracts` | exit 0；`6 / 6` |
| `cargo test --offline --locked -p ficant-api --test position_snapshot_service` | exit 0；`2 / 2` |
| `cargo test --offline --locked -p ficant-api --test portfolio_risk_service` | exit 0；`3 / 3` |
| 固定 Buf 1.56.0：`buf format --diff --exit-code interface`；`buf lint interface` | 两条均 exit 0 |
| 固定 Buf 1.56.0：`cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory` | exit 0；`19 / 19` |
| `uv run --offline --locked --project python python -m pytest python/tests/test_contract_import.py -q` | exit 0；`1 / 1` |
| `pwsh -NoProfile -NonInteractive -File scripts/check-coverage.ps1` | exit 0；目标 descriptor 判据 `1 / 1`（其余 18 filtered）；66 个 success arm 闭集分类完整，组合 carrier `3`，63 个非组合 arm 全部选择四成员封闭理由 |
| `pwsh -NoProfile -NonInteractive -File scripts/test-coverage-check.ps1` | exit 0；三个删字段、原新增裸组合、标量裸组合回归及 inventory 外未知 arm 共六项均真实 exit 1；已分类 base inventory exit 0 |
| `pwsh -NoProfile -NonInteractive -File scripts/check-layering.ps1` | exit 0；AC03、AC01、C++/FFI、funding、tax 与 allowlist 计数均为 0 |
| `pwsh -NoProfile -NonInteractive -File scripts/test-layering-check.ps1` | exit 0；`51` assertions |
| `cargo test --offline --locked -p ficant-storage --test migration_acceptance -- --test-threads=1` | exit 0；`4 / 4`；只验证既有 PostgreSQL migration `0001–0017`，migration tree 未改 |

**生成确定性：** 按 `interface/README.md` 使用固定 Buf 1.56.0 在两个独立临时树生成，各得 `71` 个文件；规范化 SHA-256 比较 `71 / 71` 相同、mismatch `0`。执行期扩权后又在全新临时根 `ficant-r5b-grpc-proof-afc177f9ab7c432fa54175f15d314746` 重跑两次固定生成，仍为 `71 / 71`、mismatch `0`。七个 §6 点名输出（Rust flat package，Python coverage / exposure / position，TypeScript coverage / exposure / position）及 §5 精确扩权的 Python gRPC stub 均与首次输出逐文件相同；`coverage_pb2_grpc.py` 为生成器原样 159 字节，SHA-256 `d686e804f171693117b7d030ec4023f205c70c234c8590f6557aa8702f65fe09`。补齐后重新执行 Buf format / lint 均 exit 0、descriptor inventory `19 / 19`、Python 精确 import `1 / 1`。

**完整本地检查：**

- 在 63 个非组合 arm 全部选择四成员封闭理由、`AnalyzeBond` / `AnalyzeCarryRoll` 明确归为 `NO_NUMERIC_AGGREGATE`、六个负向 fixture 全部保持后，使用固定 Node 22.17.0 与 Buf 1.56.0 执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1`，最终 exit 0。严格 Clippy、Rust 全量测试、descriptor `19 / 19`、C++ `8 / 8`、主 matrix `36 / 36`、Phase 2B `16 / 16`、Phase 2C `18 / 18` 与 Oracle `3 / 3`、Phase 2D `18 / 18` 与 Oracle `3 / 3`、Python、Phase 2E live、Phase 3A、Web typecheck / build 与 Web `35` tests 全部通过。
- 在同一最终候选上导入六个 Windows User 级 `FICANT_TEST_*` 变量且未输出值后，使用相同固定工具链执行 `pwsh -NoProfile -NonInteractive -File scripts/check.ps1 -IncludeIntegration`：exit 0。migration `4 / 4`、lease queue `1 / 1`、execution closure `3 / 3`、worker `1 / 1`、Phase 1 `1 / 1`、negative invariants `13 / 13`、Phase 2B / 2C / 2D 各 `1 / 1`、Phase 3A registry / dual-source 各 `1 / 1`、Phase 3B codec `3 / 3` 与 publication `1 / 1` 全部通过。
- 首次完整检查在 strict Clippy 因冗余 `.into_iter()` 与构造器参数过多 exit 1；以删除冗余调用、把 source + coverage 组成同一受校验值并移除可推导参数 forward-only 修复，未增加 lint allow。第二次完整检查在 Web typecheck 因专用 worktree尚无 `web-dm/node_modules`、找不到 `tsc` exit 1；以 Node 22.17.0 执行 `corepack pnpm@10.12.4 install --offline --frozen-lockfile`，exit 0，`178 / 178` 从离线 store 复用、download `0`、lockfile 未变。两次失败均未修改 expected、Oracle、Golden、matrix、canonical、allowlist、门禁断言或容差。
- 本节首次证据转录后、补齐 Python gRPC stub 后、coverage gate 收紧后及分类理由封闭化后的最终语义候选上，均在固定 Buf 1.56.0 与 Node 22.17.0 环境执行 `pwsh -NoProfile -NonInteractive -File scripts/check-fast.ps1`；四次均 exit 0。最终一次 coverage descriptor `1 / 1`、六个负向 fixture与已分类 base 正向 fixture、Rust format / workspace check / 非环境测试、storage、Phase 3A `5 / 5` 与 Phase 3B `3 / 3` 全部通过。
- 分类理由封闭化后的首次完整检查尝试使用 PATH 中 Node v24.18.0，统一入口按固定工具链合同在 preflight exit 1，测试未开始、候选未变；第二次误把 Node 22.17.0 安装父目录而非可执行文件目录加入 PATH，active version 仍为 v24.18.0，同样在 preflight exit 1。改用 `C:\Users\kermi\AppData\Local\ficant\toolchain\node-v22.17.0\node-v22.17.0-win-x64` 后取得上述完整 exit 0；两次前置失败不作为测试证据。
- 分类理由封闭化后的首次 integration 尝试在 migration acceptance 以 `0 / 4`、首个真实错误 `PoolTimedOut` exit 1；只读诊断确认 Docker Desktop Linux engine 未运行且本机 PostgreSQL 测试端口不可达。仅启动既有 Docker Desktop 与既有测试拓扑、未改配置、数据卷、凭据、代码、断言或 expected，待 PostgreSQL、Ceph RGW 与应用容器 healthy 后，在同一候选上重跑并取得上述完整 integration exit 0。

**范围与受保护事实复核：** 2026-08-03 push 前再次 `git fetch --prune origin`，确认公共 `origin/main` 仍精确为冻结 base `1b5e2661de4616e8ccc80822acc8e116be3433ea`；当前候选是该 base 的 forward-only 后继，`git merge-base HEAD origin/main` 仍为该 base。§6 冻结清单保持 `35` 项原文不变，加上 §5 唯一事前扩权后，有效写路径为 `36` 项；实际 tracked + untracked changed-path 集合精确 `36 / 36`，extra `0`、missing `0`，`git diff --check` exit 0。上述 `25 / 25` 个冻结 blob / tree OID 逐项与 base 一致，`git diff --quiet <base> -- <path>` 均 exit 0；`scripts/layering-allowlist.json` 内容仍精确为 `[]`。公共候选未改 authority；AC35 的 acceptance sentence 已由实现与机械判据满足，但在公共候选 rebase merge、authority 精确绑定、Human 单独签署“输出”限定语并逐条批准前，只能称为本地自测候选，不能宣称 AC35 已正式点亮。

## 7. 残余风险

- `PriceSourceCount` / `PriceSourceSummary` 保持 protobuf FQN 与 wire identity，但从 `exposure.proto` 移到 `coverage.proto` 会改变 Python / TypeScript 的生成模块 import 路径。仓库内当前没有直接从旧生成模块导入这两个 message 的 consumer；固定生成、Python import test 与 Web typecheck 仍必须重取证。若发现外部 source-level consumer 依赖旧模块路径，必须返回 Human，不得手写平行 shim。
- PositionSnapshot 的 `economic_value`、PnL 与 capital requirement 当前没有 R5a DataSource exact ref。R5b 不推断它们的来源类型；因此 PositionViews / CapitalUse coverage 的 price-source summary 缺席只表示“本 carrier 没有消费 typed price evidence”，不表示这些导入字段质量良好。R5c DataHealthReport 仍需显式报告未标型与缺失。
- 当前三个成功 carrier 对关键字段全部失败关闭，所以 `missing_critical_field_record_count` 只能为 0。非零值保留给未来经 Human 冻结的 partial-coverage 语义；R5b 不能拿字段存在本身宣称已经支持跳过坏仓位。
- gross economic value 是已导入 PositionSnapshot 内的覆盖分母，不是可审计的组织总资产，也不证明未导入数据为零。多 UnitRef 列表不做 FX，因此消费者不能自行把不同元素相加成单一百分比。
- server / wire 合同能阻止服务端返回裸组合 payload，但本轮不验证 Web/UI 或报告是否丢弃 coverage 后单独渲染数值；该呈现义务仍是显式后续债务，不能用 AC35 追认。
- coverage gate 与 descriptor expected 都是实施者可写的自管门禁。其安全性依赖闭集默认失败、四成员分类理由、独立 diff 审阅和保留负向 fixture；理由枚举只能让错误分类在 diff 中显形，不能机械证明实施者选择了正确理由，仍须由独立审查逐项判断。任何 carrier / success-arm inventory 缩减、新例外、跳过、新增兜底理由、重新引入形状猜测或“测试专用”旁路均须停止返回 Human。§5 记载的收紧规则仍待 authority 正式固化。
- R5b 不改变数值，但把 PositionViews / CapitalUse 的 `content_hash` 收紧为结果 hash；依赖其旧值等于 PositionSnapshot hash 的未登记 consumer 会受影响。descriptor、API 与 MANUAL 必须明确新语义，不能保留旧 hash 又让 coverage 游离于身份之外。
- R5c 不是 R5b 的机械续写：它仍需独立冻结健康服务边界、阈值分层及“预警不改变数值”的证据。R5c 新增任何 RPC success arm 时，R5b 闭集门禁预期先以“未分类 arm”失败；这不是意外回归，R5c brief 必须显式裁定组合性及对应封闭理由后才可更新 inventory。R5b 完成只点亮 AC35，不点亮或预先实现 AC36。
