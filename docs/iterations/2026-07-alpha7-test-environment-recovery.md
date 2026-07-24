# alpha.7 测试环境恢复

## 目标

修复 `v0.1.0-alpha.6` 已定位的测试环境发布缺口，以真实 PostgreSQL、Ceph RGW、Server、Worker、Web 和 UI 完成版本候选构建、扫描、迁移、部署及烟测；在创建版本 tag 前增加与正式发布镜像和拓扑一致的本地预检。

## 验收

- 测试 Compose 启动真实 Worker 和 Ceph RGW，Worker 使用显式数据库、S3 和身份配置。
- 五个发布镜像均由 GitHub Runner 构建、扫描并以 Commit SHA 标识。
- forward-only 数据库在回滚时允许已应用迁移是旧版本所需迁移的超集，但不得缺少旧版本要求的迁移。
- 从首次包含 Ceph 的版本回滚到不包含 Ceph 镜像的旧应用 SHA 时，仍使用已发布的 Ceph 存储运行时镜像。
- 本地预检在 tag 前完成五镜像构建、漏洞扫描、完整拓扑启动和迁移兼容性检查。
- `v0.1.0-alpha.7` 指向合并后的精确 `main`，版本 CI、镜像发布和测试环境部署成功。

## 非目标

- 不移动、删除或复用失败的 `alpha.4`、`alpha.5`、`alpha.6` tag。
- 不改变业务领域语义、量化算法、数据库迁移内容或生产环境。
- 不把普通 branch、Pull Request 或 `main` 合并改为完整发布触发器。

## 公共契约变化

- 测试环境新增 `ficant-ceph-rgw` 发布镜像和持久化 `ceph-data`。
- 测试环境新增保密配置 `FICANT_S3_ACCESS_KEY`、`FICANT_S3_SECRET_KEY` 和 `FICANT_S3_BUCKET`。
- 部署状态同时记录应用 `FICANT_DEPLOY_SHA` 与存储运行时 `FICANT_STORAGE_SHA`，用于跨旧版本安全回滚。
- 新增 `scripts/check-release-candidate.ps1` 作为创建版本 tag 前的本地只读发布候选预检入口。

## 需 Human 决策

无；Human 已明确授权修复测试环境并以 `v0.1.0-alpha.7` 重建。

## 最终真实测试证据

- `scripts/check-fast.ps1`：exit 0；Rust workspace check、非环境回归、存储库单元测试及 Phase 3A/3B 定向测试全部通过。
- `python .github/scripts/tests/test_compose_security_gate.py`：exit 0；30 项通过，2 项仅在显式启用 live fixture 时运行。
- 解析后的正式测试 Compose 输入 `deploy/test/validate_release.py`：exit 0，`release-compose: PASS`。
- `scripts/test-templates.ps1`（中央 `kayz/cicd` 权威源）：exit 0，`Template syntax checks passed.`。
- `configure-object-store.sh` 独立临时根目录测试：exit 0；三项配置原子写入且 `.env` 权限为 `0600`。
- tag 前的五镜像构建、Trivy 扫描和完整本地拓扑预检，以及 tag 后的 GitHub CI/CD 与测试环境烟测，将在精确合并提交上补录。

## 残余风险

- 当前尚未创建 `v0.1.0-alpha.7`；正式发布候选预检和远端测试环境结果仍是版本交付退出条件。
- 首次 Ceph 数据卷在测试服务器上的真实初始化只能由获授权的版本部署验证；失败时自动回滚应用 SHA，并保留候选的存储运行时 SHA。
