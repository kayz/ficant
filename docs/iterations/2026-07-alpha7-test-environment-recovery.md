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
- 自动版本交付仍要求新 tag 指向当时的精确 `main`；对既有不可变 tag 的人工重试按 tag 解析原始候选，不要求事后仍等于已前进的 `main`。
- Ceph 的精确 SHA 镜像先由 GitHub Runner 从 GHCR 拉取，再经受控 SSH 通道预热到测试机；目标 Compose、健康检查、烟测和回滚仍只使用相同 SHA 身份。

## 需 Human 决策

无；Human 已明确授权修复测试环境并以 `v0.1.0-alpha.7` 重建。

## 最终真实测试证据

- `scripts/check-fast.ps1`：exit 0；Rust workspace check、非环境回归、存储库单元测试及 Phase 3A/3B 定向测试全部通过。
- `python .github/scripts/tests/test_compose_security_gate.py`：exit 0；共 31 项，29 项通过，2 项仅在显式启用 live fixture 时运行。
- 解析后的正式测试 Compose 输入 `deploy/test/validate_release.py`：exit 0，`release-compose: PASS`。
- `scripts/test-templates.ps1`（中央 `kayz/cicd` 权威源）：exit 0，`Template syntax checks passed.`。
- `configure-object-store.sh` 独立临时根目录测试：exit 0；三项配置原子写入且 `.env` 权限为 `0600`。
- `scripts/check-release-candidate.ps1` 在候选 `6d486b6321d401ca1113a7ec5bd0b7dee6ada80d` 上 exit 0：五个正式镜像构建与 HIGH/CRITICAL 扫描通过，真实 PostgreSQL、Ceph RGW、migration、server、worker、web、ui 完整拓扑全部健康。
- `v0.1.0-alpha.7` 是不可变 annotated tag，剥离后精确指向 `6d486b6321d401ca1113a7ec5bd0b7dee6ada80d`；[版本 CI](https://github.com/kayz/ficant/actions/runs/30094434587) 11/11 jobs 通过。
- 首次发布 run 在目标机直拉 Ceph 的 492.1 MB GHCR 层时卡死，日志显示超过 17 分钟仅下载 12.58 MB；该 run 被明确取消，tag 和已扫描镜像均未移动。
- runner 预热修复经中央 `kayz/cicd` PR #22 和 FICANT PR #28 合并；[同版本恢复 run](https://github.com/kayz/ficant/actions/runs/30097717987) 的五构建、五扫描、晋升与 deploy 全部通过。
- 恢复 run 的部署 checkout、镜像引用和目标状态均为原始候选 `6d486b6321d401ca1113a7ec5bd0b7dee6ada80d`；目标六项服务健康检查通过，smoke 为 `required_migrations=13 applied_migrations=13`。
- 五个 `v0.1.0-alpha.7` 镜像 manifest digest 分别与对应 `sha-6d486b...` manifest 完全一致；[GitHub prerelease](https://github.com/kayz/ficant/releases/tag/v0.1.0-alpha.7) 已发布。

## 残余风险

- Trivy 0.72.0 将 Ceph 基础镜像识别为 CentOS Stream 9，但报告该 OS family 不受支持；语言包扫描无 HIGH/CRITICAL 发现，不能据此声称 Ceph OS 包已获得完整漏洞覆盖。
- 测试机直连 GHCR 下载大型 Ceph 层不可依赖；当前以受控 GitHub Runner SSH 预热解决，长期若引入受信任的上海区域镜像仓库，仍需保持 digest 一致性校验。
