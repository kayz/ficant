# ficant 验收与证据索引

**当前结论：** iteration-1 仅提供治理文档检查；没有生产代码、测试或可运行系统，因此没有 Phase 0 或 DMQuant 行为被判定通过。

## 状态词汇

| 状态 | 含义 |
|---|---|
| `planned` | 已定义验收，但尚未收集执行证据 |
| `collected` | 已收集证据，尚未评审 |
| `passed` | 证据满足预期且已评审 |
| `failed` | 观察结果不满足预期 |
| `accepted-deviation` | 人类在当前清单中明确接受偏差 |

文档存在不等于可执行行为通过。

## 当前治理证据

| ID | 验收 | 当前状态 | 证据位置 |
|---|---|---|---|
| QG-01 | 当前迭代、总体目标和有效期明确 | collected | `.proqaid/orchestrator/current-iteration.md` |
| QG-02 | Product/Architecture/Interface/Quality/Delivery/Review 全覆盖 | passed | 六个常驻角色均完成；最终 Review 为 `pass-with-accepted-findings` |
| QG-03 | Codex/Claude 工具硬约束语义相同 | collected | `.codex/AGENTS.md`、`.claude/CLAUDE.md` |
| QG-04 | 每个角色 context 声明 docs 产物、用途和文件边界 | collected | `.proqaid/<role>/context.md` |
| QG-05 | Review 阻塞/重要发现全部路由 | passed | 最终 Review 为 `pass-with-accepted-findings`；R-I-01..R-I-05 已纠正或由人类接受 |
| QG-06 | 清理与 Git 变更清单完整 | passed | `.planning/` 已清理；allowlist Git 基线已推送，远端提交与 10 文件树均验证一致 |
| QG-07 | 外部系统与密钥访问符合授权边界 | passed | GitHub 仓库创建与 allowlist push 已获授权；测试机和 `C:\git\key` 未访问；仓库敏感标记扫描无命中 |
| QG-08 | 无法证明的模型应用标为 unverified | collected | dispatch log 与各角色输出 |

## 后续 Phase 0 证据

以下全部为 `planned`：

| ID | 必须证明的闭环 |
|---|---|
| QP0-01 | Ubuntu 24.04 x86_64 一条命令启动开发环境 |
| QP0-02 | Rust、Python、C++、Web 构建可重复，版本和 lock 固定 |
| QP0-03 | Protobuf 对 Rust/Python/TypeScript 生成且 CI 防漂移 |
| QP0-04 | PostgreSQL 16 空库迁移、重复执行、升级/恢复证据 |
| QP0-05 | README Phase 0 命名交付物清单完整 |
| QP0-06 | 唯一技术栈、可复现、依赖/SBOM/漏洞与密钥安全门禁 |

## 后续 DMQuant 业务闭环证据

以下全部为 `planned`：

| ID | 必须证明的闭环 |
|---|---|
| QD-01 | AI 流式输出、生成步骤、完成、断线/失败与重试 |
| QD-02 | AI 参数应用、人工修改和来源标记 |
| QD-03 | 策略新版本保存与幂等回测提交 |
| QD-04 | queued/running/succeeded/failed/cache 状态及阶段/进度/原因 |
| QD-05 | 指标、校验、fingerprint、NAV/信号序列与单位正确 |
| QD-06 | 草稿/失败策略源码与成功运行 Artifact 的不同可用规则 |
| QD-07 | 错误 `code`、`trace_id`、复制和恢复路径 |
| QD-08 | viewer/researcher 后端权限与对象级 RBAC/ABAC |
| QD-09 | 导出/下载/删除的确认、审计与失败结果 |
| QD-10 | 时区、无障碍和评审工具条不进入产品 |

## JSON 证据边界

未来机器可读证据只能放在 `docs/quality/evidence/*.json`，并至少包含：验收 ID、版本/摘要、环境、动作、预期、观察、采集时间和评审状态。必须脱敏；禁止凭证、令牌、私钥、敏感业务载荷、原始二进制、测试代码或生产 Artifact。

## Validity

Valid: long-term until superseded
