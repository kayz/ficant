# ADR-0026：监管规则包与约束集

- 状态：Accepted
- 日期：2026-10-24
- 决策者：Human（2026-10-24 在对话中确认：先建 RegulatoryRulePack；首批度量为单券持仓面额、组合总面额、借入资金；解析器放在新 crate `ficant-regulatory-pack`，R5D 的工作区冻结由 18 个包改为 19 个；契约与回放执行分两个 PR）
- 范围：SPEC v1.4 §4.2 二期 L1 对象「Constraint」；为 AC42（回放中的约束）与 AC20（约束占用）提供约束定义
- 关联：ADR-0012（Subject 与状态快照）、ADR-0013（分层法则）、ADR-0025（回放账本）

## 背景

ADR-0012 已经定下约束的分工：

- `SubjectVersion` 绑定一个约束集（`ConstraintSetRef`，引用监管规则包，不内嵌数值）；
- `SubjectStateSnapshot` 携带各项额度的上限（`LimitCeiling`：`limit_code` + 数值）；
- 额度占用由持仓计算。

缺的是中间那一层：某个 `limit_code` **量的是什么**。没有它，上限只是一串没有含义的数字，回放无法判断一笔成交是否突破约束（AC42）。

## 决策

### 1. 形状在 L1，内容在 L3

- **L1（`ficant_domain::constraint`）** 定义约束的形状：`ConstraintMeasure` 枚举、`ConstraintDefinition`（`limit_code` + 度量）、`ConstraintSet`（规则包的精确版本 + 定义列表），以及把约束集与状态快照结合成带上限的 `BoundConstraint`。
- **L3（`RegulatoryRulePack`）** 定义某个市场或机构类型有哪些约束、各自用哪种度量。规则包不含任何上限数值；上限是主体状态，按 ADR-0012 放在 `SubjectStateSnapshot`。

### 2. 首批三种度量

| 度量 | 含义 |
|---|---|
| `INSTRUMENT_FACE` | 每个工具的 \|持仓面额\| |
| `PORTFOLIO_FACE` | Σ \|持仓面额\| |
| `BORROWED_CASH` | max(0, −现金)，即为持仓融入的资金 |

DV01、久期等风险度量要接定价内核，以后在同一枚举上追加。

### 3. 契约（`ficant.market.v1`，加法式）

```proto
message RegulatoryRulePack { repeated ConstraintDefinition constraints = 1; }
message ConstraintDefinition { string limit_code = 1; ConstraintMeasure measure = 2; }
enum ConstraintMeasure { UNSPECIFIED = 0; INSTRUMENT_FACE = 1; PORTFOLIO_FACE = 2; BORROWED_CASH = 3; }
```

- 作为 `MarketRulePack.content` 的强类型内容，`rule_type = "regulatory"`，`type_url = type.googleapis.com/ficant.market.v1.RegulatoryRulePack`。
- 解析时失败关闭：没有约束、`limit_code` 为空或重复、度量为 `UNSPECIFIED` 或未知值。

### 4. 解析与绑定

- 解析器 `RegulatoryRulePackV1Parser` 在新 crate `ficant-regulatory-pack`，与资金、税收、期货交割包一致（每类 L3 内容一个 crate）。
- Application 用例 `ResolveConstraintSet`：按 Subject 的 `ConstraintSetRef` 读取精确版本的 `MarketRulePack`，校验所有者、生效期、内容哈希、市场、规则类型与 `type_url`，再交给解析器。Subject 没有绑定约束集时失败关闭，错误指明缺的是约束集。
- `ConstraintSet::bind(&SubjectStateSnapshot)`：每条定义都必须在快照里找到同码的上限，上限不能为负，快照必须属于同一个 Subject；否则失败关闭。快照里多出的上限（其他用途）不影响绑定。

### 5. 工作区冻结 18 → 19

R5D/R6B 把工作区冻结在 18 个 Cargo 包，任何变动都需要架构决定。本 ADR 即该决定：新增 `ficant-regulatory-pack`，依赖为 `ficant-application`、`ficant-contracts`、`ficant-domain`，与其他规则包 crate 相同。随之更新的冻结点：

- `crates/ficant-contract-tests/tests/r5d_layer_dependencies.rs` 的包与依赖边清单；
- `.github/scripts/supply-chain.lock.json` 与 `license-inventory.lock.json` 的第一方包清单；
- `.github/scripts/tests/test_license_inventory_bindings.py` 的包数断言；
- `.github/scripts/verify-contract-generation.sh` 的 descriptor 哈希。

### 6. 本 ADR 不做的事

- 不在回放中执行约束（AC42，下一个 PR）。
- 不接入 gRPC 服务与服务端组装；`ficant-server` 暂不依赖新 crate。
- 不提交 `domain-packs/**` 规则包内容：哪些约束适用于哪类机构是规则内容，需要 Human 另行提供并审定。

## 代价

- 契约与第一方包清单的冻结点都要随新 crate 改动，改动面比功能本身大。
- 度量只有三种，风险类约束要等后续扩展。

## 替代方案

- **约束定义作为回放输入**：不改契约，但约束含义不进入规则包，违背 ADR-0012「约束集绑定监管规则包」。Human 选择先建规则包。
- **解析器放进 `ficant-funding-pack`**：不动冻结，但一个 crate 同时解析两类规则，名实不符。拒绝。
