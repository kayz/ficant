# Phase 4A 强类型 ResearchGraph

## 目标

- 在精确基线 `1847458329e1c948cf7ae2053db7b56361415f03` 上交付纯领域层的版本化 `ResearchNodeContract` 与 `ResearchGraph`。
- 用精确类型、schema hash 和确定性 DAG 把后续运行时可执行的研究定义冻结为不可歧义输入。

## 验收

- 节点合同完整声明输入/输出类型、状态/参数 schema、确定性等级、权限、资源限制和必守不变量，并拒绝空值、重复端口与无效资源限制。
- 图拒绝重复/缺失节点、缺失端口、重复或未绑定输入、类型不匹配、自环和环路。
- 相同定义即使以不同 nodes、edges、ports 或 invariants 顺序输入，也得到相同规范顺序、拓扑序和内容摘要。
- `./scripts/check-fast.ps1` 在最终候选上 exit code 0。

## 非目标

- 不实现 NativeNode/GeneratedNode 执行、Run 状态机扩展、RunJournal 扩展、Lease Queue、Worker、恢复、Artifact 节点血缘或实验比较。
- 不修改 Protobuf、数据库 migration、存储 adapter、服务 API、数值算法、Oracle、expected、断言或容差。
- 不创建版本 tag，不 push，不部署，不触发 GitHub CI/CD。

## 公共契约变化

- `ficant-domain::research` 新增 `TypedValue`、`PortType`、`ResearchNodeContract`、`ResearchNode`、`ResearchEdge` 和 `ResearchGraph` 及其受控输入值。
- 类型兼容要求 type ID、version 与 schema hash 全部相等；每个节点输入端口必须恰好由一条边绑定，零输入节点作为图内显式 source。
- 合同与图使用带版本前缀、长度分隔的大端 canonical encoding 生成 SHA-256 摘要；图的确定性拓扑序以 ULID 升序打破并列关系。

## 需 Human 决策

- 当前无待决业务语义；本迭代不授权版本交付。

## 最终真实测试证据

- `cargo test --offline --locked -p ficant-domain --test research_graph_contracts`：exit code 0，4/4。
- `./scripts/check-fast.ps1`：exit code 0；工作区非环境测试与 doc tests 全部通过，新增 ResearchGraph 合同 4/4、storage library 3/3、Phase 3A 5/5、Phase 3B 2/2。

## 残余风险

- 当前摘要是 Rust 领域层 canonical v1 合同，尚无 Protobuf/数据库表示；后续若引入跨语言持久合同，必须用 golden bytes 锁定完全相同的 presence、排序和编码规则。
- 当前图只表达内部节点间连接；外部 Snapshot/Artifact 必须在后续执行计划中映射为显式 source node，不能绕过图注入隐式全局输入。
