# 2026-07 OPAID 治理收敛

## 目标

- 将已停用的 HOQA 资产归档为只读历史。
- 以 PowerShell 7 统一本地快速检查、完整检查和可选集成检查入口。
- 保持 OPAID 本地自测候选与中央 CICD 发布管理的职责边界。
- 将迭代治理收敛为一份 Human brief 和多个不产生治理文档的快速子循环。
- 修正产品范围与架构字典中已经被 Phase 2A 实现取代的状态描述。

## 验收

- 精确候选上的 `.\scripts\check-fast.ps1` 退出码为 0。
- 同一精确候选上的 `.\scripts\check.ps1` 退出码为 0。
- `docs/product/scope.md` 与 `docs/architecture/data-dictionary.md` 准确区分已交付 Phase 2A 和尚未实现能力。
- 推送后的远端 `codex/opaid-reorganization` 与最终已验证的本地 commit 相同；精确身份由 Git 与 Pull Request 事实源绑定。

## 非目标

- 不改变业务行为、产品路线边界或运行时语义；产品文档只修正已经确认的状态漂移。
- 不改变公共 API、Protobuf、数据库 migration 或 Artifact 格式。
- 不修改 Oracle、expected、断言或容差。
- 不以本地治理收敛替代 CI/CD、部署、UAT、发布或回滚。

## 公共契约变化

- 产品、业务、API、Oracle、expected 和容差均无变化。
- 产品范围与架构字典改为如实记录已合并的 Phase 2A 能力，不把它扩写为完整 Phase 2 或 Phase 3+。
- 本地开发合同新增单一迭代 brief、快速子循环和 forward-only checkpoint 规则；子循环不得改变 Human 验收。

## 需 Human 决策

- 无；Human 已授权现在保持本地与 GitHub 状态一致。完整本地门禁若受当前机器的离线缓存阻塞，远端只作为 Draft PR 检查点，不冒充已完成的 OPAID 候选。没有待决的业务、API 或 Oracle 语义变化。

## 最终真实测试证据

- `.\scripts\check-fast.ps1`：exit 0；174 项测试通过，0 failed、0 ignored。
- `.\scripts\check.ps1`：exit 0；锁定工具链下 Rust strict Clippy、workspace build、非环境测试、storage library 与 generated-contract tests 全部通过；C++ 4/4、Q-001..Q-036 为 36 mapped / 0 missing、Python 1/1、Web 4 files / 29 tests，并完成 production build。
- `.github/scripts/tests/run-gates-tests.sh`：exit 0；固定供应链、许可证、风险接受和负向 fixture 全部符合预期。
- `.github/scripts/tests/run-repo-policy-tests.sh` 与 `.github/scripts/verify-repo-policy.sh --stage final`：均 exit 0；`git diff --check`：exit 0。
- 未运行 `.\scripts\check.ps1 -IncludeIntegration`：本轮不改变业务或存储运行时，且未把共享环境作为本地治理检查的替代品。

## 残余风险

- 供应链拓扑锁定 `main` 精确 base、两项线性 forward-only commit 和无 merge 历史；GitHub 仍须在推送后的同一精确候选上重新运行 required checks，远端结果不得由本地证据代替。
- 对象存储运行时继续 fail-closed；本轮不迁移 `minio` client 或 server。该风险只能作为一次性本地/CI fixture 的受控缓冲，必须在共享 SIT、运行时装配、真实数据接入或 2026-10-13（最早者）之前另行关闭或由 Human 重新接受。
- 本轮不重新验证 CI/CD、部署或目标环境；这些结果仍由中央 CICD 合同独立产生。
