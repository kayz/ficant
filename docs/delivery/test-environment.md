# GitHub test 环境运行合同

## 范围

本合同覆盖 `ficant` 收敛后单进程服务底座（[ADR-0030](../architecture/adr/0030-core-reset-and-trading-desk-ibor.md)）的自动化版本构建与测试环境交付：Human 版本 tag 授权、GitHub 版本 CI、GHCR 不可变镜像构建与漏洞扫描、以及 Linux 测试机单容器 Compose 运行约定。它不授权生产发布。

## 触发与制品

- **自动触发**：Human 创建符合语义化版本格式、指向当前 `main` 精确提交的不可变 `v*` tag；普通 branch push、Pull Request 和 `main` 合并不运行完整 GitHub CI。
- **手动重试**：`release-test` workflow dispatch 只接受已存在的不可变 `v*` tag，并重新解析该 tag 对应的原始候选 Commit SHA 与 Tree SHA。
- **应用镜像**：由 GitHub Linux Runner 基于 `deploy/dev/RustService.Dockerfile` 构建 `ghcr.io/kayz/ficant-server:sha-<40位CommitSHA>`，生成 SPDX SBOM 与 SLSA provenance，经 Trivy `HIGH,CRITICAL` 漏洞扫描通过后提升为不可变 `:<version>` tag。不得创建或更新可变 `latest` tag。

## 测试机运行约束

- 根目录：`/srv/ficant-test`，配置文件见 `deploy/test/compose.test.yml`。
- 单服务拓扑：仅运行 `ficant-server` 容器（同时提供原生 gRPC 与 `gRPC-Web`），无需外部 PostgreSQL 或 Ceph 对象存储。
- 安全基线：非 root 用户（`1654:1654`）、只读根文件系统（`read_only: true`）、`cap_drop: [ALL]`、`no-new-privileges:true`，仅向宿主机回环地址 `127.0.0.1:${FICANT_SERVER_PORT:-28080}` 暴露端口。
- 测试机只拉取 GHCR 不可变 SHA 镜像并执行健康检查与原子回滚（`deploy/test/bin/`），不得在目标机执行任何现场源码编译。
