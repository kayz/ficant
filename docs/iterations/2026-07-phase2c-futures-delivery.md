# Phase 2C：国债期货交割价值链

## 目标

- 在精确基线 `93dcf1efa1ed842a0c114457f356557f310ed18a` 上交付中金所 2 年、5 年、10 年和 30 年国债期货的参考合约参数与可交割券资格判断。
- 交付中金所口径转换因子（CF）、交割发票价、基差、含融资成本的净基差、隐含回购利率（IRR）和最便宜可交割券（CTD）参考实现。
- 贯通 Rust 领域类型、C++20 数值内核、加法式 C ABI、安全 Rust adapter、确定性 Arrow Artifact，以及真实 PostgreSQL/Ceph RGW 发布、重启重放和篡改失败关闭。
- 通过多个快速子循环交付：合约与资格；CF/发票价；基差/净基差/IRR/CTD；Artifact 与真实集成；最终独立 Oracle 和全门禁。

## 验收

- `TS`、`TF`、`T`、`TL` 的名义票息均为 3%，合约面值为 100 万元，百元净价报价；可交割券原始期限和交割月首日剩余期限严格按中金所现行合约表判断，边界日期有正负用例。
- CF 使用中金所公布公式，`r=3%`；交割月至下一付息月的月数 `x` 和剩余付息次数 `n` 必须由冻结债券日程推导，不接受调用方直接提供，可交割券票息为 `c`、年付息次数为 `f`，结果按十进制四舍五入至小数点后 4 位。
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

- 独立 Oracle：`uv run --offline --locked --project python python -m pytest tests/oracle/china-rates/test_phase2c_manual_oracle.py -q`，exit 0，3/3；覆盖冻结 expected 精确生成、四个期限品种及年付息/半年付息和持有期有/无付息，并校验中金所官方来源事实摘要 `d1149c4594f3cc14ad977200e1bab6e48de3475d17dc03c7bb096ca369e05499`。
- Phase 2C acceptance matrix：`uv run --offline --locked --project python python tests/phase2c/verify_acceptance_matrix.py`，exit 0，18/18；输入、expected、来源 manifest、Oracle、领域/ABI/native/Arrow/SIT 路径均由 SHA-256 fail closed。
- `./scripts/check-fast.ps1`：exit 0；包含 Phase 2C domain/native 回归和 storage library 3/3。
- `./scripts/check.ps1`：exit 0；严格 Clippy/build/test、生成契约 11/11、C++ CTest 7/7、Phase 2A matrix 36/36、Phase 2B matrix 16/16、Phase 2C matrix 18/18、Phase 2C Oracle 3/3、确定性 Arrow 1/1、Python 1/1、Web 4 files / 29 tests 全部通过。
- `./scripts/check.ps1 -IncludeIntegration`：exit 0；真实 PostgreSQL 16 + Ceph RGW 20.2.2 上 migration 4/4、Phase 1 业务闭环 1/1、负向不变量 13/13、Phase 2B 发布重放 1/1、Phase 2C `real_postgres_ceph_futures_publish_restart_replay_and_tamper_fail_closed` 1/1。Phase 2C 用例验证正式发布、adapter 重建后精确重放、确定性复算、size 篡改 fail-closed，以及 staging/orphan 均清零。
- 集成使用命名的一次性 Compose 项目 `ficant-phase2c`；测试后容器、网络、命名卷和测试数据均删除，复核残留计数为 0。

## 残余风险

- 参考算法只覆盖冻结的交割价值链，不等同于实时交易建议；交割篮子和规则更新仍需后续外部数据适配保证时效性。
- 当前融资口径固定为单利 `actual_days / 365`，票息不再投资，也不包含税费、交易成本、债券借贷成本、保证金或真实交割流程；这些扩展必须新增显式版本和独立验收，不能改变 v1 结果。
- Phase 2 仍未完成期现 DV01 套保比例、曲线风险对冲和组合层优化；这些能力由独立 Phase 2D 迭代交付，不以本候选的 CTD 结果冒充。
- 生产 Ceph 高可用拓扑、外部中金所数据时效和服务器装配不由本地 OPAID 证明，仍分别受运维、数据适配和中央 CI/CD 合同约束。
