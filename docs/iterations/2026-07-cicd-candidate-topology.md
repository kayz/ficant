# CICD 候选拓扑同步 brief

## 目标

- 修复 Phase 2 堆叠 PR 暴露的仓库路径策略与供应链候选拓扑漂移，使中央 CI 能验证任意非空、线性的 base→candidate 区间。
- 在中央 CI 全绿后，将 Phase 2B、2C、2D 依次同步并合并到 `main`。

## 验收

- `tests/phaseN/verify_acceptance_matrix.py` 与 `tests/phaseNx/verify_acceptance_matrix.py` 属于受限验收工具路径；同目录其他 Python 文件仍被拒绝。
- Pull Request 使用事件中的 base SHA；默认分支 push 使用 before SHA，功能分支 push 使用与默认分支的 merge-base；首次 push 的全零 before 安全回退默认分支，不可解析、非祖先、空区间和含 merge commit 的区间 fail-closed。
- 供应链证据记录真实 base、candidate、tree 和提交数，秘密扫描仍覆盖 base 历史、候选区间与发布树。
- 本地治理门禁、项目快速门禁和 GitHub 十项 CI 均通过。
- Phase 2B、2C、2D 按依赖顺序进入 `main`，最终本地与远端 `main` 一致。

## 非目标

- 不改变 Phase 2 数值语义、Oracle、expected、容差或业务验收。
- 不更换供应链工具、漏洞数据库快照、许可证策略或发布方式。
- 不把 GitHub CI 结果冒充 OPAID 本地测试结果。

## 公共契约变化

- `supply-chain.lock.json` 不再冻结某次历史迭代的 base SHA 和提交数，改为冻结“CI 事件/默认分支提供 base、门禁派生正数线性区间提交数”的长期合同。
- 仓库 Python 所有权策略新增严格的 Phase 验收矩阵验证器路径族，不开放通用 `tests/**.py`。

## 需 Human 决策

- 无；Human 已授权完成 GitHub 同步和必要的中央 CI 合同修复。

## 最终真实测试证据

- `bash -n .github/scripts/verify-supply-chain.sh .github/scripts/verify-repo-policy.sh .github/scripts/tests/run-repo-policy-tests.sh .github/scripts/tests/fixtures/release-topology/run.sh`：exit 0。
- `bash .github/scripts/tests/run-repo-policy-tests.sh`：exit 0，`repo-policy-tests: PASS`。
- `bash .github/scripts/tests/run-gates-tests.sh`：exit 0，许可证、风险接受、供应链证据正负夹具全部通过，`gate fixture tests: PASS`。
- `bash .github/scripts/verify-repo-policy.sh --stage final`：exit 0。
- `.\scripts\check-fast.ps1`：exit 0，Rust 格式、Workspace 检查、非环境测试和存储库测试全部通过。
- GitHub 十项 CI 与堆叠 PR 合并证据在候选推送后由中央平台生成，不预写为本地通过。

## 残余风险

- 本地按锁下载 Gitleaks 的拓扑专项夹具因 GitHub Release 传输长时间无响应而主动终止；固定供应链夹具已通过，真实 Gitleaks、SBOM、OSV 和三层秘密扫描仍须以中央 `supply-chain` job 的结果收口。
- 三个 Phase 2 PR 在本候选合并并更新 base 前仍会显示旧合同失败；不得绕过 required checks 合并。
