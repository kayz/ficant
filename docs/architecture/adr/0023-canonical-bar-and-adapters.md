# ADR-0023：标准 Bar 与外部数据适配器

- 状态：Accepted
- 日期：2026-10-23
- 决策者：Human（2026-10-23 在对话中确认第 2、3、4 条与结算方式作为键这四处选择）
- 范围：SPEC v1.4 §4.2「数据转换」；AC45；为 AC38–AC41 的点时回放提供输入
- 关联：ADR-0012、ADR-0014（Policy / PolicyArtifact）、ADR-0017（数据健康与覆盖度）、ADR-0022（平台与 WebApp）

## 背景

二期的研究闭环从外部 bar 数据开始：

> 外部 bar 数据 → 适配器转换为标准 Bar → 点时快照 → 逐 bar 点时回放 → …

SPEC 规定：ficant 不自建数据；标准结构由 ficant 定义，适配器只负责映射；日频与分钟使用同一结构，以频率字段区分。AC45 的判定是：

- 两个字段格式不同的外部 bar 数据源，经各自适配器导入后得到同一标准 Bar 结构；
- 日频与分钟只以频率字段区分；
- 缺少必需字段、时间不在交易时段或 `observed_at > visible_at` 的行被拒绝，并报告行标识与原因。

现有的一期数据链（`crates/ficant-data`）只处理报价：

- `RawQuoteSource` 适配器读出 `RawQuoteRow`（bid/ask）；
- `CanonicalQuoteIngestor` 校验后编码成 `ficant.market.quote.canonical.v1` 的 Arrow 批；
- 打包成不可变 Parquet `DataSnapshot` 与 manifest；
- 导入经 `DataSourceAuthorization` 授权，并写入血缘。

可以复用的部分：`DataSource` 注册（已带 `canonical_schema_id` 与 `price_source_type`）、`InstrumentMapping`（含生效期）、`PointInTimeWindow`、`Calendar` 交易时段校验、Unit 精度、授权导入与快照发布。

不能直接复用的部分：

- 报价 ingestor 遇到第一行错误就让整批失败，并且大多只报 `QualityRuleFailed`，不带行标识。AC45 要求逐行报告；
- 报价没有频率、区间和成交量。

为了检验设计，我用私有研究库里的 CFETS XBOND 真实成交样本（250016，2026-01，7,542 笔）核对了字段，没有把它复制进本仓库。样本暴露出四个设计点：

- 有 T+0 与 T+1 两种结算方式，价格不同；
- 时间戳不带时区；
- 本地时间 18:03 仍有成交，晚于常见的 17:00；
- 每笔成交带"记录创建时间"，可以作为可见时间。

## 决策

### 1. 标准 Bar（L1 领域对象）

一条 Bar 描述**一个工具、一种结算方式、一个频率下的一个时间区间**内的成交汇总。

| 字段 | 必需 | 说明 |
|---|---|---|
| `instrument` | 是 | 经 `InstrumentMapping` 解析的工具版本引用 |
| `frequency` | 是 | 固定枚举：`M1`、`M5`、`M15`、`M30`、`H1`、`D1` |
| `settlement` | 是 | `T0`、`T1`，或 `UNSPECIFIED`（来源不区分时） |
| `bar_start`、`bar_end` | 是 | 左闭右开区间；分钟级按市场时区的整分钟对齐，`D1` 为当日交易时段 |
| `observed_at` | 是 | 等于 `bar_end`：这条 Bar 描述的是截至区间结束的市场 |
| `visible_at` | 是 | ficant 能知道这条 Bar 的时间，规则见第 3 条 |
| `local_trading_date` | 是 | 交易日 |
| `open`、`high`、`low`、`close` | 成对可空 | 净价，每百元面值；区间内无成交时四个价格同时为空 |
| `volume` | 是 | 成交面额；无成交为 0 |
| `turnover`、`trade_count`、`vwap` | 否 | 来源提供时保留 |
| `close_yield` | 否 | 来源提供时保留，用于核对，不参与价格校验 |
| `source_record_id` | 是 | 来源行标识，用于拒绝报告和血缘 |

价格可信度（I9）取自 `DataSource.price_source_type`，按来源声明，不逐行声明。成交汇总的 Bar 是 `RealTrade`；来源若是估值序列（如日终估值），它不是 Bar，不走这条链。

### 2. 适配器与标准结构的分工

```
外部来源（CSV / NDJSON / 数据库 …）
  → BarSourceAdapter：读原始行，按该来源的字段映射产出 RawBarRow（字段名、时间格式、时区、小数都已解析）
  → CanonicalBarIngestor：统一校验、解析工具、对齐区间，产出标准 Bar 与拒绝报告
  → 快照打包：Parquet DataSnapshot（schema `ficant.market.bar.canonical.v1`）+ manifest
```

- 每种来源格式一个适配器（代码）。字段映射、时区、可见时间规则写在该来源的适配器注册配置里（数据层配置，不进 `DataSource` 领域对象，不改公共契约）。
- 第一个适配器是带字段映射的 NDJSON 文件适配器（`FileNdjsonBarSource`），支持点分路径、本地/UTC/毫秒时间戳、字符串或数字小数，以及来源代码到频率/结算方式的翻译。CSV 等其他文件格式需要在 `DataSourceKind` 里新增来源类型，属于公共契约变化（`interface/proto/**`），另行提出。
- 标准化和校验只在 ingestor 里做一次，适配器之间不重复，也不能放宽。
- AC45 的测试用两个字段格式不同的合成来源：
  - 来源 A：扁平字段、本地时间文本、字符串小数、结算方式写作 `T+1`、带发布时间字段；
  - 来源 B：嵌套字段、UTC 毫秒时间戳、数字小数、结算方式写作 `2`、没有可见时间字段，声明延迟 2 秒。

  同一组合成 bar 经两者导入后，除来源标识列外逐列相同；日线与分钟线的 schema 相同，只有频率列不同。
- 逐笔成交聚合成 Bar（XBOND 这种来源）是另一种适配器，不在 AC45 范围内，后续单独提出。

### 3. `visible_at` 从哪里来

I1 要求每条事实都有 `visible_at`。来源情况分两种：

- 来源有可见时间字段（例如 XBOND 的记录创建时间、供应商文件的发布时间），映射该字段。聚合时取区间内各记录可见时间的最大值，且不得早于 `bar_end`。
- 来源没有这个字段（例如供应商次日上午才下发的日线文件），在来源注册里声明一个固定延迟：`visible_at = bar_end + delay`。延迟写进 manifest，回放时可以审计。

二者都没有的来源不能注册为 Bar 来源。

### 4. 行级拒绝与报告

逐行校验，坏行被拒绝，其余行继续导入。

- 拒绝原因是固定枚举：`MISSING_REQUIRED_FIELD`、`INVALID_VALUE`（字段存在但无法解析）、`OUTSIDE_TRADING_SESSION`、`OBSERVED_AFTER_VISIBLE`、`UNMAPPED_INSTRUMENT`、`MISALIGNED_INTERVAL`、`INCONSISTENT_OHLC`（如 high < max(open, close)、仅部分价格为空）、`NEGATIVE_VOLUME`、`DUPLICATE_BAR`、`PRECISION_EXCEEDED`。
- 拒绝报告列出每一行的 `source_record_id`（没有可用标识时为 `row:<序号>`）、原因与相关字段，写入导入结果；写入快照 manifest 与计入数据健康（ADR-0017）随 Bar 快照打包一起实现。
- 晚于本次导入的 `as_of` 或 `visible_at` 截止的行不是坏行，只是"此刻还不知道"：它们不进快照，单独计数（`outside_window_rows`），不算拒绝。
- 以下情况整批失败（失败关闭）：请求配置不合法（schema、单位、时区、来源未声明价格可信度）；来源不可读；或窗口内没有任何一行被接受。整批失败时仍返回全部拒绝原因。

### 5. 快照与回放的接口

- 标准 Bar 快照沿用 `DataSnapshot`（不可变、内容寻址、血缘），只有 schema 与 manifest 内容不同。manifest（`ficant.data.bar-snapshot-manifest.v1`）绑定来源、映射、日历、价格/金额/收益率单位、工具集合、可见时间规则与完整拒绝报告；读取时三方互验（snapshot、Parquet、manifest），并把每一行重新构造成领域 `Bar`，任何漂移都整体失败。
- 拒绝报告进 manifest 后，"读到的行数 = 接受 + 窗口外 + 拒绝"必须成立，否则视为篡改。
- 回放（AC38）按决策时点读取：`VerifiedBarSnapshot::visible_at(t)` 只返回 `visible_at ≤ t` 的 Bar。这一条由快照读取层保证，不由策略代码保证。决策时点晚于快照的可见截止时，读取失败而不是返回不完整的数据。
- `observed_at = bar_end` 保证用不到"未结束的 bar"。

### 6. 本 ADR 不做的事

- 不改 `interface/proto/**`。通过 gRPC 导入和查询 Bar 需要新增公共契约，另行提出并经 Human 同意。
- 不新增 migration。快照元数据沿用现有表；如需按 schema 检索，另行提出。
- 不处理盘口（SPEC 已排除）、不做逐笔聚合（见第 2 条）。

## 代价

- Bar 与报价各有一套 ingestor，有部分重复的校验代码。等 Bar 稳定后，再考虑把两者的行级报告统一。
- 行级拒绝意味着一次导入可能只进了部分数据。缺失由拒绝报告、覆盖度声明与数据健康预警呈现（I10），不阻断计算。

## 替代方案

- **任一行出错即整批失败**（与现有报价一致）：拒绝。真实来源几乎总有少量脏行，整批失败会让导入无法进行，也不符合 AC45"被拒绝并报告"的表述。
- **把聚合、时区、可见时间推断放进各适配器**：拒绝。校验规则会分散在各适配器里，难以保证 AC45 要求的"同一结构"。
- **分钟与日线用两种结构**：拒绝。SPEC 明确要求同一结构、以频率字段区分。
