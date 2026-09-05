# GitHub test 环境运行合同

## 范围

本合同覆盖 `ficant` 的自动化版本测试交付：Human 版本 tag 授权、GitHub 版本 CI、GHCR 不可变镜像、Linux 测试机 Docker Compose、PostgreSQL migration、Ceph RGW 对象存储、健康检查、冒烟测试、部署记录和镜像回滚。

它不授权生产发布，也不宣称完整业务 UAT。测试环境启用与应用相同的 S3 adapter，但使用测试专用 Ceph RGW 和凭据。

## 触发与制品

- 自动触发：Human 创建符合版本格式、指向当前 `main` 精确提交的不可变 `v*` tag；普通 branch push、Pull Request 和 `main` 合并不运行完整 GitHub CI。
- 手动重试：`release-test` workflow dispatch 只接受已存在的不可变版本 tag，并重新解析该 tag 的原始候选 Commit；不接受裸 Commit SHA，也不要求历史 tag 事后仍等于已经前进的 `main`。
- 应用镜像：只构建 `ghcr.io/kayz/ficant-{server,worker,ui}:sha-<40位CommitSHA>`；三个应用镜像和锁定的 Ceph runtime 全部扫描通过后，只把三个应用镜像提升为对应的不可变 `:<version>` tag。
- Compose 的应用服务由同一对 40 位小写 commit/tree 身份驱动：`FICANT_CODE_COMMIT_SHA` 唯一派生自 `FICANT_DEPLOY_SHA`，`FICANT_CODE_TREE_SHA` 由已验证 tag 候选提供。Server/Worker 的实际镜像 config digest 在镜像拉取后只读派生，Worker 的 native source digest 继续从实际 Worker 镜像派生；Ceph 存储运行时使用 `deploy/storage-runtime.lock.json` 中的完整 OCI index digest 引用，并另外校验 config digest。不得创建或更新可变 `latest`/`test-latest`。
- Dockerfile、Rust 工具链和基础镜像沿用仓库锁定版本；版本制品构建及其 SBOM/provenance 生成只发生在 GitHub Linux Runner，本地入口只构建不发布的预检镜像。

## 测试机

- 专用账号：`ficant-deploy`，无密码、无 sudo、只用于该项目部署。
- 根目录：`/srv/ficant-test`。
- 回环端口：PostgreSQL `25432`、Ceph RGW `29000`、server `28080`、worker `28081`、UI `28083`。
- Ceph RGW 同时在 Compose 内部网络提供 S3 endpoint，并只向宿主机回环地址 `127.0.0.1:29000` 发布；不暴露公网端口，数据存放在持久化 `ceph-data` volume。
- 测试机不得执行 `git pull`、`cargo build`、`npm install`、`cmake --build` 或其他现场构建。
- Docker 日志使用 `10m × 3` 轮转限制。

## 部署事务

1. 自动新版本校验 tag、其 40 位 Commit SHA、对应 40 位 tree SHA 与触发时的当前 `main`；人工重试校验既有不可变 tag 并从 tag commit/tree 恢复同一候选，同时校验对应 migration 目录。
2. 原子写入测试专用 S3 access key、secret key 与 bucket，`.env` 权限保持 `0600`。
3. Human 按需手工触发 `prepare-storage-runtime`。任务先在测试机只读检查 lock 中准确的 index/config digest；已存在即成功退出，缺失时才由 Runner 使用 `docker save | gzip | ssh | docker load` 流式准备，并在加载后复验。它不上传 tar artifact、不使用应用版本 tag，也不表示应用发布成功。
4. 应用部署使用工作流短期 `GITHUB_TOKEN` 登录 GHCR，并以 `deploy.sh <commit> <tree>` 显式传入已授权 Code 身份；拉取固定 digest 的 PostgreSQL 镜像，以及按候选 Commit SHA 标识的 Server、Worker、UI 三个应用镜像。拉取后只读派生 Server/Worker 实际镜像 config digest 与 Worker native source digest；部署前只读确认锁定的 Ceph runtime 已准备，目标机不现场构建。
5. 启动固定 digest 的 PostgreSQL 与锁定 OCI index 的 Ceph RGW，串行执行版本化 migration。
6. 启动 Server、Worker 和 UI，等待 PostgreSQL、Ceph RGW 与三个应用共五项长期运行服务健康；一次性 migration 必须已成功退出。Server 必须接收候选 commit/tree、实际 Server runtime digest、固定测试环境摘要、受控 bootstrap bearer/tenant/actor/owner/role 以及文件/PostgreSQL 输入绑定；Worker 必须接收同一 Code 身份、实际 runtime/source 身份、真实 PostgreSQL/S3 配置和固定 orphan grace/interval。
7. 验证容器健康、服务 readiness，以及数据库已应用 migration 是候选所需 migration 的超集且无缺项。
8. 成功后原子更新 `state/current.env` 中的 deploy commit、Code tree、storage image/config、Server runtime、Worker runtime/source，并把旧状态完整写入 `state/previous.env`。新候选必须保存真实 tree/runtime；只有回滚缺少这些字段的旧状态时，才允许通过显式 legacy 模式使用零 tree/零 runtime 兼容占位。
9. 写入 `state/deployments/<sha>.json`。

失败时，若 previous 存在，则按其已保存的 commit/tree 恢复 previous 镜像、重新派生实际 Runtime 身份并重复健康/冒烟；缺少 R9E 新字段的旧状态只走显式 legacy 回滚兼容路径。首次部署失败则停止应用容器、保留数据库卷和诊断记录，不伪造成功状态。

## 数据库回滚边界

代码回滚不自动执行 down migration。migration 必须遵循扩展—兼容—收缩；破坏性 schema 变更需要独立人工审批和备份/恢复方案。候选所含全部 migration 必须从空库按文件名顺序向前执行；回滚烟测检查旧版本要求的 migration 不得缺失，不要求数据库恰好只有旧版本数量。

## 安全边界与已知代价

- SSH 私钥和 known_hosts 只存于 GitHub `test` Environment Secrets。
- 应用构建和漏洞扫描可并行；只有 `ficant-test-deploy` 部署段串行，并使用 `cancel-in-progress: false`，防止 migration 或回滚被新运行取消。
- 应用 Secret 只保存在测试机 `/srv/ficant-test/.env`，权限为 `0600`。
- 服务为非 root、只读根文件系统、`cap_drop=ALL`、`no-new-privileges`，并只发布到 `127.0.0.1`。
- 本地 preflight 使用的 Trivy 0.72.0 与 GitHub release-test 锁定的 Trivy v0.70.0 均不支持 Ceph 基础镜像的 CentOS Stream 9 OS family；语言包 HIGH/CRITICAL 扫描仍执行，但这不等价于完整的 Ceph OS 包漏洞覆盖。
- 测试机直连 GHCR 下载大型 Ceph 层已证实不稳定，当前受控 runner 预热是正式恢复路径；若未来使用区域镜像仓库，必须校验源与目标 manifest digest 完全一致。
- 当前主机 Docker 为 rootful；`ficant-deploy` 的 Docker group 成员资格具有较高主机权限。它比直接 root SSH 缩小了日常账号范围，但不等价于真正的 rootless 隔离。若测试机承载更多不互信项目，应迁移到 rootless Docker、独立 VM 或受限部署代理。
- 公开仓库启用 main branch protection，保留 Pull Request、线性历史、conversation resolution、禁止 force-push 和 deletion；普通迭代不要求 GitHub status checks。版本 tag 的创建是测试交付授权，不替代生产环境的独立 Human 审批。
- **R9A repo-policy 收口（2026-09-04）：** `.github/scripts/tests/test_compose_security_gate.py -v` 共 34 项，结果为 32 passed、2 skipped、0 failed；两个 skipped 是必须显式启用的 Ceph image / live Compose 环境测试。夹具现在验证开发 Compose 的两个精确 origin 与 `Get-ImageConfigDigest` / `Get-WorkerAttestation` 行为边界，不再依赖已失效的单 origin 或内联命令字符串。
- **R9A 状态发布收口：** `deploy/test/bin/deploy.sh` 对 `current.env` / `previous.env` 使用目标同目录临时文件，完整写入、设为 `0600` 后通过 rename 原子发布；4 个专用夹具覆盖成功、rename 失败、旧状态保持、临时文件清理和版本并发合同。
- **R9A 并发合同收口：** `cicd.yml` 的 `cancel_outdated_runs` 已与 `.github/workflows/ci.yml`、`release-test.yml` 的 `cancel-in-progress: false` 对齐。不可变版本的已开始 CI/部署不会被后续运行自动取消。
- **`v0.1.0-alpha.12` 实际交付（2026-09-05）：** Human 授权的不可变 tag 精确绑定 `42e7c323543401567f8d76166d4d13958dd55f7b`（tree `db9123eabc27d8b1d5c52dd001d57c6f5934dd32`）；[版本 CI](https://github.com/kayz/ficant/actions/runs/33943037740) 11/11 jobs 成功，[release-test](https://github.com/kayz/ficant/actions/runs/33943924358) 10/10 jobs 成功。三个应用的 SHA tag 与版本 tag 解析为相同的不可变 OCI index digest，并带 SPDX SBOM 与 SLSA provenance；Server、Worker、UI 和锁定 Ceph RGW 的 HIGH/CRITICAL 扫描均为 0 result。GitHub Deployment `6277018679` 在 `test` Environment 成功，27/27 个 migration 已应用，PostgreSQL、Ceph RGW、Server、Worker、UI 健康及 smoke 均通过，未触发自动回滚。`v0.1.0-alpha.11` 保持原位作为失败且未部署的历史候选。该结果只证明 `alpha.12` 的测试环境交付，不授权生产发布或完整业务 UAT；GitHub Deployment 未设置 `environment_url`，Ceph CentOS Stream 9 OS 包扫描限制仍然存在。详见 [R9G brief](../iterations/2026-09-r9g-linux-release-parity.md) 与 [发布说明](release-notes.md)。

