# Ficant Protobuf 契约目录

本目录是 Ficant 跨语言（Rust / Python，后续可扩展新前端客户端）后台协议的唯一权威来源。

## 包结构

- `ficant.core.v1`：基础十进制数、时间、标识符、错误详情与机构主体（`Subject` / `SubjectStateSnapshot`）。
- `ficant.market.v1`：债券、国债期货、交易日历、行情事实及各类市场规则包（`TaxRulePackV2`、`CgbFuturesDeliveryRulePack`、`FundingRulePack`、`RegulatoryRulePack`）。
- `ficant.research.v1`：全局风险因子（`FactorDefinition`）、关键期限 DV01 敞口与持仓会计三态视图（`Position`）。
- `ficant.rates.v1`：无外部存储依赖的本币利率定价服务（`RatesAnalyticsService`）。

## 校验与代码生成

```powershell
buf format -w interface
buf lint interface
buf generate interface --template interface/buf.gen.yaml
```
