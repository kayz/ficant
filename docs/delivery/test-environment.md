# GitHub test 环境运行合同

## 范围

本合同覆盖 `ficant` 的自动化版本测试交付：Human 版本 tag 授权、GitHub 版本 CI、GHCR 不可变镜像、Linux 测试机 Docker Compose、PostgreSQL migration、Ceph RGW 对象存储、健康检查、冒烟测试、部署记录和镜像回滚。

它不授权生产发布，也不宣称完整业务 UAT。测试环境启用与应用相同的 S3 adapter，但使用测试专用 Ceph RGW 和凭据。

## 触发与制品

- 自动触发：Human 创建符合版本格式、指向当前 `main` 精确提交的不可变 `v*` tag；普通 branch push、Pull Request 和 `main` 合并不运行完整 GitHub CI。
- 手动重试：`release-test` workflow dispatch 只接受已存在的不可变版本 tag，并重新解析该 tag 的原始候选 Commit；不接受裸 Commit SHA，也不要求历史 tag 事后仍等于已经前进的 `main`。
- 应用镜像：只构建 `ghcr.io/kayz/ficant-{server,worker,web,ui}:sha-<40位CommitSHA>`；四个镜像和锁定的 Ceph runtime 全部扫描通过后，只把四个应用镜像提升为对应的不可变 `:<version>` tag。
- Compose 的应用服务使用 `FICANT_DEPLOY_SHA`；Ceph 存储运行时使用 `deploy/storage-runtime.lock.json` 中的完整 OCI index digest 引用，并另外校验 config digest。不得创建或更新可变 `latest`/`test-latest`。
- Dockerfile、Rust 工具链和基础镜像沿用仓库锁定版本；构建和 SBOM/provenance 只发生在 GitHub Linux Runner。

## 测试机

- 专用账号：`ficant-deploy`，无密码、无 sudo、只用于该项目部署。
- 根目录：`/srv/ficant-test`。
- 回环端口：PostgreSQL `25432`、server `28080`、worker `28081`、web `28082`。
- Ceph RGW 只在 Compose 内部网络提供 S3 endpoint，不直接发布公网端口；数据存放在持久化 `ceph-data` volume。
- 测试机不得执行 `git pull`、`cargo build`、`npm install`、`cmake --build` 或其他现场构建。
- Docker 日志使用 `10m × 3` 轮转限制。

## 部署事务

1. 自动新版本校验 tag、其 40 位 Commit SHA 与触发时的当前 `main`；人工重试校验既有不可变 tag 并恢复其原始 Commit，同时校验对应 migration 目录。
2. 原子写入测试专用 S3 access key、secret key 与 bucket，`.env` 权限保持 `0600`。
3. Human 按需手工触发 `prepare-storage-runtime`。任务先在测试机只读检查 lock 中准确的 index/config digest；已存在即成功退出，缺失时才由 Runner 使用 `docker save | gzip | ssh | docker load` 流式准备，并在加载后复验。它不上传 tar artifact、不使用应用版本 tag，也不表示应用发布成功。
4. 应用部署使用工作流短期 `GITHUB_TOKEN` 登录 GHCR，只按 SHA 拉取 PostgreSQL 和四个应用镜像；部署前只读确认锁定的 Ceph runtime 已准备，目标机不现场构建。
5. 启动固定 digest 的 PostgreSQL 与锁定 OCI index 的 Ceph RGW，串行执行版本化 migration。
6. 启动 Server、Worker、Web 和 UI，等待全部六项服务健康；Worker 必须使用真实 PostgreSQL、S3 和身份配置。
7. 验证容器健康、服务 readiness，以及数据库已应用 migration 是候选所需 migration 的超集且无缺项。
8. 成功后原子更新 `state/current.env` 中的 `FICANT_DEPLOY_SHA`、`FICANT_STORAGE_RUNTIME_IMAGE` 与 `FICANT_STORAGE_RUNTIME_CONFIG_DIGEST`，并把旧状态写入 `state/previous.env`。
9. 写入 `state/deployments/<sha>.json`。

失败时，若 previous 存在，则直接恢复 previous 镜像并重复健康/冒烟；首次部署失败则停止应用容器、保留数据库卷和诊断记录，不伪造成功状态。

## 数据库回滚边界

代码回滚不自动执行 down migration。migration 必须遵循扩展—兼容—收缩；破坏性 schema 变更需要独立人工审批和备份/恢复方案。当前 13 个 migration 从空库向前执行；回滚烟测检查旧版本要求的 migration 不得缺失，不要求数据库恰好只有旧版本数量。

## 安全边界与已知代价

- SSH 私钥和 known_hosts 只存于 GitHub `test` Environment Secrets。
- 应用构建和漏洞扫描可并行；只有 `ficant-test-deploy` 部署段串行，并使用 `cancel-in-progress: false`，防止 migration 或回滚被新运行取消。
- 应用 Secret 只保存在测试机 `/srv/ficant-test/.env`，权限为 `0600`。
- 服务为非 root、只读根文件系统、`cap_drop=ALL`、`no-new-privileges`，并只发布到 `127.0.0.1`。
- Trivy 0.72.0 不支持 Ceph 基础镜像的 CentOS Stream 9 OS family；语言包 HIGH/CRITICAL 扫描仍执行，但这不等价于完整的 Ceph OS 包漏洞覆盖。
- 测试机直连 GHCR 下载大型 Ceph 层已证实不稳定，当前受控 runner 预热是正式恢复路径；若未来使用区域镜像仓库，必须校验源与目标 manifest digest 完全一致。
- 当前主机 Docker 为 rootful；`ficant-deploy` 的 Docker group 成员资格具有较高主机权限。它比直接 root SSH 缩小了日常账号范围，但不等价于真正的 rootless 隔离。若测试机承载更多不互信项目，应迁移到 rootless Docker、独立 VM 或受限部署代理。
- 公开仓库启用 main branch protection，保留 Pull Request、线性历史、conversation resolution、禁止 force-push 和 deletion；普通迭代不要求 GitHub status checks。版本 tag 的创建是测试交付授权，不替代生产环境的独立 Human 审批。

