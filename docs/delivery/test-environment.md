# GitHub test 环境运行合同

## 范围

本合同覆盖 `ficant` 的第一个自动化测试发布：GitHub main CI 成功、GHCR 不可变镜像、Linux 测试机 Docker Compose、PostgreSQL migration、健康检查、冒烟测试、部署记录和镜像回滚。

它不授权生产发布，不宣称完整业务 UAT，也不启用任何对象存储 adapter。

## 触发与制品

- 自动触发：默认分支 `main` 的 `ci` workflow 因 push 成功结束。
- 手动触发：`release-test` workflow dispatch，只允许 main 可达的精确 Commit。
- 镜像：`ghcr.io/kayz/ficant-{server,worker,web}:sha-<40位CommitSHA>`。
- 可变 `test-latest` 仅用于人类浏览；Compose 必须使用 SHA 标签。
- Dockerfile、Rust 工具链和基础镜像沿用仓库锁定版本；构建和 SBOM/provenance 只发生在 GitHub Linux Runner。

## 测试机

- 专用账号：`ficant-deploy`，无密码、无 sudo、只用于该项目部署。
- 根目录：`/srv/ficant-test`。
- 回环端口：PostgreSQL `25432`、server `28080`、worker `28081`、web `28082`。
- 测试机不得执行 `git pull`、`cargo build`、`npm install`、`cmake --build` 或其他现场构建。
- Docker 日志使用 `10m × 3` 轮转限制。

## 部署事务

1. 校验 40 位 Commit SHA 和对应 migration 目录。
2. 使用工作流短期 `GITHUB_TOKEN` 登录 GHCR，拉取三个 SHA 镜像。
3. 启动固定 digest 的 PostgreSQL，串行执行版本化 migration。
4. 启动三个应用服务并等待容器健康。
5. 验证 server TCP、worker/web readiness 和 migration 计数。
6. 成功后原子更新 `state/current.env`，并把旧 current 写入 `state/previous.env`。
7. 写入 `state/deployments/<sha>.json`。

失败时，若 previous 存在，则直接恢复 previous 镜像并重复健康/冒烟；首次部署失败则停止应用容器、保留数据库卷和诊断记录，不伪造成功状态。

## 数据库回滚边界

代码回滚不自动执行 down migration。migration 必须遵循扩展—兼容—收缩；破坏性 schema 变更需要独立人工审批和备份/恢复方案。当前八个 migration 从空库向前执行。

## 安全边界与已知代价

- SSH 私钥和 known_hosts 只存于 GitHub `test` Environment Secrets。
- 应用 Secret 只保存在测试机 `/srv/ficant-test/.env`，权限为 `0600`。
- 服务为非 root、只读根文件系统、`cap_drop=ALL`、`no-new-privileges`，并只发布到 `127.0.0.1`。
- 当前主机 Docker 为 rootful；`ficant-deploy` 的 Docker group 成员资格具有较高主机权限。它比直接 root SSH 缩小了日常账号范围，但不等价于真正的 rootless 隔离。若测试机承载更多不互信项目，应迁移到 rootless Docker、独立 VM 或受限部署代理。
- 公开仓库启用 main branch protection；GitHub `test` Environment 的 required reviewer 仍应按当前账户套餐能力配置，不能配置时由工作流授权门与人工合并流程兜底。

