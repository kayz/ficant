# Phase 3B：不可变 Parquet Snapshot

## 目标

- 在精确基线 `c2e2f1f2ebfd5524e5e4da2843adcf6211588097` 上，把 Phase 3A 的 Canonical Quote RecordBatch 转换为确定性 Parquet 与 canonical Snapshot Manifest，使用既有 Phase 1 双 blob proof 发布为真实 `DataSnapshot`。
- 复用 PostgreSQL `research.data_snapshots`、`SnapshotRepository`、Ceph RGW content-addressed staging/promote 和 required verified-read；不建立第二套快照 metadata、对象存储 key 或完整性语义。
- 形成唯一的快照消费边界：实验侧只接收经过 metadata、Parquet 和 Manifest 三者一致性校验后解码的 Canonical RecordBatch，不持有或调用 `RawQuoteSource`。
- 使用 forward-only 快速子循环交付：冻结 Manifest/Parquet 合同；确定性构建与离线重读；通用双 blob 发布用例；真实 PostgreSQL + Ceph RGW 发布、重启重读和外源隔离验收。

## 验收

- Parquet 只编码 `ficant.market.quote.canonical.v1` 的固定 16 列；Apache Arrow/Parquet Rust 版本固定为 `59.1.0`，单 batch/单 row group、`UNCOMPRESSED`、禁用 dictionary、Parquet writer version 2.0 与 data page v2。相同输入两次生成的 bytes、SHA-256 和大小完全一致，重读后的 schema、行顺序、nullable 和值完全一致。
- `ficant.data.snapshot-manifest.v1` 使用 UTF-8 canonical JSON：键序稳定、无空白、结尾换行；禁止运行时当前时间和绝对路径。Manifest 精确绑定 snapshot/tenant/owner、Canonical schema ID/hash、Parquet hash/size/row count、as-of/visible cutoff、DataSource exact version、Instrument mapping digest、Calendar/Unit exact version、实际 Instrument exact versions、质量规则与冻结 Parquet writer 参数。
- `DataSnapshot` 的 `as_of`/`visible_at` 来自 Phase 3A 点时窗口，`schema_hash`、`manifest_hash`、`blob_content_hash` 分别来自 canonical schema、Manifest bytes 和 Parquet bytes；lineage 至少包含 DataSource、Calendar、Unit 和实际 Instrument exact versions，任何 owner、version、hash、size、row count、时间或 lineage 漂移失败关闭。
- 新增 application 双 blob publication 用例，在任何 I/O 前验证 scope/owner、非空 payload、Parquet/Manifest hash；随后分别 stage、append、verify/promote，构造既有 `VerifiedSnapshotProof::data` 并通过 `SnapshotRepository` 发布 metadata。幂等重试返回同一快照；发布失败保留既有 orphan/reconciliation 语义，不伪装跨 PostgreSQL/Ceph 分布式事务。
- 正式重读必须先通过既有 `VerifiedReadFacade::read_verified_snapshot` 同时读取两个 required payload，再由 `ficant-data` 校验 Manifest canonical bytes、Snapshot/Manifest/Parquet 三方绑定并解码；缺失或篡改任一 blob、错误 schema/row count/lineage 均不返回部分 batch。
- 真实集成验收先通过一个计数 `RawQuoteSource` 完成一次 Phase 3A ingest，发布到隔离 PostgreSQL 16 + Ceph RGW；随后销毁外源 adapter、重建 storage adapters，只按 `DataSnapshot` ID 重读并得到同一 Canonical RecordBatch，外源调用计数保持一次。该证据证明快照绑定后的实验输入路径不再访问外部数据源。
- `./scripts/check-fast.ps1`、`./scripts/check.ps1` 和 Phase 3B 真实 PostgreSQL + Ceph RGW 集成入口在同一最终候选上 exit 0；供应链/许可证清单包含 `ficant-data` 与新增锁定依赖，仓库策略和文档与 Phase 3 正式退出状态一致。

## 非目标

- 不新增公共 Protobuf、migration、UI、外部 DataSource 管理 API、流式/CDC、分区数据集、多文件 Manifest、压缩算法选择、schema evolution 或任意 Parquet 参数。
- 不修改 Phase 1 `DataSnapshot` 字段、双 blob roles、required-read 错误语义、对象存储 content key 或 PostgreSQL/Ceph 一致性模型。
- 不把 Canonical RecordBatch、临时 staging object、`probe_verified` 或直接 Ceph 读取冒充正式快照消费；不让实验持有数据库 URL、文件路径或 `RawQuoteSource`。
- 不修改 Phase 1/2/3A 的 expected、Oracle、数值断言或容差。

## 公共契约变化

- `interface/` 与 PostgreSQL schema 不变。`ficant-data` 新增内部 `SnapshotManifest`、确定性 Parquet codec、snapshot package builder 和 verified snapshot decoder；它们不是跨进程 API。
- `ficant-application` 新增通用的 DataSnapshot 双 payload publication use case，只依赖既有 `BlobStore` 与 `SnapshotRepository` ports，不依赖 Arrow、Parquet、SQLx 或 Ceph 实现。
- 根依赖新增精确 `parquet = 59.1.0` 与已锁定 `serde = 1.0.228`；因该版本经生产可达路径引入无维护且无修复版本的 `paste 1.0.15`，Parquet 改为 `crates/vendor/parquet-59.1.0` 的本地精确来源。vendored 树以官方 crates.io 制品 `sha256:5302d4...1157dd` 为基线，只应用 Apache Arrow 已合并提交 `bc4e672` 的移除 `paste` 补丁，并由补丁 blob 与发布树摘要双重失败关闭；普通开发门禁保持 `--offline --locked`，不加入 Git 仓库或联网安装行为。

## 需 Human 决策

- 当前无待决项。若实现需要改变 `DataSnapshot`、Manifest/blob roles、公开 Protobuf、migration、对象存储 key，允许非确定性 writer 参数，或让快照消费方回退到外部 source，停止并返回 Human 决策。

## 最终真实测试证据

- `cargo clippy --offline --locked -p ficant-application -p ficant-data -p ficant-storage --all-targets -- -D warnings`：exit 0；Phase 3B 触达的三个 crate 全 target 严格 Clippy 通过，无新增 lint 豁免。
- `./scripts/check-phase3b.ps1`：exit 0；确定性编码/精确重读与 Parquet、Manifest、lineage 篡改失败关闭 2/2，真实 PostgreSQL 16 + Ceph RGW 双 payload 发布、adapter 重建、脱离外源重读 1/1。
- `./scripts/check-fast.ps1`：exit 0；workspace check 与非环境回归通过，Phase 3A Canonical ingestion 5/5、Phase 3B codec 2/2。
- `./scripts/check.ps1`：exit 0；Rust fmt/strict Clippy/workspace build、generated contract 12/12、C++ CTest 8/8、Phase 2C/2D 独立 Oracle 各 3/3、Phase 2E live SDK 1/1、Phase 3A 5/5、Phase 3B 2/2、Web 29/29，其余非环境回归全部通过。
- `./scripts/check.ps1 -IncludeIntegration`：exit 0；在同一 disposable PostgreSQL 16 + Ceph RGW 上，migration 4/4、Phase 1 业务闭环 1/1、负向不变量 13/13、Phase 2B/2C/2D 发布重放各 1/1、Phase 3A registry 与双源各 1/1、Phase 3B codec 2/2 与脱离外源发布重读 1/1。统一入口曾发现旧 Phase 1 acceptance 清理未删除 Phase 3A 新增 `data` schema；最终候选已显式清理并通过上述完整串行回归。
- `bash .github/scripts/tests/run-repo-policy-tests.sh`：exit 0；中文、路径、CI 合同和恢复 fixture 全部通过，CI 已显式运行 Phase 3B codec 与真实存储验收。
- `cargo test --offline --locked -p ficant-data --test snapshot_codec`：exit 0，2/2；Apache 上游补丁后的 Parquet codec 保持字节确定性、精确重读与篡改失败关闭。
- `bash .github/scripts/tests/run-gates-tests.sh`：exit 0；新增 vendored 第三方来源/最终树绑定与源码漂移负向 fixture，既有门禁 fixture 全部通过。
- 冻结 Syft 1.46.0 扫描发布树得到 640 个 Cargo/PyPI/npm 包，其中一方包 16 个；`paste` 为 0，`parquet 59.1.0` 恰好一项且标记为 Apache-2.0 第三方发布树来源。`verify-license-inventory.py verify --require-first-party` exit 0，inventory digest 为 `dff34ad7877eeb715a7599f03db9fae856946851cd94a5c683d99293eb4dde5b`。

## 残余风险

- 首版只交付单个 Canonical Quote batch 的单文件快照，不承诺大规模分区、压缩比、谓词下推或 schema evolution；真实验收只覆盖小型固定数据集，不代表生产吞吐与容量结论。
- Parquet writer 参数和库版本属于字节确定性合同；未来升级 Arrow/Parquet、开启压缩或分区必须作为显式兼容迭代，不能静默改写历史快照。
- 当前 vendoring 是针对无修复 `paste` advisory 的临时供应链措施；退出条件是 crates.io 上首个包含 Apache 提交 `bc4e672607f00587349b1308f6cf717fc6518848` 的 Parquet 正式版本。升级时必须重新跑字节兼容验收并删除 vendored 树，不能长期形成私有 fork。
- Phase 4 ResearchGraph 尚未实现；本迭代冻结并验证的是它唯一允许使用的 verified Canonical Snapshot 解码边界，不宣称已有完整实验运行时或业务 UI。
- 本地许可证/包集合已按锁文件独立验证；锁定 OSV release snapshot 的正式结论仍以更新候选的 GitHub required `supply-chain` job 为准，本地结果不冒充远端 CI。
