# alpha.8 供应链与发布前门禁收口

## 目标

修复 `v0.1.0-alpha.8` 暴露的许可证清单输入绑定漂移，并让本地发布候选预检在创建版本 tag 前离线拒绝锁文件或 release-tree 源码漂移。

## 验收

- 权威刷新命令在 642 项 Cargo/PyPI/npm Syft 宇宙不变的前提下，刷新当前锁文件、一方与 vendored release-tree 源码以及 canonical header 的派生绑定。
- 第三方包、许可证判断、allowlist、scoped exception 和一方许可证政策保持不变。
- 离线输入绑定入口验证三份锁文件、generator、Syft tool 身份、inventory digest、canonical header，以及一方和 vendored release-tree 源码完整性。
- 发布候选预检在构建镜像前运行绑定回归测试和真实清单校验，`-ListOnly` 明确显示两项步骤，任一步失败立即退出。
- Cargo.lock 漂移、一方源码漂移均被负向测试拒绝；LF 与 CRLF checkout 得到相同绑定结论。

## 非目标

- 不改变依赖包集合、许可证结论、allowlist、例外或业务语义。
- 不降低漏洞、许可证、供应链、迁移、回滚或健康检查门禁。
- 不移动、删除或复用 `v0.1.0-alpha.8`，不创建新的版本 tag，不部署测试环境。

## 公共契约变化

- `verify-license-inventory.py refresh-bindings` 成为现有完整清单刷新锁文件和 release-tree 派生绑定的权威入口。
- `verify-license-inventory.py verify-bindings` 成为无需在线 Syft 的发布前输入绑定校验入口；完整版本 CI 继续以真实 Syft 宇宙运行更强的 `verify`。
- `scripts/check-release-candidate.ps1` 在正式镜像构建前执行绑定回归测试和当前仓库绑定校验。

## 需 Human 决策

无；Human 已批准以 forward-only 修复、PR 和合并完成本迭代，同时明确禁止创建新版本 tag。

## 最终真实测试证据

- `python .github/scripts/tests/test_license_inventory_bindings.py`：exit 0，8 项通过；覆盖 Cargo.lock、一方源码、canonical header、政策包缺失、刷新原子性及 LF/CRLF 等价性。
- `bash .github/scripts/tests/fixtures/license-inventory/run.sh .github/scripts/verify-license-inventory.py`：exit 0，既有完整许可证 fixture 全部通过。
- `verify-license-inventory.py refresh-bindings`：exit 0；随后以 `v0.1.0-alpha.8` 真实 Syft evidence 运行完整 `verify` exit 0。原始 Syft artifacts 为 650 项，其中受权威清单管理的 Cargo/PyPI/npm 宇宙精确为 642 项；第三方条目变化 0，一方仅 9 项 `source_integrity` 变化。
- 新清单绑定为 `Cargo.lock=d16848130d9ff60018cf95c4ea227fd208223eac16babca69ee4490244c1ee19`、`input_tree_digest=18e28f950f9f15c4201d772a890cb1c757f868ab3079a2415b9720463b75bed5`、`inventory_digest=cb150ed81956258a87c0389ddc5fb24e7397d90e561960384e539e5f6da0fe7b`。
- `scripts/check-release-candidate.ps1 -ListOnly`：exit 0，绑定回归测试与真实绑定校验分别显示为步骤 1、2，随后才是镜像构建、扫描和运行拓扑。
- `scripts/check-fast.ps1`：exit 0；Rust workspace check、非环境回归、Storage library、Phase 3A/3B 定向测试全部通过。
- `scripts/check.ps1`：exit 0；Rust 259 项、CTest 8 项、Python/Oracle 7 项、Web 29 项全部通过，四组 acceptance matrix 分别为 36/36、16/16、18/18、18/18。
- `scripts/dev-up.ps1`：exit 0；本地 PostgreSQL、Ceph RGW、migration、Server、Worker、Web、UI 全部健康。
- `scripts/check.ps1 -IncludeIntegration`：exit 0；包含完整本地回归，并额外通过 31 项真实 PostgreSQL/Ceph 集成测试，其中 Phase 4 lease queue 1 项、执行闭环 3 项、生产 Worker 1 项。
- `scripts/check-release-candidate.ps1`：待本迭代 PR 合并到干净且与 `origin/main` 一致的 `main` 后执行；该入口按合同拒绝在 feature branch 上冒充发布候选。

## 残余风险

- 完整版本供应链仍需由 Human 后续创建的新不可变版本 tag 在 Linux Runner 上重新生成 Syft/OSV/Gitleaks 证据；本迭代不创建 tag。
- Ceph 基础系统的 Trivy OS 支持缺口不在本迭代改变，将在独立 storage-runtime 迭代继续保持显式风险。
