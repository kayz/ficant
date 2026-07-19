# Phase 2B：收益率曲线与 Carry/Roll-down

## 目标

- 在精确基线 `5b2be2453937b82091b34e256bc4fb69aa9e7415` 上交付 CFETS 风格“剩余期限—到期收益率”节点的区间内线性插值参考实现。
- 交付不含融资成本的国债持有期 Carry/Roll-down 分解：`carry = horizon_dirty_at_initial_yield + paid_cashflows - initial_dirty`，`roll_down = horizon_dirty_at_rolled_curve_yield - horizon_dirty_at_initial_yield`，并满足 `total_return = carry + roll_down`。
- 结果贯通 Rust 领域校验、C++20 数值内核、加法式 C ABI、安全 Rust adapter、独立 Oracle、确定性 Artifact 和真实 PostgreSQL/Ceph RGW 发布重放；一个迭代内以 forward-only 快速子循环交付。

## 验收

- 曲线节点日期严格递增、至少两个、全部晚于曲线估值日；节点收益率为有限小数。精确节点原值返回，节点之间按实际日数线性插值，节点范围外 fail closed，不做隐式外推。
- 持有期起止日、日历覆盖和债券存续期合法；区间内已支付现金流采用调整后支付日所有权。Carry、Roll-down 与总回报分解恒等式在固定利率债和贴现债 Golden Case 上成立。
- C++ 数值函数具备边界、非有限值、容量、ABI、异常不跨边界和内存安全测试；Rust/C++ 结果与独立手工 Oracle 及 QuantLib 对照结果在冻结容差内一致。
- 每个结果绑定 owner、Bond、CurveSnapshot、MarketRulePack、输入 DataSnapshot、估值时点、持有期、算法/约定/ABI 版本；输入或血缘漂移 fail closed。
- 结果以确定性 Arrow Artifact 发布，真实 PostgreSQL 16 + Ceph RGW 路径可发布、重启、恢复、重放并检测篡改，不用 mock 或内存替身冒充集成验收。
- 精确最终候选通过 `check-fast.ps1`、`check.ps1` 和 `check.ps1 -IncludeIntegration`；证据必须记录真实命令、exit code 和可得 test count。

## 非目标

- 不实现节点范围外外推、曲线 bootstrap/回归拟合、即期或远期曲线、多曲线框架、信用利差、融资成本或税费。
- 不实现国债期货合约、可交割券、转换因子、基差、净基差、IRR、CTD 或期现套保；这些属于后续 Phase 2 数值链。
- 不接入外部行情源，不建设 Phase 3 Snapshot 平台，不增加 UI、公共 Protobuf、数据库 migration、发布或部署行为。
- 不修改既有 Phase 2A expected、Oracle、断言或容差来制造通过。

## 公共契约变化

- 公共 Protobuf、数据库 schema 和外部 API 保持不变。
- 固定收益内核新增版本化曲线/Carry C ABI 函数与结构体，不破坏既有 bond analytics v1 布局或符号。
- Rust 新增 provider-neutral 的内部领域输入/结果、Application port 和独立 Artifact schema；公共语义固定为 CFETS 风格到期收益率线性插值及上述持有期分解。

## 需 Human 决策

- 当前无待决项。若实现证据表明 CFETS 期限—YTM 线性插值无法支持可重放的 Carry/Roll-down 分解，或需要改变既有 Phase 2A 约定/容差/公共合同，必须停止并返回 Human 决策，不得静默替换为其他曲线口径。

## 最终真实测试证据

- 独立 Oracle：`python -m pytest tests/oracle/china-rates/test_phase2b_manual_oracle.py -q`，exit 0，4/4；官方 `QuantLib==1.42.1` test-only binary 输出与冻结文件精确一致，生产 native 对照 2/2。Phase 2B acceptance matrix：16/16，受保护输入、expected、Oracle、C++/Rust/Artifact/SIT 路径均由 SHA-256 fail closed。
- `./scripts/check-fast.ps1`：exit 0；包含 Phase 2B domain/native 回归及 storage library 3/3。
- `./scripts/check.ps1`：exit 0；严格 Clippy/build/test、生成契约 11/11、C++ CTest 6/6、Phase 2A matrix 36/36、Phase 2B matrix 16/16、Python 1/1、Web 4 files / 29 tests 全部通过。
- `./scripts/check.ps1 -IncludeIntegration`：exit 0；真实 PostgreSQL 16 + Ceph RGW 20.2.2 上 migration 4/4、Phase 1 业务闭环 1/1、负向不变量 13/13、Phase 2B `real_postgres_ceph_publish_restart_replay_and_tamper_fail_closed` 1/1。Phase 2B 用例验证正式发布、adapter 重建后的重放、确定性复算、size 篡改 fail-closed，以及 staging/orphan 均清零。
- 集成时发现并修复 Windows CRLF 导致 Ceph entrypoint `bash\r` 的可复现性缺陷；重建后 RGW healthy。验收完成后已删除仅属于 `ficant-phase2b` 的 2 个容器、2 个命名卷和网络，剩余容器/卷均为 0；测试数据不可恢复且不含共享或生产数据。

## 残余风险

- CFETS 实时到期收益率曲线适合作为中国国债可解释参考基准，但不是无套利贴现曲线；本迭代明确不把它冒充 bootstrap 后的即期/远期结构。
- Carry 未包含回购融资、税费、交易成本和再投资收益；这些因素在研究层加入前，结果只能解释为冻结口径下的未融资持有期分解。
- 单节点 Ceph 仅证明 S3 协议、持久发布和重放语义，不证明生产高可用、容量、性能或灾备；官方 QuantLib 1.42.1 仅是独立 test Oracle，不进入产品运行时依赖。
