# R9G 迭代 brief — Linux 版本门禁一致性

**面向 Human 的产品名：** 金证FICC合同管理系统 · **平台名：** FICANT · **内部迭代：** R9G · **execution base：** `791555b22b6ef8c847622621c860a8789ba9e32d` · **base tree：** `fc723187bf4421fde0b71165f239b98dd311db77` · **状态：** 已通过 [PR #73](https://github.com/kayz/ficant/pull/73) 合入并由不可变 `v0.1.0-alpha.12` 完成版本 CI、镜像晋升与测试环境交付

本 brief 是 R9G 面向 Human 的唯一范围、权限边界与最终本地证据载体。不可变 tag `v0.1.0-alpha.11` 精确绑定上述 base；其 [版本 CI run 33933106201](https://github.com/kayz/ficant/actions/runs/33933106201) 在本地 17 步 preflight 通过后暴露了 Rust 测试夹具的跨平台路径假设，以及 Web CI 启动参数未跟随当前 Server 配置合同的问题。该 CI 的 11 个 job 中 9 个成功、2 个失败；[release-test run 33934227298](https://github.com/kayz/ficant/actions/runs/33934227298) 的 7 个 job 全部 skipped，未构建、推送或部署版本应用镜像。R9G 合入和 current-truth 文档收口后，Human 另行授权的 `v0.1.0-alpha.12` 精确绑定 `42e7c323543401567f8d76166d4d13958dd55f7b`（tree `db9123eabc27d8b1d5c52dd001d57c6f5934dd32`），并完成版本 CI、镜像晋升与 GitHub `test` Environment 交付；该后续 CICD 事实不改变 R9G 当时冻结的写路径、验收与非目标。

## 1. 目标

让同一候选在 Windows 本地检查与 Ubuntu 版本 CI 中使用等价的有效测试配置：Server 正向测试夹具只使用当前平台绝对路径；Rust job 显式获得仓库锁定的 Buf 1.56.0；Web job 提供当前 `ServerSettings` 要求的完整启动配置，并在服务容器进程提前退出时立即输出捕获日志而不是空等超时。

## 2. 验收

一句话验收：在精确 R9G 候选上，七个 Server 正向夹具在 Windows 与固定 Linux Rust 镜像中均通过，Rust CI 的 topology 测试只能消费锁定 Buf 1.56.0，Web CI 以完整当前配置启动真实 gRPC-Web 服务且能对提前退出立即留证失败，相关变异门、`check-fast.ps1` 与标准 `check.ps1` 全部 exit `0`。

| 条目 | 可执行判据 |
|---|---|
| 跨平台夹具 | `composition`、`data_health_sit`、`data_source_registry_sit`、`factor_registry_sit`、`portfolio_risk_sit`、`rates_sit`、`service_topology` 的受治理文件根目录均由 `std::env::temp_dir()` 派生；生产绝对路径校验及路径注入负例保持不变。 |
| Rust 工具合同 | `rust` job 从 `deploy/dev/toolchain.lock.toml` 已锁定的 `bufbuild/buf@sha256:89fa...` 提取 Buf，并将只读可执行文件和 `FICANT_BUF` 注入固定 Rust 容器；版本必须精确为 `1.56.0`。 |
| Web 启动合同 | Web Server 容器显式接收 server runtime、bootstrap actor/tenant/owner/role 以及 file/PostgreSQL input binding；配置值满足当前生产解析器，但不放宽任何必填或身份校验。 |
| 快速留证 | Web readiness 在端口就绪前持续检查附着的容器进程状态；进程提前退出时必须输出捕获日志并立即失败，正常退出仍由唯一 trap 清理，且不得引入策略禁止的运行时 inspect。 |
| 防回归 | repo-policy 静态合同对 Rust Buf 的来源、版本、挂载与环境注入，以及 Web 必填环境和提前退出日志路径逐项做删除/漂移变异并拒绝。 |
| 本地候选 | Windows 七项目标测试、固定 Linux Rust 镜像七项目标测试、repo-policy 变异套件、YAML 解析、`check-fast.ps1` 与 `check.ps1` 均在最终候选实际执行并记录 exit code 与可得 test count。 |
| 版本边界 | `v0.1.0-alpha.11` 保持原位且不重跑、移动、复用或删除；R9G 不创建新 tag，不发布或部署，新的 forward-only 版本号须由 Human 明确选择。 |

## 3. 非目标

- 不修改业务域、公共 API/Proto、数据库 migration、生成契约、数值结果、Oracle、expected、断言或容差。
- 不放宽 `FileNdjsonQuoteSource` 的当前平台绝对路径要求，不把 Linux 专用硬编码路径引入跨平台测试。
- 不新增依赖、基础镜像或可变工具下载；Buf 继续使用现有工具链锁中的固定版本与不可变镜像摘要。
- 不要求 Web smoke 访问 PostgreSQL 或 Ceph RGW；本轮真实浏览器路径仍只验证 Platform Registry/session，外部 adapter 保持惰性且由既有业务门另行覆盖。
- 不执行测试环境部署、回滚、服务器管理或旧版本流水线重跑。

## 4. 公共契约变化

- 业务公共契约：无变化。
- 生产安全语义：无变化；受治理文件根仍必须是当前平台绝对路径，Server 身份、runtime 与 input 配置继续失败关闭。
- 交付测试合同：Rust job 新增锁定 Buf 的显式容器边界；Web job 与当前 Server 启动合同同步，并把服务提前退出从十分钟无信息超时收紧为立即输出容器日志的确定性失败。

## 5. 需 Human 决策（已完成）

- `v0.1.0-alpha.11` 是失败且不可变的版本候选，不构成已发布镜像或测试环境交付证据。
- R9G 已完成合入，current-truth 文档通过 [PR #74](https://github.com/kayz/ficant/pull/74) 收口；Human 随后明确选择 forward-only 版本 `v0.1.0-alpha.12`，并在精确、干净、同步的最终 tag commit 上重新通过 17/17 步门禁。
- `alpha.12` 的版本 CI、镜像晋升和测试环境部署均成功，R9G 不再有待定版本决策；任何后续版本仍须重新取得 Human 的精确版本授权。
- 本轮不需要改变业务语义或测试 Oracle；若实施发现必须如此，立即停止并返回 Human。

## 6. 最终真实测试证据

最终可执行候选在测试后原样提交为 `cd8e64e5b2912147cf4a24cc4247ebbdb4fe82b0`（tree `e59ca735291833b2d12da8f078d429c73d59ff85`，parent 为 execution base）；PR #73 rebase merge 后的代码提交 `6ddc399e6b72f8b59c235f1e94604158de301a4e` 保持相同 tree，随后只改文档的集成提交为 `811a7062e25af41df1316d2c883914a00c5142ed`（tree `34cba6a4afb1bd2440211ea3342c82a73d41c6af`）。下表分别记录最终本地候选的实际命令，以及 R9G 结束后由 Human 独立授权并已完成的 CICD 外部事实；两类证据不互相替代。

| 真实命令/检查 | Exit / Conclusion | 结果 |
|---|---:|---|
| `v0.1.0-alpha.11` / CI run `33933106201` | `failure` | 精确 tag/commit 为 `v0.1.0-alpha.11@791555b...`；authorize、contract、Python、C++、repo-policy、business-loop、supply-chain、migration、reproducibility 成功，Rust 与 Web 失败，0 skipped。 |
| Rust job `101215635482` | 101 | `data_health_sit` 在 Linux 将 `C:\\ficant-input` 视为相对路径，生产绝对路径校验返回 `trusted governed input catalog is invalid`；同类正向夹具共七处。 |
| Web job `101215635537` | 1 | Server 容器未获得当前必需 runtime/bootstrap/input 配置，在监听前退出；readiness 未检查容器状态或保留日志，最终空等 600 秒。 |
| release-test run `33934227298` | `skipped` | authorize、build、build-ui、scan、scan-storage-runtime、promote、deploy 共 7/7 jobs skipped；没有版本应用镜像、GHCR 晋升或测试环境部署。 |
| Windows 七组 Server 测试 | 0 | `composition`、`data_health_sit`、`data_source_registry_sit`、`factor_registry_sit`、`portfolio_risk_sit`、`rates_sit`、`service_topology` 共 12 passed、0 failed；7 个新增受治理输入根均由当前平台临时目录派生。 |
| 固定 Linux Rust 镜像同七组测试 | 0 | 使用固定 Rust image、只读挂载从工具链锁定 Buf image 提取并验证的 `1.56.0`；同样 12 passed、0 failed。 |
| Linux Rust CI 等价完整链 | 0 | `cargo build --workspace --all-targets --locked`、主 workspace tests、storage lib、canonical ingestion、snapshot codec 全部通过；证明修复后的 Rust job 不再依赖 Windows 路径或其他 job 的临时 Buf 安装。 |
| Linux Web 启动与快速失败定向验证 | 0 | 固定 Rust image 以 31 个受审 `--env` 启动真实 Server，TCP readiness 成功，真实 gRPC-Web 请求 HTTP 200、响应 260 bytes；删除 `FICANT_SERVER_RUNTIME_IMAGE_DIGEST` 的负向启动在约 2.75 秒内以精确缺项日志失败，不再空等 600 秒。 |
| `bash .github/scripts/tests/run-repo-policy-tests.sh` | 0 | release-state Python tests 9/9 通过；49 个 Linux workflow 变异与 2 个 Buf lock 变异全部被拒绝，包括错误值、可变 Buf create、重赋值、Docker env 覆盖、日志/PID decoy、cleanup 不可达与 readiness 提前成功。 |
| repo-policy / syntax / format | 0 | `bash -n`、YAML parse、`verify-repo-policy.sh --stage final`、`cargo fmt --all -- --check`、`git diff --check` 全部通过；七个目标夹具已无 `C:\\ficant-input`，并全部改为从平台临时目录派生。 |
| 一方许可证派生绑定 | 0 | 首轮 `check.ps1` 在第 32 项正确拒绝旧 `ficant-server` binding；使用既有 `refresh-bindings` 后仍为 649 packages / 20 first-party，唯一 package 字段变化为 `pkg:cargo/ficant-server@0.1.0.source_integrity`。最终 digest `44d7940dab70fb7b599e52b8a07462ef87b0638214796b7bdfa7a83adbe11c38`，绑定正负测试 11/11 通过。 |
| `scripts/check-fast.ps1` | 1 → 0 | 首轮活动 Node `v24.18.0` 被精确工具链门拒绝；仅在进程 PATH 前置本机既有 Node `v22.17.0` 后从头 23/23 步通过，未修改版本断言、expected 或容差。 |
| `scripts/check.ps1`（最终同候选） | 0 | 固定 Node `v22.17.0` 后从头 40/40 步通过并输出 `FICANT complete local checks passed.`；包括 strict Clippy、Rust 全量、C++ 9/9、Cross-Clang 71 rows、各 Decimal Oracle、Python/live SDK、许可证、Web typecheck/build 与 5 files / 35 tests。 |
| 独立只读复审 | PASS | 最终结论 blocker 0 / major 0 / minor 0；此前实测可绕过的 Buf `printf -v`、Web 动态值重赋、`-e`、`--env-file`、cleanup `if false` 与 readiness 提前 `break` 六个候选现均被拒绝。 |
| [PR #73](https://github.com/kayz/ficant/pull/73) rebase merge | 0 | PR base `791555b...`、head `944b306...`，17 个文件与冻结闭集一致；普通 PR 未触发版本流水线。合入后代码提交 `6ddc399e...` 的 tree 仍为 `e59ca735...`，R9G 集成提交为 `811a706...` / tree `34cba6a...`。远端临时分支自动删除，本地同 tree 分支删除。 |
| `trivy image --download-db-only` + `scripts/check-release-candidate.ps1`（clean `main@811a706...` / tree `34cba6a...`） | 0 | Trivy 0.72.0 DB 更新为 `UpdatedAt 2026-09-05 01:12:13Z`、`DownloadedAt 2026-09-05 02:18:25Z`；17/17 步通过。许可证/存储绑定、Server/Worker/UI 正式镜像构建、三个应用与锁定 Ceph 的扫描、OCI/Compose 身份、PostgreSQL/Ceph 健康、migration、Server/Worker/UI readiness 与 forward-only migration 兼容均通过，运行容器、卷和网络由脚本清理。 |
| [PR #74](https://github.com/kayz/ficant/pull/74) 文档收口与最终 tag preflight | 0 | PR #74 以 rebase merge 收口 R9G 的 current-truth 文档；最终干净、同步的 `main@42e7c323543401567f8d76166d4d13958dd55f7b` / tree `db9123eabc27d8b1d5c52dd001d57c6f5934dd32` 在创建 tag 前更新同日 Trivy 数据库，并重新通过 17/17 步 `scripts/check-release-candidate.ps1`。 |
| `v0.1.0-alpha.12` / [CI run `33943037740`](https://github.com/kayz/ficant/actions/runs/33943037740) | `success` | annotated tag object `d682b53ebc5168545953b2d8a59a8480036ef7bd` 与远端 peeled commit 核验一致；authorize、migration、contract、Python、repo-policy、Web、Rust、supply-chain、business-loop、reproducibility、C++ 共 11/11 jobs 成功，R9G 修复由真实 GitHub Linux Runner 证明。 |
| [release-test run `33943924358`](https://github.com/kayz/ficant/actions/runs/33943924358) | `success` | 10/10 jobs 成功；Server `sha256:61e1e57d...`、Worker `sha256:7ec42789...`、UI `sha256:0cff08d1...` 的 SHA/版本 tag 分别解析为相同 OCI index digest，三者带 SPDX SBOM 与 SLSA provenance。三个应用及锁定 Ceph RGW 的 HIGH/CRITICAL 扫描为 0 result；7 个 workflow artifacts 为 4 份 Trivy SARIF 与 3 份 Docker build record。 |
| GitHub Deployment `6277018679` / `test` | `success` | 状态依次为 waiting、queued、in_progress、success；`required_migrations=27`、`applied_migrations=27`，PostgreSQL、Ceph RGW、Server、Worker、UI 健康与 smoke 全部通过，最终绑定 `42e7c323...`，未触发自动回滚。 |

### 冻结写路径

- `.github/workflows/ci.yml`
- `.github/scripts/tests/run-repo-policy-tests.sh`
- `.github/scripts/license-inventory.lock.json`（仅允许以仓库既有 `refresh-bindings` 机械刷新一方源码绑定、input tree 与 inventory 摘要）
- `binaries/ficant-server/tests/composition.rs`
- `binaries/ficant-server/tests/data_health_sit.rs`
- `binaries/ficant-server/tests/data_source_registry_sit.rs`
- `binaries/ficant-server/tests/factor_registry_sit.rs`
- `binaries/ficant-server/tests/portfolio_risk_sit.rs`
- `binaries/ficant-server/tests/rates_sit.rs`
- `binaries/ficant-server/tests/service_topology.rs`
- `docs/iterations/2026-09-r9g-linux-release-parity.md`（本文件）
- `docs/iterations/README.md`
- `docs/delivery/release-notes.md`
- `docs/delivery/test-environment.md`
- `docs/quality/evidence.md`
- `README.md`
- `docs/product/scope.md`

实施期精确扩权：最终 `check.ps1` 的第 32 项真实失败关闭于 `pkg:cargo/ficant-server@0.1.0` source binding；临时输出与旧 inventory 的结构化比较证明包集合仍为 649、first-party 集合仍为 20，唯一 package 字段变化是该包的 `source_integrity`，另有机械派生的 `input_tree_digest` 与 `inventory_digest`。因此只把上述 inventory 文件加入写闭集；不得改变包身份、许可证、分类、source locator、third-party 完整性、`supply-chain.lock.json` 或验证代码。

上述闭集之外的源码、业务测试、构建脚本、其他 lock、migration、Dockerfile、发布脚本与私有 authority 均不得修改。

## 7. 残余风险

- `alpha.12` 已提供 GitHub Runner 与测试环境证据，但证据只绑定其不可变 commit/tree；后续源码或文档提交不会自动继承该版本结论。
- Web 真实 gRPC-Web smoke 不访问 Experiment/数据接入 RPC；PostgreSQL、Ceph RGW 与受治理输入的真实链路继续由 migration、business-loop、集成检查及发布 preflight 覆盖。
- GitHub Actions 仍提示部分上游 action 使用即将弃用的 Node 20 runtime；本次由 Runner 强制使用 Node 24 后成功，但后续应跟随中央 CICD 平台升级 action pin。
- 本地 preflight 的 Trivy 0.72.0 与 GitHub release-test 锁定的 Trivy v0.70.0 均不支持 Ceph 基础镜像的 CentOS Stream 9 OS family，现有 0 result 只覆盖其可识别组件；GitHub Deployment 也未提供 `environment_url`。这些限制及测试环境成功均不构成完整业务 UAT 或生产发布授权。
