# ficant 迭代权威状态与 Iteration 3 整理方案

## 权威、状态与本次边界

- 治理方法：`HOQA`；活动方法为 `.hoqa/SKILL.md`，机器可恢复状态为 `.hoqa/state.toml`，本文件是唯一迭代状态与范围明细。
- 当前项目状态：`CLOSED_ITERATION_3`；没有活动迭代。
- Human 已批准采用 Iteration 3 umbrella + 3A..3D，已接受 3A 和统一 3B，并授权完成 3C。3B 的三部分仍作为一个已接受迭代顺序执行并统一收口；不改变产品目标、架构边界、Oracle、expected、容差或风险决定。
- Human 已接受 3A..3D；2026-07-16 限时接受 `RUSTSEC-2025-0052` 至 2026-10-13 的私有 GitHub 源码集成风险，并分别授权候选 push + PR 与 merge。Human 随后单独授权 `v0.1.0-alpha.3` 私有、仅源码 GitHub Pre-release，并将该风险接受扩展至本次发布；Quality 已完成并退出，发布授权候选仍需外部只读 Audit。部署、UAT 和任何新迭代未授权。
- `.proqaid` 已移除，`.hoqa` 是唯一活动治理入口；迁移与 3A 已随 Iteration 3 单提交候选固化，独立的 3A 候选从未作为远程集成或正式发布成果。

## HOQA 轻量激活规则

- Human 始终负责目标、业务含义、接受标准、风险接受和特权/低可观测操作。
- Orchestrator 始终负责计划、Product/Architecture/Interface lens、执行编排、集成、确定性验证和关闭准备。
- Quality 只在迭代包含代码更新并需要非平凡自动化测试设计、测试执行、缺陷循环或测试报告时激活。
- Audit 只在准备发布、形成正式发布候选或关闭发布范围时激活，且只做最终文档到意图、代码和证据的一致性检查。
- Worker 只在至少两个真正独立、有界且并行收益明确的任务存在时临时使用。
- 历史 PROQAID Quality/Review 记录保留当时事实，但不转换为当前 HOQA 参与者已运行的声明。

## Iteration 1 — 初始治理与 Phase 0 契约准备

| 项目 | 权威整理 |
|---|---|
| 迭代目标 | 建立初始治理、产品/架构说明和 Phase 0 契约准备基线，使后续工程可以在明确产品与 DMQuant 边界下开始。 |
| 输入和依赖 | Human 确认；当时的 README、UI-DM 输入和初始 10 路径发布 allowlist。 |
| 范围 | 文档与治理初始化、责任边界、初始发布证据和清理记录。 |
| 非目标 | 生产代码、测试桩、硬编码 demo、密钥访问和 Phase 0/1 工程实现。 |
| 包含代码更新 | 否。 |
| Quality | 按当前 HOQA 规则不需要；历史 PROQAID Quality 只作历史证据，不追补运行。 |
| 涉及发布 | 仅发布初始文档/治理 Git 基线，不是产品发布。 |
| Audit | 按当前 HOQA 规则不需要；历史 PROQAID Review 不改写为 HOQA Audit。 |
| Human 检查点 | 已完成当时范围与发布授权；现无待确认事项。 |
| 验收条件 | 文档/治理边界一致；无生产实现或敏感资产进入候选；初始 allowlist 和发布证据可追溯。 |
| 当前状态 | `CLOSED_HISTORICAL`，2026-07-11 关闭。 |
| 已有证据 | final commit `42f570f309e20c867f65cffbce76e7f6d64d65d5`；tree `94891a70b1df0e2befcad56246ef8c7c2c4bee8c`；trusted parent `affce937b30ba14b59777691ec8d311dbb5161ba`；历史 verdict `pass-with-accepted-findings`。 |
| 未完成事项和阻断 | 无活动事项；历史 pre-state 无法追溯证明的限制继续保留，不恢复为当前 blocker。 |

## Iteration 2 — Phase 0/1 工程与真实业务闭环

| 项目 | 权威整理 |
|---|---|
| 迭代目标 | 完成 Phase 0 仓库/契约基线与 Phase 1 领域内核，并用真实 PostgreSQL 16、MinIO、版本/血缘和重放闭环证明不是空目录、CRUD 桩或假成功。 |
| 输入和依赖 | Iteration 1 关闭基线、README Phase 0/1、根 `interface/`、Windows/Linux CI 与 Docker Compose 能力。 |
| 范围 | Rust workspace、唯一 Protobuf 契约、Migration、PostgreSQL/MinIO、领域对象、真实持久化/重放、Platform Shell、四类构建、CI 与供应链门禁。 |
| 非目标 | Phase 2 债券数值算法、完整 DMQuant/回测/仿真/GeneratedNode、测试机和密钥访问、第二后台语言/契约源/数据库。 |
| 包含代码更新 | 是。 |
| Quality | 需要；历史质量证据已形成。按 HOQA 不重新执行，只保留完成事实。 |
| 涉及发布 | 是，形成并发布正式关闭候选到 `main`。 |
| Audit | 若按 HOQA 重演则需要最终一致性 Audit；实际只有历史 PROQAID Review。迭代已关闭，不追补或重命名历史 verdict。 |
| Human 检查点 | 当时已接受业务结果和限时风险；`RUSTSEC-2025-0052` 须在每轮 Align、首次外部发布或 2026-10-13 到期时最先触发者重新评估。 |
| 验收条件 | Phase 0/1 范围、真实业务闭环、Migration、契约、四类构建、Web、供应链和 CI 证据通过；剩余风险被显式接受。 |
| 当前状态 | `CLOSED_HISTORICAL`，2026-07-13 关闭。 |
| 已有证据 | final commit `80f48706f37e3890224ca106fb763213d0beeb38`；tree `9dd6a136d453872dc37085bc55903eb90978fdf9`；candidate/main CI `29201419136` / `29201891313`；业务测试 `14 passed, 0 failed, 0 skipped`；供应链 `620 = 607 third-party + 13 first-party`。 |
| 未完成事项和阻断 | 无 Iteration 2 活动 blocker；`RUSTSEC-2025-0052` 是已接受、未修复且有到期日的持续风险。 |

## Iteration 3 — 当前固定收益分析纵向切片

| 项目 | 权威整理 |
|---|---|
| 迭代目标 | 在冻结的 CGB 参考约定下，贯通 C++20 内核、稳定 C ABI、安全 Rust adapter、确定性 Arrow Artifact、血缘、持久化和重放。 |
| 输入和依赖 | Iteration 2 基线；Q-001..Q-036；ADR-0001..0005、0007、0008；`cgb-reference-v1`；QuantLib 1.42.1 独立 Oracle；Windows 开发/runner 能力。 |
| 范围 | 固定利率和贴现国债现金流、应计、净价/全价、YTM、久期、凸性、DV01；ABI/unsafe；Rust Domain/Application/sys/native/Storage；Artifact 发布与重放；适用的自动化验证、SIT 和关闭证据。 |
| 非目标 | 曲线、期货、Web/Python SDK、CLI、新 migration、SIMD、生产 QuantLib、公共 Protobuf 变更、假成功、硬编码生产结果、测试专用生产路径、桩和未解决占位。 |
| 包含代码更新 | 是；3A、3B 已接受；3C 允许仅为自动化覆盖或结构化 blocking defect 作有界更新；3D 仍禁止自动恢复。 |
| Quality | 3A、3B 已完成各自范围；3C Quality 已完成集成自动化、SIT、缺陷分类、受影响清单重测与统一报告，当前不再激活。 |
| 涉及发布 | 是；PR #1 与收口 PR #2 已合入私有 GitHub `main`，`v0.1.0-alpha.3` 私有 source-only Pre-release 已获单独授权。实际 tag/Release 状态由 GitHub 外部页面记录；没有二进制、签名、部署或 UAT。 |
| Audit | Iteration 3 与收口候选的外部只读 Audit 均为 `pass`、0 blocking finding；本次 source-only Pre-release 授权候选仍须在发布前接受同样的外部只读 Audit，verdict 不写回被审计提交。 |
| Human 检查点 | Iteration 3 业务结果、剩余风险、候选发布、merge 与 `v0.1.0-alpha.3` source-only Pre-release 均已在 2026-07-16 获明确决定；完成外部发布后才返回下一迭代讨论。 |
| 验收条件 | 一个最终集成候选满足 Q-001..Q-036、数值/ABI/内存/unsafe/血缘边界、真实持久化与重放、适用交付门、Quality 报告、最终 Audit 和 Human 业务/风险接受。 |
| 当前状态 | `CLOSED_ACCEPTED`；3A、3B、3C、3D 均完成并接受，Iteration 3 umbrella 已关闭。 |
| 已有证据 | 既有 Wave 1/Oracle、3A/3B/3C 证据保持；正式候选 `f300597`、tree `7e8d6c6`、PR CI `29464247114`、Audit pass、squash merge `6e346d0` 与 main CI `29472793718` 形成完整发布链。 |
| 未完成事项和阻断 | 无 Iteration 3 blocking item；`RUSTSEC-2025-0052` 仍须在运行时/外部部署前或 2026-10-13 到期时重新评估。下一迭代未授权。 |

### Iteration 3 状态分层

- **已完成并已接受：** Iteration 3 产品/架构/接口边界；C++ Wave 1 `768b400`；QuantLib/Golden 资产 `0485442`；既有 Windows runner 恢复与能力发现；3A HOQA-v3 本地基线候选（最终 runner hash `dffc1dfe...`）及其 108/108 runner 与 4/4 Wave 1 证据。
- **明确拒绝的候选：** preserved `codex/i3-runner-ctest` 不作为接受候选。历史失败日志未保存当时 worktree registry、环境和精确候选 hash，无法证明根因；不以当前成功反向解释旧失败，也不直接合并该 dirty worktree。
- **本地基线成果：** 3A 接受的是经 HOQA-v3 合并和验证、随后纳入 Iteration 3 单提交候选的本地成果；不得把 3A 独立描述为远程集成或正式发布成果。
- **已完成并已接受：** 3B Rust 纵向实现按 Domain/Application → sys/C ABI/native → Arrow Artifact/Storage 顺序形成一个候选和一份统一 Quality 结果，并于 2026-07-16 被 Human 接受。
- **已完成并已接受：** 3C 的 Q-001..Q-036 集成自动化、真实 PostgreSQL/MinIO SIT、环境问题分类、四项缺陷修复与受影响清单重测；无 blocking defect，隔离资源已清理。
- **已完成并已接受：** 3D 候选门禁、外部只读 Audit、私有 PR #1、squash merge 与最终 main CI。
- **已获 Human 决定：** `RUSTSEC-2025-0052` 限时接受至 2026-10-13，覆盖私有 GitHub 源码集成及 `v0.1.0-alpha.3` 仅源码 Pre-release；候选发布、merge 与该 Pre-release 已分别授权。
- **尚未完成：** 没有 Iteration 3 未完成项；Pre-release 是关闭后的独立发布操作，外部状态由 GitHub 记录。部署、UAT 和任何新迭代均未授权。

## 已确认的剩余工作拆分

Human 已确认保留 Iteration 3 作为历史 umbrella，并以 `3A..3D` 表示其完成段；3A、3B、3C、3D 均已接受并关闭。

### Iteration 3A — runner 收敛与 Wave 1 证据接受

| 项目 | 权威结果 |
|---|---|
| 迭代目标 | 证明并修复 `I3-RUNNER-001`，对 preserved runner candidate 作接受或拒绝决定，并把 Wave 1 CTest 证据绑定到一个已接受当前候选。 |
| 输入和依赖 | HEAD `33b4210`、preserved candidate、既有 4/4 证据、runner 合同/schema、冻结 Wave 1/Oracle 输入。 |
| 范围 | linked-worktree 生命周期根因、最小 runner 修复、完整 Windows runner suite、候选身份与清理证据。 |
| 非目标 | Wave 2 业务实现、测试语义/Oracle/容差变化、SIT、发布和 UAT。 |
| 包含代码更新 | 是，仅 `deploy/execution` 的 runner、profile、schema 和测试。未修改业务代码、Oracle、expected、容差或数据。 |
| Quality | 已激活并完成；最终自动化清单为 Windows runner suite 108/108 与 candidate-bound Wave 1 4/4。 |
| 涉及发布 | 否。 |
| Audit | 否。 |
| Human 检查点 | Human 已接受完成结果，并随后授权统一 3B；该后续授权不改变 3A 历史范围。 |
| 验收条件 | preserved candidate 被明确拒绝或其根因被证明；新候选完整 suite 退出 0、无 skipped/未解释失败；4/4 证据绑定最终 runner；清理完成。 |
| 当前状态 | `CLOSED_ACCEPTED`，2026-07-16；授权仅覆盖本地基线提交，不覆盖推送或发布。 |
| 已有证据 | preserved candidate 因旧失败证据无法绑定精确状态而明确拒绝；新 HOQA-v3 候选 PowerShell 解析 3/3、schema/config 通过、runner suite `108/108` skipped 0、Wave 1 `4/4` skipped 0；能力证据 ID `sha256:4352be12...`，结果 SHA-256 `ac43d768...`。 |
| 未完成事项和阻断 | 3A 无开放 defect 或技术 blocker；后续统一 3B 授权和执行已单独记录。 |

### Iteration 3B — Rust 纵向实现

| 项目 | 权威结果 |
|---|---|
| 迭代目标 | 在冻结边界下实现并集成 Domain/Application/sys/native/Storage 的最小完整纵向切片。 |
| 输入和依赖 | 已接受 3A runner 基线、C++ Wave 1、冻结 ADR-0002/0004/0005、Q-001..Q-036 和既有 Oracle/fixture。 |
| 范围 | provider-neutral 领域合同、Application ports/use case、唯一 unsafe sys、safe native adapter、确定性 Arrow codec、Artifact stage/verify/publish/read/replay 接线。 |
| 非目标 | 公共 Protobuf、新 migration、曲线/期货/SDK/CLI、生产 QuantLib、动态插件、SIT/发布关闭。 |
| 包含代码更新 | 是。 |
| Quality | 需要；数值、FFI/内存、Artifact 和回归自动化必须由 Quality 组织并报告。 |
| 涉及发布 | 否。 |
| Audit | 否。 |
| Human 检查点 | Human 已接受统一候选；任何业务语义、Oracle、容差、公共接口或风险边界变化仍必须停下确认。 |
| 验收条件 | 模块边界和 ABI/unsafe 约束满足；纵向用例可运行；自测与风险回归通过；无 fake/stub/TODO/测试专用生产路径。 |
| 当前状态 | `CLOSED_ACCEPTED`，2026-07-16；下列 1、2、3 已顺序完成，只有一个集成候选和一个统一 Quality 报告。 |
| 已有证据 | 新增测试 7/7；受治理的主非环境库存 159/159、Storage library 3/3，连同 3B 端到端共 165/165；严格 Clippy 退出 0、CTest 4/4；Arrow golden SHA-256 `0d74da243ddd828afd47dfc4e26fc9615b3e62525dc52b646ef1440f17959ef6`。 |
| 未完成事项和阻断 | 无 3B blocking defect；3C/SIT 已另行授权并执行，3D/发布不属于本候选。 |

#### Ordered part 1 — Domain/Application 契约与计算 port

| 项目 | 权威结果 |
|---|---|
| 迭代目标 | 建立 provider-neutral Domain/Application 契约、计算 port 与最小 use case，为 FFI 接入提供稳定上层边界。 |
| 输入和依赖 | 已接受 3A 本地基线；ADR-0001/0002/0003/0005；冻结 Q-001..Q-036 业务语义。 |
| 范围 | 领域输入/输出、错误与单位契约；Application port/use case；纯 Rust 单元与边界测试。 |
| 非目标 | C ABI、unsafe、native adapter、Arrow、Storage、SIT、发布、公共 Protobuf 或 migration。 |
| 包含代码更新 | 是。 |
| Quality | 在 3B 统一 Quality 范围内确认契约、单位、错误和回归自动化，不修改 Oracle、expected 或容差。 |
| 涉及发布 / Audit | 均否。 |
| Human 检查点 | 无中间检查点；完成后直接进入 ordered part 2。 |
| 验收条件 | Domain/Application 保持 provider/FFI/Arrow/SQL 中立；use case 可用测试替身执行；无 fake、stub、TODO 或测试专用生产路径；自动化无 unexplained skip。 |
| 当前状态 | `COMPLETED_IN_UNIFIED_3B_CANDIDATE`。 |

#### Ordered part 2 — C ABI、唯一 unsafe sys 与 safe native adapter

| 项目 | 权威结果 |
|---|---|
| 迭代目标 | 在已接受 Wave 1 kernel 上实现冻结 C ABI、唯一 unsafe sys crate 和 safe native adapter。 |
| 输入和依赖 | 同一 3B ordered part 1 契约；ADR-0002/0005；C++ Wave 1 与 3A runner。 |
| 范围 | ABI 类型/错误/内存边界、sys 绑定、unsafe allowlist、safe adapter、动态链接与数值回归。 |
| 非目标 | Artifact Storage、SIT、发布、Oracle/expected/容差变化。 |
| 包含代码更新 / Quality | 是；纳入统一 Quality 的 ABI、内存、异常、数值和动态链接测试。 |
| 涉及发布 / Audit | 均否。 |
| Human 检查点 | 无中间检查点；ordered part 1 自测通过后顺序进入，完成后直接进入 part 3。 |
| 验收条件 | unsafe 只存在于唯一 sys crate；native adapter 对上层安全；ABI/内存/异常/数值测试通过且无 unexplained skip。 |
| 当前状态 | `COMPLETED_IN_UNIFIED_3B_CANDIDATE`。 |

#### Ordered part 3 — Arrow Artifact 与存储重放接线

| 项目 | 权威结果 |
|---|---|
| 迭代目标 | 实现确定性 Arrow Artifact 编码及 stage/verify/publish/read/replay 接线。 |
| 输入和依赖 | 同一 3B ordered part 2 adapter；ADR-0005；现有 Artifact/Storage ports 与 Phase 1 持久化基线。 |
| 范围 | 确定性 schema/codec、内容哈希、lineage、Artifact 生命周期与重放路径。 |
| 非目标 | 新 migration、公共 Protobuf、SIT、发布和范围外业务功能。 |
| 包含代码更新 / Quality | 是；纳入统一 Quality 的确定性、篡改、失败恢复、发布原子性与重放测试。 |
| 涉及发布 / Audit | 均否。 |
| Human 检查点 | 无中间检查点；完成后与前两部分一起形成统一候选和 Quality 报告，再返回 Human。 |
| 验收条件 | 相同输入生成相同 Artifact；stage/verify/publish/read/replay 与 lineage 可追溯；失败不产生假发布；自动化无 unexplained skip。 |
| 当前状态 | `COMPLETED_IN_UNIFIED_3B_CANDIDATE`。 |

### Iteration 3C — 集成自动化验证与 SIT

| 项目 | 权威状态 |
|---|---|
| 迭代目标 | 对一个集成候选完成 Q-001..Q-036 自动化覆盖、真实持久化/重放和必要 SIT，收敛阻断缺陷。 |
| 输入和依赖 | 3B 集成候选、冻结数据/expected/Oracle/容差、Windows runner；需要 SIT 时由 Human 保持 Docker Desktop 可用。 |
| 范围 | Quality 自动化清单与报告、数值/ABI/unsafe/动态链接、PostgreSQL/MinIO stage-to-replay、重启/篡改/清理、适用供应链与可复现门。 |
| 非目标 | 改写验收以取得绿灯、UAT、外部发布、凭证访问和未批准的新产品范围。 |
| 包含代码更新 | 允许仅针对结构化 bug list 的有界修复；每次修复重测受影响清单。 |
| Quality | 已激活并完成；负责测试策略、执行汇总、test report、bug list 和受影响清单重测。本轮未使用 Test Worker。 |
| 涉及发布 | 否，仅形成可进入正式候选准备的验证候选。 |
| Audit | 否。 |
| Human 检查点 | Docker Desktop 由 Human 保持可用；当前检查点是接受 3C 候选，并与是否授权 3D 分开决定。 |
| 验收条件 | 自动化清单完整；实际计数、退出码和候选身份可追溯；无 skipped/fabricated/flaky success；无开放阻断 defect；环境清理完成。 |
| 当前状态 | `CLOSED_ACCEPTED`，2026-07-16；实现、Quality 缺陷循环、集成验证与 SIT 已完成并获 Human 接受。 |
| 已有证据 | `tests/iteration-3/acceptance-matrix.json` 完整映射 Q-001..Q-036 且校验退出 0；冻结 expected/Oracle/calendar hash 未变；production-native 冻结用例 12/12、Oracle 自测 31/31、Storage 31/31、Acceptance 14/14、契约 11/11、health 5/5、Release 与 ASan CTest 各 4/4；动态链接、unsafe allowlist、依赖隔离和受影响 crate 严格 Clippy 均通过；隔离 SIT 资源清理为 0。 |
| 未完成事项和阻断 | 无 blocking defect。正式 QuantLib 1.42.1 本轮未由 Orchestrator 重新抓取/构建，因该工作流明确属于 Human Operator；冻结资产未变，沿用已接受历史执行证据。全 workspace 严格 Clippy 的 196 个 generated Protobuf/tonic 文档 lint 不作为手写受影响代码失败；3D 交付/供应链/Audit 门仍未授权。 |

### Iteration 3D — 正式候选、发布准备与关闭

| 项目 | 提案 |
|---|---|
| 迭代目标 | 冻结正式候选和证据，完成发布准备、最终一致性 Audit 与 Human 业务/风险接受，关闭 Iteration 3 umbrella。 |
| 输入和依赖 | 3C 无阻断缺陷的候选、Quality report、制品/供应链/持久化证据、最终文档和剩余风险。 |
| 范围 | 候选/tree/parent 身份、制品和回滚方案、发布就绪证据、最终文档一致性、Audit、Human 接受和状态原子关闭。 |
| 非目标 | 新业务实现、测试语义变化、自动 UAT、未授权部署和任何凭证读取。 |
| 包含代码更新 | 原则上否；若候选变化，返回相应代码/测试迭代并重新建立证据。 |
| Quality | 条件激活；复用 3C 最终报告，只有候选变化或需重测时再次激活。 |
| 涉及发布 | 是，形成正式发布候选和发布准备；实际部署仍需 Human 明确授权。 |
| Audit | 是，只读检查最终文档、Human 意图、候选和证据一致性。 |
| Human 检查点 | 接受最终业务结果、剩余风险和发布权限；凭证/低可观测操作只能由 Human 执行。 |
| 验收条件 | 正式候选身份唯一；适用门禁与回滚准备完整；Quality 无阻断；Audit 通过；Human 接受；状态改为 `CLOSED`。UAT 位于关闭后且不影响关闭。 |
| 当前状态 | `CLOSED_ACCEPTED`，2026-07-16；Human 已接受剩余风险、授权私有候选发布并随后明确授权 merge。 |
| 已有证据 | 正式候选 `f300597a222e6442b52853eff046241a396aad4b`、tree `7e8d6c6139814e268e76e859ccba6c59ce2ec54c`、PR #1、候选 CI `29464247114`、Audit pass、squash merge `6e346d0dcb7b236289e1063fbb84f3372d689703`、收口 merge `1053aae1694040bff5a77263cb788f56fbe65f40` 和 main CI `29502458038`。Pre-release 的 tag、target 与页面由 GitHub 外部证据记录。 |
| 未完成事项和阻断 | 无 3D blocking item；风险替代方案须在运行时/外部部署前或到期日复评。不得访问、枚举、复制、输出或提交 `C:/git/key`。 |

## Human 决定与后续权限边界

1. Human 于 2026-07-16 限时接受 production-reachable 的 `RUSTSEC-2025-0052` / `async-std 1.13.2` 风险至 2026-10-13；范围为私有 GitHub 源码集成及 `v0.1.0-alpha.3` 仅源码 Pre-release，任何运行时或外部部署前必须重新评估替代方案。
2. Human 已分别授权候选 push + PR、squash merge 与本次 source-only Pre-release。发布不得附二进制或签名；部署和 UAT 仍须另行确认。

## 当前停止点与下一轮

Iteration 3 已关闭，没有活动迭代。本次只执行已授权 `v0.1.0-alpha.3` 私有 source-only Pre-release，发布状态以 GitHub tag/Release 页面为外部权威；完成后返回下一迭代讨论。Human 明确授权前不恢复业务实现、部署或 UAT。
