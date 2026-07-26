# 交付发布说明

## Phase 3B 不可变 Parquet Snapshot 候选（2026-07-20）

- 新增确定性 Canonical Quote Parquet codec 与 `ficant.data.snapshot-manifest.v1` canonical JSON；固定 Arrow/Parquet `59.1.0`、单 row group、无压缩、无 dictionary、writer/data page v2，并把 schema、hash/size/rows、点时窗口、DataSource/映射/Calendar/Unit/Instrument 血缘、质量与 writer 参数完整绑定。
- 新增 application 双 payload 发布用例，复用 Phase 1 `BlobStore`、`VerifiedSnapshotProof::data`、`SnapshotRepository` 和 required read，不增加第二套 metadata、object key 或完整性语义。缺失、篡改、非 canonical Manifest、Parquet 元数据或血缘漂移全部失败关闭。
- 真实 PostgreSQL 16 + Ceph RGW 验收证明外源只调用一次；销毁 `RawQuoteSource`、重建 storage adapters 后，只按 `DataSnapshot` ID 仍得到相同 Canonical RecordBatch。Phase 3 因而正式退出；最终命令、测试数量和残余风险以 [`docs/history/iterations/2026-07-phase3b-immutable-parquet-snapshot.md`](../history/iterations/2026-07-phase3b-immutable-parquet-snapshot.md) 为准。
- 供应链一方包集合新增 `ficant-data`，同步修正 Phase 2E 的 `ficant-sdk` 包身份，并由冻结 Syft 1.46.0 重建 640 包许可证清单。`parquet 59.1.0` 的可达 `paste 1.0.15` 无维护 advisory 通过官方 crates.io 源码加 Apache 已合并提交 `bc4e672` 的最小 vendoring 消除；上游制品、补丁 blob、最终发布树和退出条件均被门禁锁定，不新增风险忽略或 Git 运行时依赖。

## Phase 3A 双源 Canonical Quote 接入候选（2026-07-19）

- 新增版本化 DataSource 注册、文件 NDJSON 与 PostgreSQL 两种 adapter，并将同一中国国债双边净价输入转换为固定 16 列 Canonical Arrow Schema；路径、数据库 URL 与凭据不进入领域对象或错误文本。
- 接入边界对 Instrument/Calendar/Unit 精确版本、observed/visible 双时间点时选择、交易会话、Decimal 和双边报价质量失败关闭；任一坏行使整批失败。真实 PostgreSQL 双源验收证明 schema hash、metadata、稳定排序和业务列一致。
- 本候选不写 Parquet、不发布 Snapshot/Manifest，也不宣称实验已脱离外部数据源；Phase 3B 将单独交付该退出条件。最终命令、测试数量和残余风险以 [`docs/history/iterations/2026-07-phase3a-canonical-data-ingestion.md`](../history/iterations/2026-07-phase3a-canonical-data-ingestion.md) 为准。

## Phase 2E Python SDK 一致性候选（2026-07-19）

- 新增 `ficant.rates.v1.RatesAnalyticsService` 与可安装 `ficant-sdk`，通过五个认证后一元 RPC 调用 Phase 2A–2D 的真实 Rust/C++ 生产路径；Python 不重写数值算法，不直连 PostgreSQL、Ceph RGW 或 C ABI。
- 真实服务进程上的跨语言 Golden Case 覆盖现券、曲线/Carry-Roll-down、交割篮子/CTD 与套保，结果与冻结参考一致，Phase 2 正式退出。完整证据以 [`docs/history/iterations/2026-07-phase2e-python-sdk.md`](../history/iterations/2026-07-phase2e-python-sdk.md) 为准。

## Phase 2D 国债期货 DV01 套保比例候选（2026-07-19）

- 新增基于带符号目标 DV01、CTD 每百元 DV01 与转换因子的单合约套保参考实现，输出连续合约数、推荐整数手数、剩余 DV01 与套保有效性；固定中金所 `TS`、`TF`、`T`、`TL` 100 万元合约面值和稳定整数平局规则。
- C++20 内核通过加法式 C ABI 与安全 Rust adapter 提供结果，并与独立 50 位 Decimal Oracle 的四品种冻结案例一致；非有限值、ABI 漂移、reserved 漂移和整数边界失败关闭。
- 确定性 Arrow Artifact 绑定目标风险、Phase 2C 交割结果、CTD 分析、合约、债券、RulePack 与 DataSnapshot 七段血缘，并通过真实 PostgreSQL 16 + Ceph RGW 发布、adapter 重建后重放、篡改检测和临时状态清零。Phase 2 的参考算法优先清单已完成，但 Python SDK 一致性退出条件仍未交付；完整验收与残余风险以 [`docs/history/iterations/2026-07-phase2d-futures-hedge.md`](../history/iterations/2026-07-phase2d-futures-hedge.md) 为准。

## Phase 2C 国债期货交割价值链候选（2026-07-19）

- 新增中金所 `TS`、`TF`、`T`、`TL` 合约参数和可交割券资格，并交付 CF、交割发票价、基差、含融资成本净基差、未再投资 IRR 与 CTD；公共 Protobuf、数据库 migration 和既有 Phase 2A/2B expected、Oracle、容差不变。
- C++20 内核从冻结债券日程推导 `x`、`n`、购入/交割应计利息和持有期票息；生产 Rust/C++ 结果通过独立 Decimal Golden Case，对中金所规则事实绑定官方来源与内容摘要。
- 结果以确定性 Arrow Artifact 绑定完整输入和血缘，并通过真实 PostgreSQL 16 + Ceph RGW 发布、adapter 重建后重放和篡改 fail-closed。期现套保比例、外部数据适配、保证金、交易所交割流程和 UI 不在本候选；完整验收与残余风险以 [`docs/history/iterations/2026-07-phase2c-futures-delivery.md`](../history/iterations/2026-07-phase2c-futures-delivery.md) 为准。

## Phase 2B 收益率曲线与 Carry/Roll-down 候选（2026-07-19）

- 新增 CFETS 风格区间内线性 YTM 曲线，以及固定利率/贴现国债的未融资 Carry/Roll-down 分解；公共 Protobuf、数据库 migration 和既有 Phase 2A expected/Oracle/容差不变。
- 生产 Rust/C++ 结果通过独立 Decimal Oracle 与官方 QuantLib 1.42.1 对照；结果以确定性 Arrow Artifact 绑定完整输入和血缘，并通过真实 PostgreSQL 16 + Ceph RGW 发布、重启重放和篡改 fail-closed。
- 国债期货、可交割券、CF、基差、IRR、CTD、套保、外部数据适配和 UI 均不在本候选；完整验收与残余风险以 [`docs/history/iterations/2026-07-phase2b-curve-carry-roll.md`](../history/iterations/2026-07-phase2b-curve-carry-roll.md) 为准。

## Ceph RGW 对象存储迁移候选（2026-07-19）

- 新增独立迭代，把对象存储服务端从 MinIO 更换为 Ceph RGW 20.2.2，把 Rust client 从 `minio 0.4.0` 更换为 Apache `object_store 0.14.1`；公共 Protobuf、数据库 migration 和业务数值语义不变。
- 活动 Cargo 锁文件与可达依赖图不再包含 `minio` 或 `async-std`，供应链 `risk_acceptances` 收敛为空；`RUSTSEC-2025-0052` 的历史接受记录仍保留在旧迭代证据中，但不再适用于当前候选。
- 开发 Compose 和 Linux business-loop CI 统一使用锁定摘要、非 root、只读根文件系统的单节点 Ceph RGW 夹具。它用于真实 S3/业务回归，不授权生产 Ceph 集群、数据迁移或发布。
- 本节的最终命令、测试数量、候选 commit 和残余风险以 [`docs/history/iterations/2026-07-ceph-object-store-migration.md`](../history/iterations/2026-07-ceph-object-store-migration.md) 与对应 Pull Request 为准；以下旧发布说明保持其发生时的历史事实。

## v0.1.0-test.1 GitHub 测试环境（2026-07-17）

- Human 已授权第一个 GitHub `test` Environment 和 Linux 测试机发布链路；这是一项 Iteration 3 关闭后的 Delivery 活动，不恢复业务迭代。
- 发布对象仅为 `ficant-server`、`ficant-worker`、`ficant-web` 三个 Linux 镜像。精确 `cargo tree --locked -p <binary>` 依赖闭包均排除 `minio` 和 `async-std`。
- GitHub Runner 构建并推送 GHCR `sha-<commit>` 镜像、SBOM 和 provenance，Trivy 对可修复 HIGH/CRITICAL 运行时漏洞 fail-closed。
- Linux 测试机不编译源码，只执行固定 PostgreSQL 镜像、九项版本化 migration、SHA 镜像拉取、Compose、健康和冒烟检查。
- MinIO、对象存储 adapter、完整业务 UAT 和生产发布不在本次授权内。`RUSTSEC-2025-0052` 的源码接受范围不扩展到对象存储运行时；在任何对象存储运行时或 2026-10-13（取较早者）重新评估。
- 自动回滚只切换已构建镜像；数据库 migration 不自动向下回滚，继续要求扩展—兼容—收缩。

## Iteration 3 私有 GitHub 发布与关闭（2026-07-16）

### 发布目标与边界

- 目标仓库为私有仓库 `kayz/ficant`，默认分支为 `main`；正式候选 `f300597` 以 `main@80f4870` 为唯一 parent，tree 为 `7e8d6c6`。
- 源码集成路径已完成：候选分支、PR #1、候选十项 CI、外部只读 Audit、Human 独立 merge 授权、squash merge `6e346d0`、收口 merge `1053aae` 和 main 十项 CI。Human 随后单独授权 `v0.1.0-alpha.3` 私有、仅源码 GitHub Pre-release；实际 tag/Release 状态以 GitHub 页面为外部权威。
- 本次候选交付 Iteration 3 已接受的 CGB 固定利率/贴现债分析纵向切片、当时的 HOQA 治理基线与 Windows runner、Q-001..Q-036 自动化、真实 PostgreSQL/MinIO SIT 和候选绑定的发布门禁；HOQA 与 runner 后续已归档到 `docs/history/hoqa/`，不再是活动开发或发布入口。本候选不包含 UAT、部署、曲线、期货、SDK/CLI、新 migration 或公共 Protobuf 变更。

### 候选与证据绑定

- 正式候选必须是当前私有 `main` 之上的一个 commit；commit、tree、parent 和供应链 evidence digest 在候选形成后记录于外部执行证据，避免候选文件自引用其自身 hash。
- Iteration 3C 的冻结业务证据已由 Human 接受：Q-001..Q-036 映射完整，production-native 冻结用例 12/12、Oracle 自测 31/31、Storage 31/31、Acceptance 14/14、契约 11/11、health 5/5、Release/ASan CTest 各 4/4，无 blocking defect。
- 3D 复用 3C Quality 报告；只有实现、测试、Oracle、expected、容差或候选行为发生变化时才重新激活 Quality。本轮发布/治理资产变化由 Orchestrator 运行 repo-policy、供应链、许可证、秘密扫描和可复现性门禁。
- 候选 CI `29464247114`、Audit verdict `pass`（0 blocking finding）与 main CI `29472793718` 共同绑定最终发布；merge commit 与候选 tree 相同且只有一个 parent。

### 风险检查点

- `RUSTSEC-2025-0052` 仍是 `INFO / unmaintained`，没有 patched version。2026-04-23 发布的最新 `minio 0.4.0` 仍直接依赖 `async-std ^1.13`，当前 production Storage 依赖链可达。
- Human 于 2026-07-16 重新评估并限时接受该风险至 2026-10-13，范围为私有 GitHub 源码集成及 `v0.1.0-alpha.3` 仅源码 Pre-release；该决定不沿用 Iteration 2 的旧接受记录。
- 本次 Pre-release 只包含 GitHub 自动源码归档，不附二进制或签名，不等于生产部署，也不授权 UAT、运行时或外部部署。任何运行时或外部部署前必须重新评估替代 S3 client；到期日仍为 2026-10-13。

### 发布与回滚方案

1. 已审计的单一 squash 候选 `f300597` 已通过候选分支与 PR #1 发布，没有直接 push `main`。
2. 候选精确 SHA 的十项 CI 和 Audit 通过后，Human 独立授权 merge；GitHub 生成线性 squash commit `6e346d0`。
3. merge 后 `main` 十项 CI 全绿，最终源码集成证据闭合。
4. 源码集成本身如需回滚，创建显式 `git revert` PR 并重新运行同一 CI，禁止改写私有 `main` 历史或强推；Pre-release 如需撤回，删除 GitHub Release 与 tag 必须再次取得 Human 明确授权。本次没有运行时部署可回滚。
5. 经 Human 单独确认后，从最终已审计 `main` 创建 `v0.1.0-alpha.3` tag 与私有 GitHub Pre-release；只使用 GitHub 自动源码归档，不附二进制、不签名。GitHub 页面承担外部发布证据，部署和 UAT 仍禁止。

> **历史记录（superseded）：** 本文保留 iteration-2 的 Delivery/Review 交付证据。旧治理责任和 verdict 只作历史审计；ADR-0008 已被 ADR-0009 取代，当前本地开发由 OPAID 管理、发布由中央 CI/CD 管理。

## 候选与证据绑定

- GitHub Actions `ci` run [`29193249268`](https://github.com/kayz/ficant/actions/runs/29193249268) 于 2026-07-12 完成，repo-policy、contract、python、migration、rust、cpp、business-loop、supply-chain、web、reproducibility 十个 gate 均为 `success`。其候选为 `ef96c5edea11b0d5f6ebc693501f40a9b40df061`，树为 `2d1fa3a1be11e563c486d7c67df349ec06faf4d0`。
- 最终 integration commit 为 `9f044b796a912746df2080c5d42bf696797c4424`，树同为 `2d1fa3a1be11e563c486d7c67df349ec06faf4d0`。此前 run `29191239090` 是较早发布树的成功检查点，不再作为当前最终 CI 结论。
- 最终 Compose 验收基线为 commit `87db3897d82b0bea4e35eee3595178f366bbf041`、树 `e8fb65c5a86bac93382e93e50c90926954e4298f`。该基线到最终树之间仅许可证清单和证据文档发生变化，Compose 配置、镜像构建配置及运行时安全检查 blob 未变化，因此下述 live Delivery 证据适用于当前最终树；它与最终 CI 证据互补而不互相替代。

## 前两轮阻断与关闭

- 第一轮在树 `3baed89aa1791bd32a9d19d5ab244d6c279c1ef1` 上失败：MinIO 用户 `1000:1000` 无法写入新命名卷 `/data`。修复 `af0197c299a7baf9a92f4fe129e9849bdca89601` 使用锁定基础镜像构建 hardened MinIO runtime image，并在镜像层建立所有者为 `1000:1000` 的 `/data`。
- 第二轮在包含上述修复的树 `6e5898bf2db9c0dca5f29b4a8d73a9f06163de68` 上失败：Compose 将未配置的 `FICANT_BOOTSTRAP_*` 变量作为空字符串注入，server 返回 `bearer credential must not be empty`。修复 `87db3897d82b0bea4e35eee3595178f366bbf041` 在未配置时从容器环境中省略这组三个可选变量。
- 两轮失败后均执行了带卷和 orphan 的完整清理；本轮预检确认项目资源为零。

## 最终 Docker/Compose 验收

- 环境：Docker Desktop Linux `linux/x86_64`，kernel `6.18.33.2-microsoft-standard-WSL2`；Docker client/server `27.5.1`；Docker Compose `v2.32.4-desktop.1`。
- 项目名为 `ficant-iteration-2`，配置为 `deploy/dev/docker-compose.yml`，profile 为 `dev`。凭证、签名密钥和 trace 密钥仅通过验收进程环境变量注入，未写入仓库或本文。
- resolved gate 命令为 `docker compose ... config --format json | python python/compose_security_gate.py resolved --project ficant-iteration-2`，结果为 `PASS`。
- 唯一启动命令为 `docker compose ... up -d --build --wait --wait-timeout 600`。七服务 DAG 完整启动：PostgreSQL、MinIO、ficant-server、ficant-worker、ficant-web 均健康；migration 与 minio-init 均以退出码 0 完成。
- live inspect 通过 `python/compose_security_gate.py runtime --project ficant-iteration-2`：七个容器均为非 root、只读根文件系统、`CapDrop=ALL`、无 `CapAdd`、启用 `no-new-privileges`、仅回环端口绑定，并具有冻结的 CPU、内存、PID、tmpfs、只读挂载、镜像来源和健康检查。
- MinIO live 检查确认进程 UID/GID 和新卷 `/data` 所有者均为 `1000:1000`，可创建、读取并删除写入标记；其 HTTP live probe 通过。PostgreSQL `pg_isready` 通过，八个 migration 版本可读。
- 最小交付 smoke 通过：server 容器内 readiness probe 成功；worker 和 web 的 health/readiness HTTP 路径返回 `ok`；server gRPC-Web CORS preflight 从允许的精确 origin 返回 HTTP 204。未重复 Quality 的完整业务套件。
- 对五个常驻服务执行一次 restart 后，五者均重新达到 healthy；PostgreSQL 测试行和 MinIO 卷写入标记均可读取，证明重启后的持久性。测试表与写入标记在销毁前已主动删除。

## 清理证据

- 已执行 `docker compose ... down --volumes --remove-orphans --rmi local`。
- 复核 `ficant-iteration-2` 项目标签及 `ficant/minio-runtime` 镜像名范围：容器、网络、卷、构建镜像、测试数据和项目 cache tag 均为零。
- 主机上的其他 Compose 项目未被停止、删除或改写。

## 当前交付判断

当前已验收树的 Docker/Compose 专项结论为 `PASS`，前两轮 runtime blocker 均已由真实证据关闭。2026-07-13 closure audit 不修改运行时、Compose、Migration 或业务行为；closure CI `29200796715` 十项全绿，独立 Quality 为 `PASS-WITH-ACCEPTED-RISK`，内部 Review 为 `pass-with-accepted-findings`。iteration-2 状态已收敛为 `CLOSED`。

## RUSTSEC-2025-0052 发布约束

- 状态：`accepted-unfixed`；RustSec 类型为 `INFO / unmaintained`，不是已知可利用漏洞，也没有 patched version。
- 发布 Workspace/生产 storage 代码链 `async-std 1.13.2 -> minio 0.4.0 -> ficant-storage` 可达；当前 server/worker 尚未直接装配该 adapter。接受不能写成 lock-only、已修复或 ignored。
- `minio 0.4.0` 是 2026-07-13 的 crates.io 最新版，上游活跃但仍依赖 `async-std`，因此本轮不进行伪升级。
- iteration-2 可以按内部开发切片收束；iteration-3 Entry Gate、首次外部发布或 2026-10-13 前（最早者）必须验证受维护替代方案，届时不得自动沿用 D-026。

## Closure audit

- 原先 `Review skipped by explicit human authorization` 只作为历史过程偏差保留；2026-07-13 已补齐新的独立 Quality 与内部 Review，不再是最终 lifecycle 状态。
- 第一次 closure CI `29200599639` 因契约脚本引用干净单分支 clone 不可达的内部基线而失败；比较基线前移到已发布 `main@7378073` 后，run `29200796715` 的 Contract 与其余九项 required job 全部成功。
- 状态 successor 只更新 Quality/Delivery/Review 文档事实；机器策略和生产边界不变，fast-forward `main` 前继续要求 required CI 与 targeted final Review。

## 有效期

有效至当前候选树、Compose 配置、镜像锁或运行时合同发生变化。
