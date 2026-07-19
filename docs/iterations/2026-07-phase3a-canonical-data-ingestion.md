# Phase 3A：数据源与 Canonical Quote 接入

## 目标

- 在精确基线 `2d65145f1010afeec23b2b08802773529e243978` 上交付第一条外部数据接入纵向切片：同一组中国国债双边净价 Quote 可分别从文件和 PostgreSQL 两个异构来源读取，并形成相同的 Canonical Arrow Schema。
- 新增可版本化、可授权、可持久化的 `DataSource` 注册合同；注册内容只保存非敏感连接绑定名和逻辑 dataset，连接串、文件根目录与凭据由 composition/runtime 注入。
- 在接入边界完成精确 Instrument version 映射、`Asia/Shanghai` 市场时间与交易日历校验、observed/visible 双时间、点时过滤和失败关闭的数据质量规则。
- 使用 forward-only 快速子循环交付：领域与持久合同；文件 adapter；PostgreSQL adapter；Canonical RecordBatch 与质量规则；真实 PostgreSQL 双源一致性验收。

## 验收

- `DataSource` 由 ULID、正整数 version、owner、kind、名称、连接绑定名、逻辑 dataset、canonical schema ID/hash 构成；同一身份只允许追加下一版本，tenant/owner、幂等 fingerprint 或历史版本漂移均失败关闭。
- 首版只开放 `FILE_NDJSON` 与 `POSTGRES` 两种 source kind；路径和数据库 URL 不进入领域对象、日志、Manifest 或错误文本，adapter 只能使用 composition 传入的已解析资源。
- 两个 adapter 读取同一 raw quote 合同：source record ID、source instrument key、observed time、visible time、可空 bid/ask 的规范 `coefficient + scale`；禁止 float、隐式当前时间、自动时区猜测和 adapter 私自补值。
- Instrument 映射按 source identity、外部 key 和半开有效区间解析到精确 `VersionRef`；缺失、重复或区间重叠失败关闭。Calendar 必须覆盖 observed time 对应的本地交易日，闭市日或会话外数据失败关闭。
- 点时查询同时要求 `observed_at <= as_of` 与 `visible_at <= visible_at_cutoff`，并拒绝 `as_of > visible_at_cutoff`；晚到数据在其真实 visible time 之前不可见，adapter 不得先读取后通过不受约束的“最新值”替换。
- 质量规则至少覆盖：非空且唯一 source record ID、可解析规范时间、`observed_at <= visible_at`、Instrument 可映射、交易会话有效、至少一侧报价存在、bid 不高于 ask、Decimal scale 与 exact Unit version 一致。任一行失败时不返回部分 RecordBatch。
- Canonical Quote Schema v1 固定为 16 列：`tenant_id`、`owner_id`、`data_source_id`、`data_source_version`、`source_record_id`、`instrument_id`、`instrument_version`、`observed_at`、`visible_at`、`local_trading_date`、`bid_coefficient`、`bid_scale`、`ask_coefficient`、`ask_scale`、`unit_id`、`unit_version`；列类型、nullable、UTC microsecond、metadata 和排序规则由一个实现定义。
- 文件与真实 PostgreSQL adapter 对同一业务输入生成字段类型、nullable、schema metadata 和 schema SHA-256 完全相同的 RecordBatch；canonical rows 按 `(observed_at, instrument_id, source_record_id)` 稳定排序，并证明业务列一致。
- `./scripts/check-fast.ps1`、`./scripts/check.ps1` 和 Phase 3A 真实 PostgreSQL 集成入口在同一最终候选上 exit 0；测试覆盖授权、版本/幂等、映射/日历/时间/质量负向边界和双源一致性。

## 非目标

- 不在本迭代写 Parquet、不生成或发布 `DataSnapshot`/Manifest、不写 Ceph RGW，也不宣称实验已经脱离外部数据源；这些是 Phase 3B 的唯一交付。
- 不新增 UI、公共 gRPC service、Python 数据接入 API、流式采集、CDC、消息队列、调度器、供应商专用字段或任意 SQL/任意文件解析能力。
- 不把平台内部 PostgreSQL metadata 表冒充外部数据源；集成验收使用独立 source table 和独立 adapter 查询边界。
- 不修改 Phase 1/2 的 Snapshot、Artifact、数值 expected、Oracle、C ABI、断言或容差。

## 公共契约变化

- `ficant-domain` 新增 `DataSource` / `DataSourceKind` 版本化定义；`ficant-application` 新增窄 `DataSourceRepository` 注册与 exact read port；`ficant-storage` 使用新的 append-only PostgreSQL migration 实现该 port。
- 新增 `ficant-data` Rust crate，唯一拥有 raw source row、Instrument 映射、点时选择、数据质量和 Canonical Quote RecordBatch；Domain/Application 不依赖 Arrow、SQLx、文件系统或 adapter 实现。
- Canonical Schema ID 固定为 `ficant.market.quote.canonical.v1`；首版 schema 的 hash 算法对 Arrow 字段名、类型、nullable 与有序 metadata 做确定性编码后使用 SHA-256。
- 本迭代不改变 `interface/` Protobuf；DataSource 管理 API 在出现首个真实 Human/Agent 调用方前不提前开放。

## 需 Human 决策

- 当前无待决项。若实现必须持久化凭据/绝对路径、开放任意 SQL、放宽点时可见性、允许部分质量通过，或改变现有 Snapshot 公共字段，停止并返回 Human 决策。

## 最终真实测试证据

- 待最终候选形成后填写；中间子循环命令由编排运行状态承载，不在仓库新增状态文档。

## 残余风险

- 待最终候选形成后填写。首版只覆盖批量读取的中国国债双边净价 Quote，不承诺实时订阅、供应商全量语义或其他 fact kind。
