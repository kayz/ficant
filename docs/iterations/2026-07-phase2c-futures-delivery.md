# Phase 2C：国债期货交割价值链

## 目标

- 在精确基线 `93dcf1efa1ed842a0c114457f356557f310ed18a` 上交付中金所 2 年、5 年、10 年和 30 年国债期货的参考合约参数与可交割券资格判断。
- 交付中金所口径转换因子（CF）、交割发票价、基差、含融资成本的净基差、隐含回购利率（IRR）和最便宜可交割券（CTD）参考实现。
- 贯通 Rust 领域类型、C++20 数值内核、加法式 C ABI、安全 Rust adapter、确定性 Arrow Artifact，以及真实 PostgreSQL/Ceph RGW 发布、重启重放和篡改失败关闭。
- 通过多个快速子循环交付：合约与资格；CF/发票价；基差/净基差/IRR/CTD；Artifact 与真实集成；最终独立 Oracle 和全门禁。

## 验收

- `TS`、`TF`、`T`、`TL` 的名义票息均为 3%，合约面值为 100 万元，百元净价报价；可交割券原始期限和交割月首日剩余期限严格按中金所现行合约表判断，边界日期有正负用例。
- CF 使用中金所公布公式，`r=3%`，输入为交割月至下一付息月的月数 `x`、剩余付息次数 `n`、可交割券票息 `c` 和年付息次数 `f`，结果按十进制四舍五入至小数点后 4 位。
- 每百元交割发票价固定为 `futures_clean_price * conversion_factor + delivery_accrued_interest`，交割应计利息按实际天数/实际天数计算并四舍五入至 7 位。
- 每百元基差固定为 `spot_clean_price - futures_clean_price * conversion_factor`。持有收益固定为 `delivery_accrued_interest - purchase_accrued_interest + interim_coupons - financing_cost`，其中 `financing_cost = purchase_dirty_price * financing_rate * actual_days / 365`；净基差为 `gross_basis - holding_carry`。
- 未再投资 IRR 固定为 `((invoice_price + interim_coupons) / purchase_dirty_price - 1) * 365 / actual_days`。篮子 CTD 按 IRR 由高到低选择；IRR 相同则按净基差由低到高、再按稳定 bond ID 字典序选择。
- 所有输入绑定 owner、FuturesContract、Bond、MarketRulePack、DataSnapshot、估值/结算/交割日期、算法/约定/ABI 版本；非法合约月份、非正价格/CF、日期倒置、不可交割券、空篮子、重复 bond ID、非有限值、ABI/size/reserved 漂移均失败关闭。
- 至少覆盖四个期限品种、年付息/半年付息、持有期内有/无付息、资格上下边界、CF 四舍五入边界、CTD 排序与 tie-break 的独立 Decimal Golden Case；生产结果不得由 expected 反向生成。
- C ABI 不抛异常，不破坏 Phase 2A/2B 布局和符号；Rust/C++ 结果逐项一致，确定性 Arrow bytes/hash 可重放。
- `./scripts/check-fast.ps1`、`./scripts/check.ps1` 和 `./scripts/check.ps1 -IncludeIntegration` 在同一最终候选上 exit 0；集成测试使用真实 PostgreSQL 16 与固定 digest 的 Ceph RGW，覆盖发布、adapter 重建后重放、复算、篡改和 orphan/staging 清零。

## 非目标

- 不实现 Phase 2D 的期现 DV01 套保比例、曲线风险对冲、多合约优化或组合层头寸管理。
- 不实现交易所交割配对、意向申报、持仓限额、保证金、手续费、违约处置、期转现、移仓、撮合或真实清算。
- 不做外部行情/交割篮子自动下载，不把测试 fixture 冒充实时中金所数据。
- 不修改公共 Protobuf、数据库 migration、既有 Phase 2A/2B expected、Oracle、断言或容差来制造通过。
- 不增加 UI、Python SDK、CLI、无套利曲线、税费、债券借贷成本或票息再投资模型。

## 公共契约变化

- 固定收益内核新增版本化的国债期货交割分析 C ABI 结构体与函数；仅加法扩展，既有 ABI version、结构体布局和符号保持不变。
- Rust 新增 provider-neutral 的内部领域输入/结果、Application port/use case 和独立 Arrow schema；公共 Protobuf 与 PostgreSQL schema 不变，仍以 `ArtifactKind::Generic` 和完整 lineage 发布。
- 中金所规则事实进入冻结 fixture/source manifest，并绑定来源 URL、抓取日期、内容摘要和 MarketRulePack；运行时不联网读取规则。

## 需 Human 决策

- 当前无待决项。若官方公式证据与上述冻结口径冲突，或实现必须改变既有公共 Protobuf、数据库 schema、Phase 2A/2B 数值约定或容差，必须停止并返回 Human 决策。

## 最终真实测试证据

- 待最终候选形成后填写；中间命令和 Agent 交流不写入本 brief。

## 残余风险

- 待最终候选形成后填写。当前明确：参考算法只覆盖冻结的交割价值链，不等同于实时交易建议；交割篮子和规则更新仍需后续外部数据适配保证时效性。
