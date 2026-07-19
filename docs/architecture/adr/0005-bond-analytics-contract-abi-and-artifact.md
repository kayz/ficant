# ADR-0005：债券分析内部合同、C ABI 与 Artifact 编码

- 状态：Accepted
- 日期：2026-07-13
- 决策者：Human，经 Orchestrator Architecture/Interface lens 形成方案并由 checklist 确认

> 本 ADR 的数值、C ABI、Arrow 与 Artifact 语义继续有效；其中 MinIO 专名和 adapter 路径已由 ADR-0010 的通用 S3 port、Apache `object_store` 与 Ceph RGW 取代。

## 背景

Phase 2A 必须在不修改公共 Protobuf、不开新数据库 migration 的前提下，穿过 Rust/C++ 边界并把 `BondAnalyticsResult` 作为可验证的不可变 Artifact 保存。若 Domain 持有 FFI/Arrow 类型，Application 直接操作指针，或 Storage 决定业务语义，复杂性会跨层扩散。README 同时要求 Artifact 只使用 Arrow、Parquet 或 Protobuf，因此不能使用临时 JSON 或平台私有裸二进制。

## 决策

### 版本身份

本轮冻结以下稳定身份；字符串和值必须进入结果、Artifact 与证据：

| 边界 | 身份 |
|---|---|
| Domain input schema | `ficant.bond-analytics.input.v1` |
| Domain result schema | `ficant.bond-analytics.result.v1` |
| Artifact payload schema | `ficant.bond-analytics.arrow.v1` |
| Artifact codec | `ficant-bond-analytics-arrow/1` |
| Engine | `ficant-fixed-income-native/0.1.0` |
| Algorithm | `ficant.cgb.fixed-rate.reference/1` |
| Convention | `cgb-reference-v1` |
| Calendar | `cgb-reference-calendar-v1`，精确 RulePack version/hash |
| C ABI | `FICANT_FIXED_INCOME_ABI_V1 = 1` |

Bond、MarketRulePack 与 DataSnapshot 使用已有 `VersionRef` 和 content hash，不建立第二套身份类型。Golden fixture 在 Test Worker 生成 expected 前冻结精确对象 ID、version 和 hash。

### Domain 与 Application 合同

- Domain 定义 provider-neutral `BondAnalyticsInput`、`BondAnalyticsResult`、`DerivedCashflow`、`AnalyticsMode`、单位和值域不变量和稳定 `AnalyticsError`；不依赖 C/C++、Arrow、QuantLib、数据库或对象存储。
- `BondAnalyticsInput` 只接受精确 Bond/RulePack/Snapshot 绑定、估值时点、结算日、`CalendarRequirement` 和 `YIELD_IN` 或 `PRICE_IN` 的精确十进制值。规则参数必须来自 Application 已验证的 RulePack proof，不能让调用方同时提交一份可能漂移的重复规则。
- `CalendarRequirement` 冻结为 `REFERENCE_REPLAY=1` 和 `EXACT_MARKET=2`。本轮 Q-001..Q-012 使用 `REFERENCE_REPLAY`：超出精确 coverage 的未来现金流可以使用 provisional weekend-only 规则，但结果必须标记 resolution/coverage。`EXACT_MARKET` 要求结算日及全部现金流调整所需日期位于精确 coverage 内，超出即返回 `CalendarCoverageMissing`。该字段和实际 resolution 都进入 fingerprint、结果和 Artifact lineage。
- `BondAnalyticsResult` 包含调整前/后的现金流、coupon/principal、应计、净价、全价、YTM、两类久期、凸性、DV01，以及所有 schema/engine/algorithm/convention/calendar/ABI 身份和输入血缘。
- Application 定义 `BondAnalyticsEngine` required port 和 `BondAnalyticsArtifactCodec` required port。前者只计算，后者只把已验证结果编码为确定性 payload；两者都不执行持久化。
- Application 在任何可变 I/O 前解析 exact Bond、RulePack、calendar 和 DataSnapshot proof，并形成不可伪造的内部证明。计算后先检查结果不变量，再编码、stage、验证和发布 Artifact。

### C ABI v1

- 扩展既有 `cpp/fixed-income-kernel/include/ficant_kernel.h`，它仍是唯一 ABI 源；保留 `ficant_kernel_abi_version()`，新增 `ficant_kernel_calculate_bond_v1(...)`。Rust sys 声明必须通过 C layout/constant harness 逐字段校验，不能让手写副本在无门禁时漂移。
- 输入、标量结果和现金流使用带 `struct_size`、`abi_version` 的 C POD；只使用 `uint32_t`、`uint64_t`、`int32_t`、`double` 与指针，不使用 `bool`、`size_t`、C++ enum/string/container。
- `ficant_kernel_cashflow_v1.sequence` 从 `1` 开始，按 eligible payment cashflow 的支付顺序连续递增；C header 是该 ABI 语义的唯一源，Oracle、Rust adapter 与 Artifact 必须消费相同编号，不能各自选择 zero-based 表达。
- 日期用相对 `1970-01-01` 的有符号 `int32_t` 天数；频率、day-count、business-day convention、计算模式、`CalendarRequirement` 和 calendar resolution 用显式固定整数常量。
- RulePack 日历作为已排序且不重复的非工作日例外和工作周末例外数组传入；C++ 不读取系统日历、时钟、环境变量、文件或网络。
- 调用方拥有输入、结果和现金流 buffer。第一次可用 `cashflows=null, capacity=0` 查询 `required_count`；第二次由调用方提供足够 buffer。C++ 不跨 ABI 分配或释放内存。
- 状态码冻结为 `OK=0`、`INVALID_ARGUMENT=1`、`ABI_MISMATCH=2`、`BUFFER_TOO_SMALL=3`、`NO_BRACKET=4`、`NOT_CONVERGED=5`、`NON_FINITE=6`、`CALENDAR_COVERAGE_MISSING=7`、`INTERNAL_ERROR=255`。native adapter 将其翻译为稳定 `AnalyticsError`；供应商文本不进入 Domain。
- 所有入口 `noexcept`，捕获全部异常；禁止跨 ABI unwind。实现不得持有可变全局状态，必须可重入并支持并发只读调用。
- sys crate 是唯一 unsafe 所有者，逐个 unsafe block 声明长度、对齐、生命周期和别名条件；safe native adapter 在调用前后复核版本、buffer、枚举、长度和值域。

### Artifact payload

- 使用已有 `ArtifactKind::Generic` 和既有 Artifact/MinIO 发布路径，不新增公共枚举、RPC、Protobuf 或数据库 migration。
- payload 为单文件、单 record batch、单 row 的 Arrow IPC File，media type 为 `application/vnd.apache.arrow.file; profile=ficant.bond-analytics.v1`。
- Arrow Rust crate 固定为 `arrow=59.1.0`、仅启用 `ipc` 所需 feature；IPC metadata version 固定为 `V5`。编码器禁止 dictionary、压缩和可变生成时间；固定 Ubuntu 24.04 x86_64、little-endian、单 file、单 record batch、单 row。schema metadata 与全部 field metadata 均为空，避免 map 顺序进入字节结果。
- 顶层字段按下表序号编码，全部 `nullable=false`，不得增加、删除、重排或改变类型而不提升 schema/codec version：

| # | 字段 | Arrow type |
|---:|---|---|
| 1–7 | `schema_id`, `codec_id`, `engine_id`, `engine_version`, `algorithm_id`, `convention_profile`, `calendar_id` | `Utf8` |
| 8 | `algorithm_version` | `UInt32` |
| 9 | `abi_version` | `UInt32` |
| 10 | `calendar_version` | `UInt64` |
| 11 | `calendar_content_hash` | `FixedSizeBinary(32)` |
| 12–13 | `calendar_requirement`, `calendar_resolution` | `UInt8` |
| 14–15 | `calendar_coverage_start`, `calendar_coverage_end` | `Date32` |
| 16 | `market_timezone` | `Utf8`，值固定为 `Asia/Shanghai` |
| 17 | `valuation_at` | `Timestamp(Microsecond, "UTC")` |
| 18 | `settlement_date` | `Date32` |
| 19 | `input_mode` | `UInt8` |
| 20 | `input_value` | `Decimal128(38,12)` |
| 21 | `bond_id` | `Utf8` |
| 22 | `bond_version` | `UInt64` |
| 23 | `bond_content_hash` | `FixedSizeBinary(32)` |
| 24 | `rule_pack_id` | `Utf8` |
| 25 | `rule_pack_version` | `UInt64` |
| 26 | `rule_pack_content_hash` | `FixedSizeBinary(32)` |
| 27 | `snapshot_id` | `Utf8` |
| 28 | `snapshot_version` | `UInt64` |
| 29 | `snapshot_content_hash` | `FixedSizeBinary(32)` |
| 30 | `face_amount` | `Decimal128(38,12)` |
| 31 | `cashflows` | non-null `List` of non-null `Struct` |
| 32–39 | `accrued_interest`, `clean_price`, `dirty_price`, `yield_to_maturity`, `macaulay_duration`, `modified_duration`, `convexity`, `dv01` | `Decimal128(38,12)` |

- `cashflows` 的 non-null Struct 子字段顺序固定为：`sequence: UInt32`、`nominal_date: Date32`、`payment_date: Date32`、`coupon: Decimal128(38,12)`、`principal: Decimal128(38,12)`、`total: Decimal128(38,12)`；所有子字段 `nullable=false`，list offset 固定为 32-bit Arrow `List`。
- `schema_id`、`codec_id` 等身份既是 payload 字段也是证据断言；不放入可变 metadata。Arrow 59.1.0 升级、IPC V5 改变、writer 参数改变或任何 schema 差异都必须提升 codec/schema version，并使旧 Golden hash 保持可重放。
- safe adapter 先把有限浮点结果规范化为 12 位 round-half-even Decimal，Artifact codec 不再接触浮点。内容哈希是精确 Arrow 文件字节的 SHA-256。
- Arrow 类型只存在于 Storage adapter 的 `BondAnalyticsArtifactCodec` 实现与测试工具；不得进入 Domain、Application port、C ABI 或公共接口。该用法仅编码一个 Phase 2A Artifact，不建设 Phase 3 Snapshot/Arrow 数据平台。

### 发布与重放

1. Application 完成 scope、owner、Bond/RulePack/Snapshot exact proof 和 coverage 检查。
2. native engine 纯计算并返回 Domain result；Application 验证 `dirty=clean+accrued`、现金流、单位、版本和有限数不变量。
3. Artifact codec 生成确定性 Arrow bytes、size 和 SHA-256。
4. 使用现有 MinIO staged blob/verified proof 流程上传并回读验证；随后使用现有 idempotency/owner/tenant 约束发布 `Generic` Artifact 元数据。
5. 失败或并发冲突按现有 staged-object cleanup/compensation 规则处理；不得留下可见的半发布 Artifact。
6. 重放必须重新读取并校验 payload hash/size/owner/tenant/schema/lineage，再用精确旧输入和版本重算；相同规范结果和 payload hash 才算通过。

### 实现执行器边界

- 执行拓扑、Worker Profile、权限、task-local capability、统一 runner 和升级规则由 ADR-0007 与 ADR-0008 统一定义，本 ADR 不再维护平台或 CLI 特例。执行器选择不得改变这里冻结的模块所有权、业务语义和验收责任。
- 模型与权限正交；Test Author/Fast Development 可以写入，但只有合同白名单路径可写。Quality 和最终 Audit 是独立只读参与者，不作为 Worker；Quality 负责测试报告，Audit 仅检查最终一致性。
- 本 ADR 涉及的 C++ 估值算法、YTM、久期、凸性、DV01、C ABI、unsafe、内存/异常边界和跨模块 FFI 属于 Strong Profile，默认通过 Windows runner 使用强 Codex `gpt-5.6-sol` + isolated `workspace-write` 执行，不能拆成表面小任务规避风险升级。
- Spark 只能消费已冻结语义执行机械任务；Claude Medium 只处理边界冻结、根因明确的常规实现。任一 Worker 请求修改本 ADR 的边界时必须停止并返回 Orchestrator 的 Architecture lens；涉及业务语义或风险接受时由 Human 决策。

## 模块声明

| 模块 | 负责 | 不负责 | 允许依赖 | 禁止依赖 |
|---|---|---|---|---|
| `ficant-domain` analytics | 业务输入、结果、不变量、错误 | FFI、编码、I/O | primitives | sys/native/Arrow/QuantLib/storage |
| `ficant-application` analytics | proofs、ports、用例、发布顺序 | 数值、物理编码、I/O 实现 | Domain、已有 ports | sys/native/Arrow/QuantLib/SQL/MinIO |
| `ficant-fixed-income-native` | 单位/错误/浮点规范化、safe provider | 业务身份、存储 | Domain/Application port、sys | Arrow/QuantLib/SQL/MinIO |
| `ficant-kernel-sys` | 唯一 unsafe 和 header bindings | 校验策略、业务错误 | C ABI | Application/Storage/QuantLib |
| C++ kernel | 现金流、现值、求根、风险算法 | 身份、血缘、I/O、Artifact | C++20 STL 内部实现 | Rust/Arrow/QuantLib/网络/文件 |
| Storage Arrow codec | 纯确定性物理编码/解码，实现窄 `BondAnalyticsArtifactCodec` port | 计算、规则决定、blob I/O | Application codec port、Domain result、Arrow | blob ports、sys/native/QuantLib |

## 依据

稳定 Domain port 隔离 provider，唯一 sys crate 隔离 unsafe，Storage codec 隔离 Arrow。使用 Generic Artifact 避免公共接口和 migration 扩张；使用受控 Arrow IPC 遵守既有 Artifact 格式约束，又没有把 Phase 3 数据平台提前带入本迭代。

## 被否决方案

1. **新增公共 `BondAnalyticsResult` Protobuf。** 违反本轮公共 descriptor 零变化并扩大 Web/Python 消费面。
2. **用 JSON 或 C struct 原始字节保存。** 违反已定 Artifact 格式，且跨平台规范化和演进不可控。
3. **让 Domain/Application 依赖 Arrow。** 将物理格式扩散到稳定业务层。
4. **由 C++ 直接生成 Artifact。** 把身份、血缘和存储责任带入数值内核。
5. **新增 analytics Artifact 数据库枚举/migration。** Generic Artifact 与 media type 已足够表达本轮内部结果。
