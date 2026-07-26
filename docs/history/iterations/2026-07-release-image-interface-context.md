# Release image interface context correction

## 目标与验收

- 修复 `v0.1.0-alpha.4` 发布运行中 `ficant-worker` 镜像无法读取编译期嵌入 Proto 合同的问题。
- 验收句：正式 Rust 服务 Dockerfile 在隔离构建上下文中包含 `interface/`，仓库策略测试会阻止该合同再次漂移，精确候选通过规定的本地检查。

## 非目标

- 不移动或复用失败的 `v0.1.0-alpha.4` tag。
- 不改变 Proto、Rust API、业务语义、镜像基础层或发布拓扑。
- 不在本迭代创建新版本 tag、部署或修改目标环境。

## 公共契约变化

- 无产品公共契约变化。
- 构建合同补充：`deploy/dev/RustService.Dockerfile` 必须复制仓库 `interface/`，以满足 Rust crate 的编译期 `include_bytes!` 依赖。

## Human 决策

- 本地候选合并到 `main` 后，需要 Human 选择一个新的 forward-only 版本号；`v0.1.0-alpha.4` 永久保留为失败的不可变候选。

## 最终测试证据

- `bash .github/scripts/tests/run-repo-policy-tests.sh`：exit code 0，`repo-policy-tests: PASS`。
- `bash .github/scripts/verify-repo-policy.sh --stage final`：exit code 0，`repo-policy (final): PASS`。
- `.\scripts\check-fast.ps1`：exit code 0，格式、workspace check、非环境测试、storage 与 Phase 3A/3B 检查全部通过。
- `cargo build --offline --locked --release --bin ficant-worker`：exit code 0，精确 release binary 在 7m14s 内完成。
- 使用正式 Dockerfile、已缓存的 Rust 1.96.1 与 runtime 镜像启动本地隔离构建：`COPY interface ./interface` 层成功，随后 Cargo 在线索引长期无输出，主动停止，未计为完整镜像构建通过。

## 残余风险

- GitHub Actions 当前把仍声明 Node.js 20 的锁定 action 强制运行在 Node.js 24；本次失败与该告警无关，但需在后续维护迭代升级 action。
- 本机无法离线提供 Linux Cargo registry 缓存；正式 Docker 冷构建需由新版本候选的 GitHub 发布链闭合。
