# ADR-0010：采用 Ceph RGW 与 Apache object_store

- 状态：Accepted
- 日期：2026-07-19
- 范围：对象存储服务端、Rust S3 client、开发/CI 夹具与旧 MinIO 风险接受
- 取代：既有 MinIO 服务端与 `minio` Rust crate；关闭 D-026 的活动限时风险接受

## 背景

原 Storage adapter 使用 `minio 0.4.0`，其可达依赖包含已停止维护的 `async-std 1.13.2`。该问题没有可验证的小版本升级路径，且 MinIO 社区版的长期产品方向不足以作为本长期项目的唯一对象存储承诺。由于对象存储尚未装配进正式 server/worker 运行时，也没有生产数据迁移，本阶段同时更换服务端和客户端的成本最低。

## 决策

生产目标对象存储选用 Ceph RADOS Gateway（RGW）。Ceph 由 Linux Foundation 下的 Ceph Foundation 支持，采用多组织治理；RGW 提供项目所需的 S3 GET、PUT、HEAD、DELETE 和 multipart 能力。当前开发与 CI 锁定 Ceph Tentacle `20.2.2` 的 OCI index digest，并记录 Linux amd64 manifest digest；不使用 `latest` 或停止维护的 `ceph/daemon`、`ceph/cn` 镜像。

Ceph 上游说明其主体代码按 `LGPL-2.1-only OR LGPL-3.0-only` 双重许可，少量文件另有许可证；工具链锁与运行时标签记录该主体表达式。官方容器还包含 CentOS 与系统包，因此标签不替代镜像级 SBOM/许可证清单，正式发布仍须由中央供应链门禁对精确 digest 扫描。

Rust client 选用 Apache Arrow 社区维护的 `object_store 0.14.1`，只启用 `aws` feature。Storage 对外仍暴露现有 narrow application ports，使用 path-style S3、显式 endpoint、`us-east-1` 签名区域和环境注入凭证；Ceph 默认 zonegroup 的 `api_name` 也显式设为同一区域，避免服务端 LocationConstraint 与客户端 SigV4 scope 漂移。夹具的原始 SigV4 空载荷请求必须按协议显式生成 canonical request、HMAC key chain 和标准空内容 SHA-256；不能依赖 Ceph 20.2.2 镜像内早于相关 S3 修复的 curl 7.76.1 签名器，也不以匿名写或关闭认证绕过兼容性验证。不把 Ceph 类型泄漏到 Domain、Application、Protobuf 或数据库 schema。

开发 Compose 与 GitHub business-loop 使用同一个基于官方 Ceph 镜像构建的单节点 RGW 夹具。夹具以非 root UID/GID `167:167` 运行，根文件系统只读，持久状态只写入命名卷；它只用于兼容性与业务验收，不是生产容量、故障域、复制或升级方案。生产 Ceph 必须由独立运维工作确定至少三节点的 MON/OSD、复制/纠删码、备份、监控、密钥轮换和滚动升级。

旧 `minio`、MinIO server/client 镜像和 `async-std` 从活动代码、Compose、CI 与 Cargo 锁文件移除。供应链活动风险接受集合必须为空；历史 evidence 与 release notes 保留原事实，不回写成当时已经修复。

## 选择依据

- Ceph 有基金会治理、跨厂商参与和明确的稳定版本生命周期，降低单一公司的产品策略风险。
- RGW 是 Ceph 的正式对象网关，而非附属兼容层；项目当前只依赖其稳定 S3 子集。
- `object_store` 属于 Apache Arrow Rust 生态，维护活跃、许可证宽松，并把 client 与某个服务端厂商解耦。
- 服务端与 client 同时更换后，真实 PostgreSQL + RGW 验收可以直接覆盖签名、错误映射、内容寻址、完整性检测和 orphan 清理。

参考：[Ceph Foundation](https://ceph.io/en/foundation/)、[Ceph active releases](https://docs.ceph.com/en/latest/releases/)、[RGW S3 API](https://docs.ceph.com/en/latest/radosgw/s3/)、[Ceph container images](https://docs.ceph.com/en/latest/install/containers/)、[Apache object_store](https://github.com/apache/arrow-rs-object-store)。

## 替代方案

- 继续等待 MinIO 或 `minio` crate 上游升级：拒绝，因为时间表和最终结果不受本项目控制，长期维护风险继续存在。
- 只替换 Rust client、保留 MinIO server：拒绝，因为只能消除直接依赖，不能消除服务端长期方向的不确定性。
- Garage 或 SeaweedFS：二者均可提供开源 S3 能力且部署更轻，但本项目优先选择基金会治理、长期版本生命周期和更成熟的多组织运维生态。
- 自研对象存储或维护 `minio-rs` fork：拒绝，因为会把协议、安全和维护责任转移到单人项目。

## 代价与升级条件

Ceph 比 MinIO 更重，单节点开发夹具的启动和镜像下载成本更高；它也不能证明生产高可用。该代价由固定摘要、共享夹具、持久开发卷和只在集成门运行来控制。

每次 Ceph 次版本升级、`object_store` minor 升级、S3 调用集合扩展或生产装配前，都必须重跑真实 RGW 的对象存储测试、Phase 1 正/负业务闭环与重启读取。当前 Tentacle 20.2 系列公布的预计支持截止日为 2027-11-18；最迟在截止日前六个月完成下一受支持 major 的兼容验证和升级决策。
