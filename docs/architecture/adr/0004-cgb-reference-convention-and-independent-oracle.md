# ADR-0004：中国记账式国债参考约定与独立 Oracle

- 状态：Accepted
- 日期：2026-07-13
- 决策者：Human，经 Orchestrator 的 Product/Architecture lens 形成方案

## 背景

iteration-3 Phase 2A 需要以六只真实中国记账式国债为基础，验证现金流、应计利息、净价、全价、到期收益率和风险指标。债券发行条款、市场日历、收益率口径、结算规则和数值工具并不是同一个概念；若依赖未声明的“市场默认值”或 QuantLib 默认值，结果将无法稳定重放，第三方类型与约定也会渗入领域层。

中国人民银行 2023 年第 12 号公告已经废止早期银行间债券到期收益率计算标准，因此本决策不把历史公式描述为当前唯一监管算法。财政部发行文件是本息日期和“节假日顺延”的条款来源；中国外汇交易中心公开资料用于佐证本组参考用例采用 T+1 结算。具体计算规则由版本化 `MarketRulePack` 明示。

## 决策

### 定位与身份

- 冻结项目参考约定 `cgb-reference-v1`。它用于确定性比较、测试与重放，不宣称覆盖全部中国债券品种、场所或当前市场惯例。
- 约定、日历、债券、估值时点、结算日、输入快照、算法、ABI 和 engine 都以精确版本进入输入血缘；缺少任何必需版本时 fail closed。
- 生产代码不得在运行时读取 QuantLib 的日历、约定或类型。`MarketRulePack` 是生产计算的唯一规则输入。

### 时间、日历与现金流

- 市场时区为 `Asia/Shanghai`。源数据 epoch 必须先转换为该时区的本地日期；禁止把 UTC 日期截断后当作中国市场日期。
- 六只核心 Golden Case 的估值时点为 `2026-07-13T15:00:00+08:00`，结算日为 `2026-07-14`（T+1）。估值时点只用于血缘；价格、应计和现金流所有权以明确结算日计算。
- 名义现金流日期从起息日、到期日和付息频率生成；规则化券期不启用月末规则，`end_of_month=false`。
- 参考日历标识为 `cgb-reference-calendar-v1`，以版本化数据保存在 `MarketRulePack`。当前精确覆盖区间为 `2005-01-01..2026-12-31`，采用冻结的中国银行间日历假日和调休工作日；2026 年条目须与国务院办公厅《2026 年部分节假日安排》交叉核对。生产代码不得运行时调用 QuantLib 日历。
- 对 `2027-01-01` 及以后尚无官方日历的数据，参考模式按“周六、周日非工作日，其余日期工作日”生成支付日，并在结果中写入 `calendar_resolution=PROVISIONAL_WEEKEND_ONLY` 和 `calendar_coverage_end=2026-12-31`。这不表示未来真实市场营业安排，也不缩短债券到期范围。
- 要求真实市场日历的调用若早于 `2005-01-01` 或晚于 `2026-12-31` 必须 fail closed，不能静默使用 provisional 规则。后续日历数据通过新的 RulePack/calendar version 发布；历史执行继续绑定旧版本，不回写、不自动升级。
- 支付日遇参考日历非工作日使用 `Following`；计息期仍止于名义日期，顺延期间不产生额外利息。
- 除息期为 0 天。结算日严格早于调整后支付日时包含该笔现金流；结算日等于或晚于调整后支付日时不包含。
- 票息与本金分别表达，最后一期支付包含最后票息及面值 100 的本金。

### 日数、价格与收益率

- 规则化固定利率附息债使用 `Actual/Actual (Bond/ISMA)`，参考期由名义票息日期确定。
- 贴现债只有面值 100 的到期兑付、无票息且应计为零；一年以内采用按自然年分段的 Actual/Actual 年分数和单利 `y = (100 / dirty_price - 1) / year_fraction`。
- 附息债名义收益率的复利频率与付息频率相同；年付为 annual，半年付为 semiannual。
- 利率在领域和计算边界内用小数表达；展示层才转换为百分比。净价、全价、应计和 DV01 均按 CNY 100 面值表达，并满足 `dirty_price = clean_price + accrued_interest`。
- C++ 内核只返回有限 IEEE-754 数值；safe Rust adapter 以小数点后 12 位、round-half-even 规范化数值后再参与 Artifact 规范序列化和哈希。

### 反解与风险指标

- 价格反解收益率使用 Brent 方法，初始区间为 `[-0.50, 1.00]`，最多 100 次迭代；价格残差和收益率区间的收敛阈值均为 `1e-12`。
- 非有限输入或输出、无法形成根区间、超出迭代上限和无法收敛必须返回稳定错误，不得返回近似成功或切换 provider。
- 麦考利久期、修正久期和凸性以全价为分母。凸性定义为 `(1 / P_dirty) * d²P_dirty/dy²`，单位为年平方。
- DV01 按每 100 面值的正数表达，定义为 `(P_dirty(y - 1bp) - P_dirty(y + 1bp)) / 2`；独立有限差分验证使用相同的正负 1bp 冲击。

### 六只核心业务用例输入

以下收益率是专为确定性测试选取的合成输入，不表示 2026-07-13 市场行情：

| 债券 | 类型/频率 | `YIELD_IN` |
|---|---|---:|
| `269937.IB` | 182 天贴现债 | `0.0110` |
| `260013.IB` | 2 年，年付 | `0.0130` |
| `260011.IB` | 3 年，年付 | `0.0138` |
| `260008.IB` | 5 年，年付 | `0.0155` |
| `260012.IB` | 7 年，年付 | `0.0165` |
| `260010.IB` | 10 年，半年付 | `0.0180` |

每只债券必须形成一条 `YIELD_IN` 和一条 `PRICE_IN` 用例。`PRICE_IN` 使用独立 Oracle 根据对应合成收益率生成并冻结的完整精度净价，禁止从被测生产 C++ 输出反写 expected。

### 独立 Oracle 与容差

- Oracle 使用官方 QuantLib `1.42.1` 源码发布版本；源码摘要、构建镜像/工具链、编译参数、Oracle 程序、输入和输出都必须记录 SHA-256。
- QuantLib 只存在于测试工具链，不链接生产库，不提供生产 fallback，也不向 Domain、Application、Artifact schema 或公共契约暴露类型。
- 每个 Golden Case 同时使用手工现金流/现值公式、价格与收益率双向闭合及正负 1bp 有限差分复核。被测生产实现与 Oracle 不共享计算代码。
- 日期、现金流顺序/数量、票息/本金、错误类别、单位、版本和血缘要求完全一致。
- 初始数值容差：价格和应计的绝对误差 `1e-8`/每 100 面值；收益率绝对误差 `1e-10`；久期、凸性采用相对误差 `1e-8` 且绝对下限 `1e-10`；DV01 绝对误差 `1e-8`/每 100 面值；有限差分关系相对误差 `1e-4`。
- Quality 可以在生产实现开始前通过独立 Oracle 试算提出收紧容差的测试方案；Human 冻结业务 expected、Oracle 引用和容差。不得在观察生产结果后静默放宽；任何放宽都必须形成新的风险说明并由 Human 接受。

### Oracle 的模型与批准边界

- 独立 Oracle、业务 expected 和容差由 Quality 设计测试表达并提交证据，但其业务含义和最终接受属于 Human；任何 Test Worker 都不得制定或批准。实现 Oracle/expected 候选的 Worker 必须使用 checklist 指定的强模型，Quality 以独立上下文复核后提交 Human 冻结。
- `GPT-5.3-Codex-Spark` 只可在已冻结 Oracle、expected 和容差之后，机械实现 fixture、dataset manifest、automation mapping、测试执行与结构化报告；不得制定、改写或批准 Oracle、expected、容差和业务规则。
- 若测试资产工作暴露数值歧义、跨模块根因、FFI/安全/事务/恢复问题或需要改变冻结边界，Worker 必须停止并返回 Orchestrator；需要改变业务语义时再由 Human 决策。不能让 Spark 通过调整 expected 或断言继续执行。
- 模型输出不是正确性来源。正确性由版本化源数据、独立 QuantLib/手工公式/双向闭合/有限差分 Oracle、候选绑定执行证据、Quality test report 和 Human 接受共同建立。

## 模块边界

- Domain 拥有约定标识、精确十进制输入、业务单位、输入/结果不变量和 provider-neutral 错误语义。
- Application 拥有 exact Bond/RulePack/Snapshot proof 的解析顺序、`BondAnalyticsEngine` port、用例和 Artifact 发布意图。
- `ficant-fixed-income-native` 吸收浮点规范化、ABI/单位映射和 C++ 错误翻译。
- `ficant-kernel-sys` 独占 unsafe FFI；C++ 内核只拥有现金流、现值、收益率求根和风险数值算法。
- 强模型 Test Worker 可以实现独立 Oracle/expected 候选；Spark Test Worker 可以消费已冻结合同实现 fixtures 和 API/SIT harness；Quality 复核测试表达并提交报告，Human 冻结业务语义和接受结果。Oracle 代码不得被生产模块依赖。

## 依据

显式的项目约定把外部市场复杂性压缩进 `MarketRulePack`，把供应商复杂性压缩进测试 Oracle，把 FFI 与浮点规范化压缩进 adapter。上层只接触稳定的债券分析语言和版本血缘，符合 ADR-0003“复杂性封装在模块内部”的原则。

## 被否决方案

1. **把历史人民银行收益率公式声明为当前唯一标准。** 相关早期公告已经废止，声明范围超过证据。
2. **采用 QuantLib 默认约定作为生产定义。** 会让版本升级和供应商类型控制领域行为。
3. **直接使用测试日的市场估值作为唯一 expected。** 无法独立控制输入和复核算法，也会引入行情授权与时间依赖。
4. **从被测 C++ 生成 Golden expected。** 只能形成绿灯闭环，不能证明业务计算正确。

## 来源

- 中国人民银行公告〔2023〕第 12 号：`https://wuhan.pbc.gov.cn/chubanwu/114566/114579/4940535/5093963/2023101214213262517.pdf`
- 中国外汇交易中心/中国货币网债券收益率曲线：`https://www.chinamoney.com.cn/chinese/bkcurvrty/`
- 国务院办公厅 2026 年部分节假日安排：`https://www.gov.cn/zhengce/zhengceku/202511/content_7047091.htm`
- 财政部 2026 年记账式附息（十期）国债通知：`https://bgt.mof.gov.cn/zhuantilanmu/rdwyh/czyw/202606/t20260612_3991547.htm`
- 财政部 2026 年记账式附息（十二期）国债通知：`https://m.mof.gov.cn/tzgg/202606/t20260630_3992526.htm`
- 财政部 2026 年记账式贴现（三十七期）国债通知：`https://zwgls.mof.gov.cn/ywgg/202606/t20260610_3991431.htm`
- QuantLib 官方仓库：`https://github.com/lballabio/quantlib`
