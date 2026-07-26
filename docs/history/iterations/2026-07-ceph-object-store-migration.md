# 2026-07 Ceph 对象存储迁移

## 目标

- 在治理收敛完成后的精确 `main` base `83d2f030f9df9535c22d36f5872dd25a2cc242d7` 上，将对象存储服务端从长期社区版方向不足以信任的 MinIO Server 迁移到由 Ceph Foundation 治理的 Ceph Object Gateway（RGW）。
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
- 供应链候选拓扑继续冻结精确 base、候选 commit 数和无 merge 线性历史，但允许一个迭代的多个 forward-only checkpoint；main 的最终 squash-merge 合同不变。

## 需 Human 决策

- 已决定：现在同时替换 MinIO Server 与 Rust client，以 Ceph RGW + Apache `object_store` 作为长期基线。
- 无待决业务语义。若官方 Ceph 镜像无法在当前 Docker Desktop/Linux CI 边界内形成可重复的真实 RGW fixture，必须返回 Human 重新选择服务端，不得用 mock 或仍在运行的 MinIO 冒充验收。

## 最终真实测试证据

- `./scripts/check-fast.ps1`：exit 0；Rust workspace check、非环境测试和 storage library 3 项单元测试通过。
- `./scripts/check.ps1`：exit 0；严格 Clippy/build/test、生成契约 11/11、C++ CTest 6/6、Phase 2A matrix 36/36、Phase 2B matrix 16/16、Python 1/1、Web 4 files / 29 tests 全部通过；使用锁定 Node 22.17.0、pnpm 10.12.4、uv 0.7.13 与 Buf 1.56.0。
- `./scripts/check.ps1 -IncludeIntegration`：exit 0；真实 PostgreSQL 16 + 固定 Ceph Tentacle 20.2.2 digest 上 migration 4/4、Phase 1 正向业务闭环 1/1、负向不变量 13/13、Phase 2B 发布重放 1/1 全部通过。覆盖签名读写、完整 lineage、重启读取、内容篡改、幂等、失败恢复与 orphan cleanup，不是 mock 或 MinIO 替代。
- Windows Docker Desktop 首次真实启动发现 checkout 的 CRLF 使解释器变为 `bash\r`；候选增加全仓库 `*.sh eol=lf` 契约并在 Ceph 镜像构建时防御性归一化，重建后 RGW 健康。一次性 `ficant-phase2b` 容器、网络和 PostgreSQL/Ceph volumes 在验收后全部删除，剩余数量为 0。
- GitHub PR run [`29672981319`](https://github.com/kayz/ficant/actions/runs/29672981319) 继续提供 Linux 上同一固定 digest 的独立历史证据；本轮本地结果补齐 Windows Docker Desktop 复现，不把远端结果回写为本地通过。

## 残余风险

- Ceph 单节点 fixture 只验证协议兼容性和持久性，不证明生产高可用、容量、性能或灾备设计。
- `object_store` 的升级仍需精确版本锁、SBOM、许可证和 advisory 门；本次消除 MinIO 风险不构成对未来版本的自动接受。
- Quay 直连在本机仍可能出现大层 EOF；本次通过镜像代理取得完全相同的 OCI index digest，并由 Docker checksum 与官方仓库 digest 再登记验证。摘要锁定关闭内容替换风险，但注册表可达性仍是环境可用性风险。
