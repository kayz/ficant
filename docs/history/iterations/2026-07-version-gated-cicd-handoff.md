# 版本门控的 OPAID/CICD 交接

## 目标

- OPAID 在精确本地自测候选完成后，以本迭代唯一 brief 向 Human 交付；该交付不自动进入 GitHub CI/CD。
- Human 可以要求同一候选执行本地完整验收与人工复测，或接受已有证据并合并、进入下一次 OPAID 循环。
- 普通分支 push、Pull Request 和 `main` 合并不运行 GitHub 完整 CI，也不构建镜像或部署。
- Human 创建指向 `main` 精确提交的 `v*` 版本 tag 后，自动运行完整版本 CI；成功后自动按版本和 SHA 构建、扫描并交付测试环境。
- OPAID、CICD skill 的源码、Codex 加载副本与 FICANT 项目入口表达同一边界。

## 验收

- `.github/workflows/ci.yml` 只接受 `v*` tag push，不接受普通 branch push、Pull Request 或 `main` push。
- 版本 CI 成功后，`release-test` 只接受触发该 CI 的精确 `v*` tag；手动重试也必须输入已存在且位于 `main` 的版本 tag。
- 镜像先只发布 SHA tag；全部镜像扫描成功后才统一提升不可变版本 tag，不创建或更新 `latest`/`test-latest`，避免部分成功污染共享标签。
- `main` 分支保护保留线性历史、禁止 force-push/deletion 和 conversation resolution，不再要求每个迭代 PR 的十项 GitHub status checks。
- FICANT 与中央 `cicd` 的配置、受管 workflow 和文档一致；相关 PowerShell、JSON、YAML/策略 fixture 与 skill 校验均通过。
- 不创建版本 tag，不触发版本 CI，不构建或部署任何版本。

## 非目标

- 不改变业务代码、公共 API、Protobuf、数据库 migration、数值 Oracle、expected、断言或容差。
- 不修复 `41ac267` 自动发布中暴露的 `ficant-server` 镜像构建失败；本轮只取消普通合并触发发布的入口。
- 不设计生产环境发布、签名或 UAT 流程；测试环境仍部署精确 SHA，生产发布继续需要独立 Human 决策。
- 不改写历史 tag 或现有 GHCR 制品，不创建新的治理 ledger、checklist 或子任务文档。

## 公共契约变化

- GitHub 完整 CI 的入口从任意 `push`/`pull_request` 改为 Human 创建的 `v*` 版本 tag。
- 普通迭代合并是源码历史集成，不再等价于 CICD 或测试环境交付。
- CICD 接受的身份从裸 `main` SHA 改为 `version tag + exact main SHA`；失败版本使用新的 forward-only 版本候选 tag，不移动旧 tag。
- 本地“完整检查”仍属于 OPAID/Human 验收，不称为 GitHub CI，也不替代版本 CI 的 Linux、供应链、制品和环境证据。

## 需 Human 决策

- 已决定：保留自动工作线，但只由版本更新触发；创建版本 tag 即授权完整 CI、镜像构建、扫描和测试环境交付。
- 已决定：普通迭代经 brief 验收后仍合并到 GitHub，但不触发完整 GitHub CI。
- 无待决业务语义。若 GitHub 无法把 tag 触发的 CI 与后续发布绑定到同一精确 tag/SHA，停止并返回 Human，不回退到 `main-ci-success`。

## 最终真实测试证据

- `bash .github/scripts/tests/run-repo-policy-tests.sh`：exit code 0；中文、CI 与 5 组路径策略检查均 PASS，最终输出 `repo-policy-tests: PASS`。
- `pwsh -NoProfile -File .\scripts\test-templates.ps1`（中央 `cicd`）：exit code 0，`Template syntax checks passed.`。
- `pwsh -NoProfile -File .\scripts\validate-config.ps1 -Path .\ficant\cicd.yml`（中央 `cicd`）：exit code 0，配置校验通过。
- `quick_validate.py` 分别校验 OPAID/CICD 的源码与 Codex 安装副本：4 次均 exit code 0；源码与安装副本 SHA-256 分别一致。
- 中央受管 `release-test.yml` 与本仓库生成副本 SHA-256 一致；中央 `ficant/cicd.yml` 与本仓库生成副本 SHA-256 一致。
- Python `yaml.safe_load` 解析 `.github/workflows/ci.yml` 与 `.github/workflows/release-test.yml`：exit code 0，`workflow-yaml: PASS`。
- `bash .github/scripts/verify-repo-policy.sh --stage final`：exit code 0，`repo-policy (final): PASS`。
- `.\scripts\check-fast.ps1 -ListOnly`：exit code 0，列出 6 条本地检查命令；随后执行 `.\scripts\check-fast.ps1`：exit code 1，在第 1 条 `cargo fmt --all -- --check` 失败。原因是基线已收录的 `crates/vendor/parquet-59.1.0/Cargo.toml` 声明了仓库中不存在的 benches/examples/tests 路径；本迭代未修改 Rust、Cargo 或 vendor，不把这条既有失败写成通过。

## 残余风险

- 降低远端 CI 频率会延后发现只在 Linux clean runner 或在线 advisory 数据库出现的问题；由版本候选频率、本地容器化完整验收和 Human 对高风险迭代选择额外本地复测来控制。
- GitHub tag 创建是有外部副作用的发布授权动作；任何 Agent 未经 Human 明确版本决定不得创建、移动或删除版本 tag。
- 当前完整本地入口受 vendored Parquet 清单引用缺失源码阻塞；它不影响本次 workflow/策略合同的针对性验证，但在下一次需要把 `check-fast` 作为验收门之前必须单独修复。
