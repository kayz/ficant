# PROQAID → HOQA 迁移映射

本文件记录 2026-07-15 治理迁移的语义承接关系。活动权威为 `.hoqa/state.toml`、
`iteration-3-checklist.md`、README 和已接受 ADR；本文件只解释迁移，不形成第二套状态。

| 原内容 | HOQA 承接位置 | 处理方式 |
|---|---|---|
| `.proqaid/product/charter.md` | `.hoqa/state.toml` 的 `project` 与 Orchestrator Product lens | 转换；产品事实保留，角色门退役 |
| `.proqaid/architecture/charter.md` | `.hoqa/state.toml`、ADR-0008 与既有 ADR | 转换；架构责任变为 Orchestrator lens |
| `.proqaid/interface/charter.md` | `.hoqa/state.toml` 与 Orchestrator Interface lens | 转换；接口责任保留，角色门退役 |
| `.proqaid/delivery/charter.md` | `.hoqa/state.toml` 的 environment/operate 与 Human Operator 边界 | 转换；Delivery 变为 Orchestrator 工作，不再是参与者 |
| `.proqaid/orchestrator/charter.md` | `.hoqa/SKILL.md`、`.hoqa/state.toml` | 保留计划、Development Worker、集成、验证、交付与关闭责任；退役 PROQAID 状态机和七角色调度 |
| `.proqaid/quality/charter.md` | `.hoqa/SKILL.md`、`participants.quality`、testing | 转换；Quality 领导自动化测试和 Test Workers，输出 test report/bug list，不批准业务或监督 Development Workers |
| `.proqaid/review/charter.md` | `.hoqa/SKILL.md`、`participants.audit` | 转换为最终文档一致性 Audit；退役 Design Freeze/过程 Review |
| `.proqaid/archive/iteration-1/**` | `.hoqa/state.toml` iteration-1 evidence；原文位于 `.hoqa/history/proqaid-superseded/archive/iteration-1/**` | 保留关闭事实、SHA、证据和当时限制；全部标记 historical/superseded |
| `.proqaid/archive/iteration-2/**` | `.hoqa/state.toml` iteration-2 evidence/risk；原文位于 `.hoqa/history/proqaid-superseded/archive/iteration-2/**` | 保留 Phase 0/1 完成事实、CI、供应链和风险；旧角色、WSL 和尝试顺序只作历史 |
| `.agents/skills/proqaid/**` | `.hoqa/SKILL.md` 与 `.hoqa/references/contracts.md` | 原包移入 `.hoqa/history/proqaid-superseded/project-skill/`；活动 skill 完全替换为 HOQA |
| `.codex/AGENTS.md`、`.claude/CLAUDE.md` | 同路径 | 改写为 HOQA 单一治理入口并引用 `.hoqa` |
| `iteration-3-checklist.md` | 同路径 + `.hoqa/state.toml` | 从 Governed/七角色/Review 门改为 HOQA Align/Decide/Execute/Test/Operate/Close；实现保持暂停 |
| ADR-0004 的 Quality 批准权 | ADR-0004 + ADR-0008 | expected/Oracle/容差候选由 Quality 设计测试，业务含义和风险由 Human 接受；禁止 Worker 自改仍保留 |
| ADR-0007 的默认 Worker、Delivery 角色 | ADR-0007 + ADR-0008 | Worker 只用于有收益的并行任务；环境/发布为 Orchestrator delivery work，特权操作由 Human 执行 |
| `deploy/execution/profiles.toml` | 同路径 | 四参与者、Worker 所有权、Quality/Audit 分离权限、Human/Orchestrator 操作责任 |
| runner contract/result schema 与 Windows tests | 同路径 | `quality-review` 拆为 `quality`、`audit`；确定性权限边界保留 |
| WSL runner 与其七角色配置 | ADR-0007/0008 与文件历史标记 | 保留兼容来源但明确 superseded/historical，禁止作为普通活动入口 |
| `docs/quality/evidence.md`、`docs/review/audit-summary.md` 的 PROQAID 文本 | 原文件 | 保留 iteration-2 历史事实并加 superseded/historical 说明，不改写成当前 HOQA verdict |
| repo-policy 中 `.proqaid` deny-list | `.github/scripts/verify-repo-policy.sh` 与测试 | 保留 deny-list，防止旧治理目录重新进入；新增 `.hoqa` allowlist 与完整性检查 |
| supply-chain `Architecture/Delivery` 和 `final-review` | supply-chain lock/validator | 转换为 `Human/Orchestrator` 风险责任和 final consistency Audit；安全事实不变 |
| `.agents/logs/**` 与 `.agents/tasks/**` | 原地历史资料；`.hoqa/state.toml` 不引用其过程状态 | 保留但停止作为活动治理日记或权威；不得用其覆盖候选/测试确定性证据 |

## 历史冲突的承接结论

- iteration-1 的“尚无生产实现”仅描述当时状态，已被 iteration-2 Phase 0/1 完成事实取代。
- iteration-2 audit-report 中较早的阻塞 finding 被后续关闭记录取代；最终关闭事实以
  `80f48706f37e3890224ca106fb763213d0beeb38`、tree
  `9dd6a136d453872dc37085bc55903eb90978fdf9` 和最终 CI 为准。
- D-027 的“跳过 Review”是中间时序事实，后续 `docs/review/audit-summary.md` 记录了最终历史 Review；
  两者都不成为 HOQA Audit 判定。
- `D2-TDD-01` 的 pending 文本属于已关闭迭代的记录不完整，不恢复为 iteration-3 blocker。
- 旧 WSL、Docker stage、角色 inbox/status 和 PROQAID 版本切换是历史执行机制，不是当前指令。
