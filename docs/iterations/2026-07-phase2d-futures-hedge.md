# Phase 2D：国债期货 DV01 套保比例

## 目标

- 在精确基线 `b7872f7e89bde71c9bbfa1fd39735125e2dba9a1` 上完成 Phase 2 剩余的期现 DV01 套保比例参考实现。
- 以 Phase 2C 选出的 CTD、转换因子和冻结的 CTD DV01，把任意带符号的现券或组合目标风险价值换算为连续期货合约数、推荐整数合约数和整数化后的剩余 DV01。
- 贯通 Rust 领域类型、C++20 数值内核、加法式 C ABI、安全 Rust adapter、独立 Decimal Oracle、确定性 Arrow Artifact，以及真实 PostgreSQL/Ceph RGW 发布、重启重放和篡改失败关闭。
- 通过快速子循环交付：合同与数值内核；应用装配与独立 Oracle；Artifact 与真实集成；Phase 2 最终退出审计。

## 验收

- 风险符号固定为：目标 DV01 为正表示需要卖出期货对冲的多头利率风险，为负表示需要买入期货对冲的空头利率风险；目标 DV01 不得为零。CTD 每百元 DV01、转换因子和合约面值必须为正且有限。
- 国债期货每手 DV01 固定为 `ctd_dv01_per_100 * (contract_notional / 100) / conversion_factor`；中金所 `TS`、`TF`、`T`、`TL` 合约面值均固定为 100 万元，不接受调用方覆盖。
- 连续套保合约数固定为 `raw_contracts = -target_dv01 / futures_contract_dv01`。推荐整数合约数从向下取整、向上取整和零手中选择，使 `abs(target_dv01 + contracts * futures_contract_dv01)` 最小；并列时选择绝对合约数较小者，再选择数值较小者。负数表示卖出期货，正数表示买入期货。
- 结果同时给出推荐整数合约数、整数化剩余 DV01 和 `hedge_effectiveness = 1 - abs(residual_dv01) / abs(target_dv01)`；有效性必须在 `[0, 1]`，连续合约数对应的剩余风险在冻结容差内为零。
- 输入必须绑定 owner、目标 Risk Artifact、Phase 2C Delivery Artifact、CTD Analytics Artifact、FuturesContract、CTD Bond、MarketRulePack、DataSnapshot、估值时点、算法/约定/ABI 版本；CTD 身份、转换因子、DV01 或血缘漂移均失败关闭。
- 至少覆盖 `TS`、`TF`、`T`、`TL`、正负目标风险、零手最优、上下取整和并列规则的独立 Decimal Golden Case；生产结果不得由 expected 反向生成。
- C ABI 不抛异常，拒绝非有限值、错误 size/version/reserved 和整数溢出，不改变 Phase 2A/2B/2C 结构体布局或符号；Rust/C++ 与独立 Oracle 在冻结容差内一致。
- 结果以确定性 Arrow bytes/hash 发布；真实 PostgreSQL 16 + Ceph RGW 覆盖发布、adapter 重建后重放、精确复算、篡改失败关闭，以及 staging/orphan 清零。
- 精确最终候选通过 `./scripts/check-fast.ps1`、`./scripts/check.ps1` 和 `./scripts/check.ps1 -IncludeIntegration`，并以验收矩阵逐项证明 README Phase 2 的参考算法、边界、ABI、Golden Case、规则/快照血缘和套保研究要求。

## 非目标

- 不实现关键期限 DV01 生成、多合约矩阵优化、约束优化、组合头寸账本、保证金、手续费、冲击成本、动态再平衡或下单执行。
- 不把单一 CTD 平行移位 DV01 对冲冒充完整曲线风险免疫；关键期限和跨合约对冲属于 Research Lab/仿真阶段的独立研究方法。
- 不接入外部行情或实时交割篮子，不增加 UI、公共 Protobuf、数据库 migration、Python 控制平面、服务器装配、部署或发布行为。
- 不修改 Phase 2A/2B/2C expected、Oracle、断言、容差、ABI 布局或业务公式来制造通过。

## 公共契约变化

- 固定收益内核新增版本化 DV01 套保 C ABI 输入、结果和函数；只做加法扩展，既有 ABI version、结构体布局与符号保持不变。
- Rust 新增 provider-neutral 的内部领域输入/结果、Application port/use case 和独立 Arrow schema；公共 Protobuf 与 PostgreSQL schema 不变，继续使用 `ArtifactKind::Generic` 和完整 lineage 发布。
- 中金所 DV01 套保公式与合约风险价值口径进入冻结 source manifest；运行时不联网读取规则。

## 需 Human 决策

- 当前无待决项。若实现需要改变既有 Phase 2 数值口径、公共 Protobuf、数据库 schema，或证据表明中金所一手公式与上述冻结语义冲突，必须停止并返回 Human 决策。

## 最终真实测试证据

- 待最终候选形成后填写；中间命令和 Agent 交流不写入本 brief。

## 残余风险

- 待最终候选形成后填写。当前明确：单一 CTD DV01 套保只消除冻结平行 1bp 冲击下的一阶风险，不消除基差、CTD 切换、曲线形变、凸性、流动性或再平衡风险。
