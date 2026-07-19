# 2026-07 Ceph 对象存储迁移

## 目标

- 在精确 base `2e986673df1b0dfbab29094313ac913e91377994` 上，将对象存储服务端从长期社区版方向不足以信任的 MinIO Server 迁移到由 Ceph Foundation 治理的 Ceph Object Gateway（RGW）。
- 将 Rust S3 客户端从 `minio` 迁移到 Apache `object_store`，删除 `async-std` 的既有可达链。
- 将产品代码、测试、部署和活跃文档中的品牌耦合收敛为通用 S3 契约；历史证据保留当时事实。
- 使用多个 forward-only 快速子循环分别完成 Ceph 可运行性、客户端迁移、部署迁移和最终收敛，每个子循环只运行针对性测试，不创建额外治理文档。

## 验收

- 活跃 Rust workspace 不再依赖 `minio`；`cargo tree --locked -i minio` 和 `cargo tree --locked -i async-std` 均无可达包。
- `S3BlobStore` 在真实 PostgreSQL 16 与固定 Ceph RGW 镜像上通过既有正向业务闭环、13 项负向不变量、重启重放、内容篡改检测、失败恢复和 orphan cleanup。
- `deploy/dev`、本地集成合同和中央 CI 不再启动 MinIO Server 或 `mc`，改为固定版本和 digest 的 Ceph RGW；测试凭据继续只由环境注入。
- Protobuf、数据库 migration、Artifact/Arrow 格式、内容哈希、幂等、租户、所有者和 required-read fail-closed 语义不变。
- 精确候选依次通过 `./scripts/check-fast.ps1`、`./scripts/check.ps1` 和 `./scripts/check.ps1 -IncludeIntegration`；缺少离线缓存或本地 Ceph 能力必须如实记录，不得冒充通过。
- 活跃 README、scope、data dictionary、development、delivery、ADR 和供应链说明准确描述 Ceph RGW + Apache `object_store` 的当前边界。

## 非目标

- 不设计或部署生产 Ceph 高可用拓扑、OSD 容量、跨站复制、灾备、监控和升级编排。
- 不迁移任何真实或共享 MinIO 数据；当前对象存储数据均视为一次性本地/CI fixture。
- 不把对象存储 adapter 装配进首版 `ficant-server` 或 `ficant-worker` 发布闭包，不扩大产品 Phase。
- 不改变 Oracle、expected、数值断言、容差、公共 API、Protobuf 或数据库 schema。
- 不触发合并、发布、SIT、UAT、生产部署或服务器运维。

## 公共契约变化

- Rust 实现类型从 `MinioBlobStore` 更名为服务端中立的 `S3BlobStore`；Application ports 与业务调用语义不变。
- 测试环境变量保留既有 `FICANT_TEST_S3_*` 名称，只把错误文字和运行时身份从 MinIO 改为 S3/Ceph。
- 本地开发 Compose 的对象存储服务、初始化步骤、健康检查、固定镜像和数据卷改为 Ceph RGW；fast gate 仍不启动环境服务。
- 新增 ADR 记录 Ceph RGW 与 Apache `object_store` 的选择、替代方案和重新评估条件。

## 需 Human 决策

- 已决定：现在同时替换 MinIO Server 与 Rust client，以 Ceph RGW + Apache `object_store` 作为长期基线。
- 无待决业务语义。若官方 Ceph 镜像无法在当前 Docker Desktop/Linux CI 边界内形成可重复的真实 RGW fixture，必须返回 Human 重新选择服务端，不得用 mock 或仍在运行的 MinIO 冒充验收。

## 最终真实测试证据

- `./scripts/check-fast.ps1`：exit 0；Rust workspace check、非环境测试和 storage library 3 项单元测试通过。
- `cargo clippy --offline --workspace --all-targets --locked --exclude ficant-contracts --exclude ficant-contract-tests --no-deps -- -D warnings`：exit 0；`cargo build --offline --workspace --all-targets --locked`：exit 0。
- C++ configure/build/ctest：exit 0，4/4 通过；Q-001..Q-036 acceptance matrix：36 mapped、0 missing。
- Python 锁定版本环境的 contract import：1 passed；Web 定向 typecheck/build/Vitest：exit 0，4 files、29 tests 通过。Web 定向命令由现有 Node 24 临时忽略 engine 检查执行，不替代完整门要求的 Node 22.17.0 证据。
- Compose security unit tests：23 tests，0 failed，2 skipped；解析后的 `ficant-dev` Compose security gate：PASS；CI 静态合同：PASS；空风险接受 fixture：PASS。
- `cargo tree --locked --workspace --all-features --target all`：exit 0；可达图不含 `minio` 或 `async-std`。许可证 inventory 机械重建为 632 个包并通过 source integrity、第一方分区、SPDX 限域例外和 notices 校验。
- `./scripts/check.ps1`：exit 1，预检发现本机没有固定的 `uv 0.7.13`，未进入完整命令；单独 contract test 的 2 项通过、9 项因本机没有固定 Buf 1.56.0 可执行文件而失败。两者均为本地工具环境 blocker，不记为通过。
- 两次 `docker pull quay.io/ceph/ceph@sha256:6b4b...` 均在 Quay/CDN 大层传输时以 `unexpected EOF` 失败；因此本地真实 Ceph RGW、`./scripts/check.ps1 -IncludeIntegration`、重启读取和完整正/负业务闭环尚无通过证据。本候选只能作为 draft checkpoint 推送，等待 Linux CI 的真实 RGW 结果；CI 结果不回写成本地通过。

## 残余风险

- Ceph 单节点 fixture 只验证协议兼容性和持久性，不证明生产高可用、容量、性能或灾备设计。
- `object_store` 的升级仍需精确版本锁、SBOM、许可证和 advisory 门；本次消除 MinIO 风险不构成对未来版本的自动接受。
- 当前 draft 的主要未关闭风险是官方 Ceph 镜像尚未在本机完成下载与启动；在真实 RGW business-loop 通过前不得把本迭代标记为完成或合并。
