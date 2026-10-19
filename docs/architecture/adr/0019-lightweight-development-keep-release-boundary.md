# ADR-0019：取消 OPAID 开发流程，保留 CICD 发布边界

- 状态：Accepted
- 日期：2026-10-19
- 决策者：Human
- 范围：日常开发的组织方式与交付物
- 取代：ADR-0009 的开发侧部分（OPAID 任务冻结、Root/Worker 编排、每迭代唯一 brief、快速子循环）

## 背景

ADR-0009 规定 OPAID 是从任务冻结到本地自测候选的唯一开发方法：每轮由 Root Orchestrator 冻结验收句、非目标、精确 base、写路径和公共契约，只有确有并行收益时才派出互斥写路径的 Worker，最终交付一份面向 Human 的 brief。

这套流程在 agent 还需要人把任务切细时有用。现在的 agent 能自行拆分任务、管理子 agent 和核对证据，流程本身变成了主要开销：

- 每个迭代的 brief 重复记录 PR 和测试输出里已有的证据；
- 写路径清单、执行边界冻结和扩权记录占用了大量迭代时间；
- R9 连续 7 轮（R9A–R9G）几乎都花在发布身份与 CI 修复上，期间没有推进 v0.1 验收之外的产品能力。

同时，真正守住"什么算完成"的是 authority 仓库中的 SPEC 与 ACCEPTANCE，以及 Human 对每条 AC 的逐条批准，而不是 brief。

## 决策

**开发侧：** 取消 OPAID。agent 自行决定如何拆分任务、是否使用子 agent。保留以下约束，写入 `AGENTS.md`：

1. 判定权威仍是私有 authority 仓库的 SPEC / ACCEPTANCE / MANUAL；
2. 不得通过修改 expected、Oracle、golden case、断言或容差制造通过；
3. 公共契约、migration 与 RulePack 内容的语义变化需要 Human 同意；
4. 完成前运行 `scripts/check-fast.ps1`，风险相关时运行 `scripts/check.ps1`，结论必须绑定真实命令、exit code 和可得的 test count；
5. ACCEPTANCE 条目只由 Human 在 authority 仓库中批准点亮。

交付物从"一份迭代 brief"改为"一个 Pull Request"：PR 描述写改动、对应的 AC 或 SPEC 条款、实际命令与结果、残余风险。不再新增 `docs/iterations/` 下的 brief；已有 brief 保留为历史证据。

**发布侧：** ADR-0009 的 CICD 边界不变。只有 Human 创建的不可变 `v*` tag 触发版本 CI、构建、扫描和测试环境交付；失败的 tag 不移动、不复用，修复后用新的版本号。`check-release-candidate.ps1` 仍是创建 tag 前的本地预检。

## 代价

- 失去每迭代一份汇总阅读材料。Human 改为阅读 PR 描述，以及 authority 仓库中的批准记录。
- 写路径不再事先冻结。越界改动依靠 PR diff 审阅和受保护路径（`.github/**`、`cicd.yml`、`deploy/**`、Oracle 与 expected）发现，而不是依靠事先清单。

## 替代方案

- 保留 OPAID 但简化 brief：拒绝。开销主要来自冻结与编排本身，而不只是 brief 的篇幅。
- 同时取消发布侧边界：拒绝。tag 授权、不可变版本和 forward-only 修复对外交付仍有价值，且发布不频繁，开销可控。

## 后续

若 PR 审阅发现越界改动反复出现，再考虑为特定路径加 CODEOWNERS 或机械检查，而不是恢复整套冻结流程。
