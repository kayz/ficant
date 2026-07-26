# Storage runtime decoupling

## 目标与验收

- 将 Ceph RGW 从应用版本构建、晋升和传输链路中移除，应用版本仅包含 Server、Worker、Web、UI。
- 用受校验的 lock 绑定 Ceph 构建输入、来源提交、镜像名称、OCI index、linux/amd64 manifest、config digest 和压缩层大小。
- Compose、部署状态和回滚使用完整 immutable storage-runtime identity；应用部署只读确认运行时已准备。
- 提供独立 Human 手工触发、幂等、缺失时才流式传输的测试环境准备任务。
- 构建/扫描允许并行，测试部署串行且 `cancel-in-progress: false`。

## 非目标

- 不重建 Ceph，不把 Ceph tar 上传为 Actions artifact，不删除 Actions cache/artifact。
- 不降低漏洞、许可证、供应链、迁移、回滚或健康检查门禁。
- 不创建、移动或复用版本 tag；存储预热不代表应用发布成功。

## 公共契约变化

- 新增 `deploy/storage-runtime.lock.json` 与校验器，绑定 `.dockerignore`、Ceph Dockerfile、entrypoint、来源提交、镜像名、OCI index、linux/amd64 manifest、config digest 和压缩层总量；本地 checkout 换行被规范化，但真实内容漂移失败关闭。
- `cicd.yml` 和中央 `kayz/cicd` 模板只声明四个应用构建；version CI、release preflight 和 release-test 复用锁定 Ceph digest。每个候选仍用最新 Trivy 数据库扫描该 digest。
- Compose、部署状态和回滚分别记录应用 `FICANT_DEPLOY_SHA`、完整 `FICANT_STORAGE_RUNTIME_IMAGE` 与 config digest；部署不再 pull Ceph。Runner 先验证 registry 中 index → amd64 manifest → config 的锁定链，测试机再校验完整 RepoDigest，且本地 image-store identity 只能是锁定 config 或 index，之后才允许 migration。
- 新增独立 `prepare-storage-runtime` 手工任务。它与 deploy 共享 `ficant-test-deploy` 串行锁且不取消在途任务；checkout 获取完整历史以复验 lock 中的来源提交；准确 runtime 已存在时退出，缺失时才经 Runner 流式传输，随后注册并复验准确 digest。
- 正式 version 供应链和漏洞证据明确保留 90 天；非版本候选证据建议 14 天。

## Human 决策

- 已批准复用 `ghcr.io/kayz/ficant-ceph-rgw@sha256:6a86bed20c79fa1df4af6621f4dea0578f82e582a2b476c9f21b8bf555130243`。
- 旧 Actions artifacts 仅盘点，删除需另行批准。

## 最终测试证据

- `python deploy/verify-storage-runtime.py verify-lock ...`：exit 0；当前构建输入与来源提交一致。
- `python deploy/verify-storage-runtime.py verify-remote ...`：exit 0；OCI index、amd64 manifest、config 与压缩层总量 `553966971` 字节一致。
- `python .github/scripts/tests/test_storage_runtime_lock.py`：4/4，覆盖有效 lock、源码漂移、LF/CRLF 和 OCI config 漂移。
- `python .github/scripts/tests/test_compose_security_gate.py`：34 项通过，2 项仅在显式 live 标志下运行而跳过。
- `bash .github/scripts/tests/run-repo-policy-tests.sh`、`bash .github/scripts/verify-repo-policy.sh --stage final`、YAML 解析、全部测试部署 Bash `-n` 与 `git diff --check`：exit 0。
- 中央 `kayz/cicd`：`scripts/test-templates.ps1`、`scripts/validate-config.ps1 -Path ficant/cicd.yml`、YAML/Bash 语法：exit 0；受管副本与业务仓库对应文件规范化后逐字一致。
- 首次手工准备 run `30148673521`：在连接测试机前失败关闭；浅克隆无法解析 lock 的来源提交 `6d486b6321d401ca1113a7ec5bd0b7dee6ada80d`，因此没有检查或改变测试机。工作流与中央模板已 forward-only 修复为 `fetch-depth: 0`，并由合同测试锁定。
- 第二次手工准备 run `30148837450`：来源校验和 SSH 配置通过；测试机可解析锁定 index，但准确 config 校验以 exit 4 失败，传输和加载步骤均跳过。工作流继续失败关闭，并增加 expected/actual 非秘密诊断以区分测试机陈旧状态与运行时表示差异。
- 第三次只读诊断 run `30149062521`：确认测试机 `.Id` 为锁定 OCI index `sha256:6a86…243`，不是第三方未知值；完整 RepoDigest 检查尚未执行即按旧 config-only 规则退出，传输和加载仍跳过。合同已改为兼容 Docker image store 的两种受锁定 `.Id` 表示，同时继续强制完整 RepoDigest 与 registry 端 config 链。
- `scripts/check-fast.ps1`：exit 0。
- `scripts/check.ps1`：第一次在测试前因本机 PowerShell 启动 Buf 输出重定向异常 exit 1；锁定 Buf 1.56.0 独立执行正常，同一候选重跑 exit 0。
- `scripts/check.ps1 -IncludeIntegration`：exit 0；PostgreSQL migration 4/4、Phase 4 lease 1/1、execution 3/3、真实 Worker 1/1、Phase 1 1/1、负向不变量 13/13、Phase 2B/2C/2D 各 1/1、Phase 3A 2/2、Phase 3B codec 2/2 + publication 1/1。
- 最终代码候选 `ce8afd2ce7efa2b22bf306ed07435107741733e6` 上 `scripts/check-release-candidate.ps1`：exit 0。Trivy 0.72.0 使用当日数据库；四个应用镜像完成正式 Dockerfile 构建；Server/Worker/Web 的 Ubuntu 24.04 与 UI 的 Alpine 3.24 扫描均为 HIGH/CRITICAL 0；锁定 Ceph 的 Node/Python 包扫描为 HIGH/CRITICAL 0。release Compose 校验通过，PostgreSQL/Ceph 健康，13 个 migration 应用，Server/Worker/Web/UI 全部健康，最终清理完成。
- 最终 preflight 前四次完整入口因本机 registry 网络 EOF 分别在 Docker Hub Rust base metadata、GHCR manifest（两次）和锁定 Ceph pull 处 exit 1；均未改变候选或门禁。固定 Rust base 与 Ceph digest 分别在重试后准确拉取；第五次从头执行完整入口 exit 0。
- 手工准备 run `30154850075`（代码候选 `ce8afd2ce7efa2b22bf306ed07435107741733e6`）：exit 0；lock 与 registry OCI 链通过；测试机返回准确完整 index RepoDigest，本地 image-store identity 为锁定 index，锁定 config 为 `sha256:3113ea1adb804e958630041c8afaca996e08d3c61d7f3c22cb81c1ddd8b323ab`；`Stream missing storage runtime through the runner` 明确 skipped，准备后再次复验准确 index、amd64 manifest 与 config 绑定。
- 中央 `kayz/cicd` 对应模板最终代码 SHA `ed37492e3ac6884ab9a2ccfe2291282b04b0ae67`；项目 PR `#34`、`#35`、`#36`、`#37` 与中央 PR `#23`、`#24`、`#25`、`#26` 均已合并。未创建、移动或复用版本 tag。

## Actions artifact 只读盘点

当前 234 个 artifacts 合计 `752537258` 字节。四个异常大的旧非版本 supply evidence 均未删除：

| artifact ID | run ID | branch / SHA | size |
|---|---:|---|---:|
| `8441887648` | `29685337397` | `codex/cicd-candidate-topology` / `60ffadbe49d291488a6c6d13d15920ce2ec895eb` | `182323786` |
| `8441888017` | `29685338816` | `codex/cicd-candidate-topology` / `60ffadbe49d291488a6c6d13d15920ce2ec895eb` | `182323786` |
| `8441952078` | `29685558914` | `codex/cicd-candidate-topology` / `37e4ba72a47b62e38a2de6f05095b8c512666d1c` | `182323786` |
| `8441953197` | `29685557741` | `codex/cicd-candidate-topology` / `37e4ba72a47b62e38a2de6f05095b8c512666d1c` | `182323786` |

单独批准删除上述四项预计释放 `729295144` 字节（729.295 MB，695.510 MiB）。

## 残余风险

- Trivy 0.72.0 仍不支持 Ceph 基础镜像的 CentOS Stream 9 OS family；语言包扫描保持执行，但不能冒充完整 OS 包覆盖。
- 测试机首次从旧 SHA tag 迁移到完整 OCI index identity 时可能需要一次约 554 MB 压缩层的流式准备；之后准确 digest 命中即不重复传输。
- `docker load` 本身不保留纯 digest archive 的 RepoDigest，因此任务用独立 storage-runtime tag 承载流，再用短期 GHCR 凭据注册准确 digest 并复验；若测试机连 GHCR 连 manifest 请求也不可达，准备会失败关闭。
