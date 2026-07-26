# Parquet vendor manifest 收口

## 目标

- 在精确基线 `e488a446192a8714dfa54b5fec8d765b21c5e425` 上恢复统一快速本地检查入口。
- 让 vendored `parquet 59.1.0` 的 Cargo manifest 只声明仓库实际保存并允许进入生产构建的 target。
- 保持 Phase 3B 的 Parquet 编码、Manifest、Snapshot 血缘和确定性重放结果不变。

## 验收

- `cargo fmt --all -- --check` 不再因不存在的 Parquet examples、tests 或 benches 失败。
- `./scripts/check-fast.ps1` 在同一最终候选上 exit code 0，覆盖 workspace check、非环境回归、storage library、Phase 3A 与 Phase 3B。
- vendored 生产源码、上游补丁文件、依赖版本和公共业务合同不变。

## 非目标

- 不升级 Arrow/Parquet，不改变 writer 参数、压缩、分区或 schema evolution 合同。
- 不补入供应链策略禁止进入发布树的 Parquet examples、tests、benches 或开发工具。
- 不修改公共 API、Protobuf、数据库 migration、数值 Oracle、expected、断言或容差。
- 不修改版本 workflow、构建镜像、创建版本 tag、部署或发布。

## 公共契约变化

- 无公共业务契约变化。
- 本地 `parquet 59.1.0` 包装 manifest 删除 4 个 example、6 个 test 和 16 个 bench 声明；其 lib、9 个实际存在的 bin、features 与生产依赖保持不变。

## 需 Human 决策

- 当前无待决业务语义。
- crates.io 发布首个包含 Apache Arrow 提交 `bc4e672607f00587349b1308f6cf717fc6518848` 的正式 Parquet 版本后，仍必须以独立兼容迭代删除 vendor，并重新验证确定性 Parquet 字节合同。

## 最终真实测试证据

- 基线 `./scripts/check-fast.ps1`：exit code 1；第 1 步列出 4 个不存在的 example、6 个不存在的 test 和 16 个不存在的 bench，证明失败来自 manifest 与允许发布树不一致。
- 最终候选 `./scripts/check-fast.ps1`：exit code 0；Rust formatting、workspace check、非环境回归、storage library 3/3、Phase 3A canonical ingestion 5/5、Phase 3B deterministic snapshot codec 2/2 全部通过。
- `cargo clippy --offline --workspace --all-targets --locked --exclude ficant-contracts --exclude ficant-contract-tests --no-deps -- -D warnings`：exit code 0；产品 workspace 全 target 严格 Clippy 通过。
- `cargo build --offline --workspace --all-targets --locked`：exit code 0；本地 workspace 全 target 构建通过。
- `./scripts/check.ps1 -ListOnly`：exit code 0，列出 24 条离线本地检查命令。
- `./scripts/check.ps1`：exit code 1，预检阶段因当前终端缺少冻结 `uv 0.7.13` 而在执行任何测试前退出；未联网安装、未使用 shim，也未把该命令写成通过。

## 残余风险

- 当前候选只修复已确认的 manifest/发布树不一致；完整本地入口仍依赖 Human 环境预先提供冻结 `uv 0.7.13`、Buf、Node、C++ 工具链和离线缓存。
- vendoring 仍是临时供应链措施；正式上游版本可用后必须退出本地副本，不能演变成长期私有 fork。
