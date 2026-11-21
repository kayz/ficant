# Ficant Protobuf 契约目录

本目录是 Ficant 跨语言（Rust / Python，后续可扩展新前端客户端）后台协议的唯一权威来源。

## 包结构

- `ficant.core.v1`：基础十进制数、时间、标识符、错误详情与机构主体（`Subject` / `SubjectStateSnapshot`）。
- `ficant.market.v1`：债券、国债期货、交易日历、行情事实及各类市场规则包（`TaxRulePackV2`、`CgbFuturesDeliveryRulePack`、`FundingRulePack`、`RegulatoryRulePack`）。
- `ficant.research.v1`：全局风险因子（`FactorDefinition`）、关键期限 DV01 敞口与持仓会计三态视图（`Position`）。
- `ficant.rates.v1`：无外部存储依赖的本币利率定价服务（`RatesAnalyticsService`）。
- `ficant.blotter.v1`：盘中实时簿记（Desk Blotter）与版本化 SPI 契约，共 **5 个服务 / 25 个 RPC（含 3 个服务端流式 RPC）/ 18 个枚举 / 100 个消息**，拆分为三个文件：
  - [`proto/ficant/blotter/v1/model.proto`](proto/ficant/blotter/v1/model.proto)（17 个枚举、24 个消息）：开盘底仓与工具目录、T+0/T+1 持仓分桶视图、占仓（`HOLD` / `COMMITTED`）与草案（`DraftEarmarkRecord`）、待核销队列、风险覆盖与资金缺口汇总、源健康视图及事件流封装。
  - [`proto/ficant/blotter/v1/spi.proto`](proto/ficant/blotter/v1/spi.proto)（1 个枚举 `TradeFillOutcomeKind`、23 个消息、4 个 SPI 服务 / 7 个 RPC）：
    - `PositionBaseProviderService`（1 个 RPC：`FetchOpeningPositionBase`）
    - `TradeFeedReconcilerService`（3 个 RPC：`PullTradeFeedBatch`、流式 `StreamTradeFeed`、`FetchTradeRevision`）
    - `MarketQuoteStreamService`（2 个 RPC：`PullMarketQuotes`、流式 `StreamMarketQuotes`）
    - `IntranetNlpIntentParserService`（1 个 RPC：`ParseNlpIntent`）
  - [`proto/ficant/blotter/v1/service.proto`](proto/ficant/blotter/v1/service.proto)（53 个消息、1 个主服务 / 18 个 RPC）：
    - `DeskBlotterService`（17 个一元 RPC + 流式 `SubscribeEvents`；`ImportOpeningBaseRequest` 保留 `reserved 4, 5`）。

## 代码生成产物与共享描述符一致性测试

[`buf.gen.yaml`](buf.gen.yaml) 锁定远端插件版本与修订号，生成双语言绑定：

- **Rust（`neoeinstein-prost` + `neoeinstein-tonic`）**：输出至 [`crates/ficant-contracts/src/generated`](../crates/ficant-contracts/src/generated)（如 `ficant.blotter.v1.rs`、`ficant.blotter.v1.tonic.rs`），由 [`crates/ficant-contracts/src/lib.rs`](../crates/ficant-contracts/src/lib.rs) 统一导出。
- **Python（`protocolbuffers/python` + `grpc/python`）**：输出至 [`python/node-contracts/src/ficant_contracts/generated`](../python/node-contracts/src/ficant_contracts/generated)（如 `ficant/blotter/v1/{model,spi,service}_pb2{,_grpc}.py`）。

双语言契约均以共享冻结描述符镜像 [`crates/ficant-contracts/tests/ficant_descriptor.bin`](../crates/ficant-contracts/tests/ficant_descriptor.bin)（由 `buf build interface --exclude-source-info` 生成）为权威基线执行跨语言一致性验证：

- **Rust 描述符与契约校验**：[`crates/ficant-contracts/tests/blotter_contracts.rs`](../crates/ficant-contracts/tests/blotter_contracts.rs)（将 `ficant_descriptor.bin` 与实时 `buf build` 逐字节比对，并断言全部 5 个服务 / 25 个 RPC / 18 个枚举 / 100 个消息、字段编号/类型/显式 `oneof` 与 `reserved 4, 5`）。
- **Python 描述符与导入校验**：[`python/tests/test_blotter_contracts.py`](../python/tests/test_blotter_contracts.py)（基于同一 `ficant_descriptor.bin` 校验 Python 生成物与描述符逐项一致）与 [`python/tests/test_contract_import.py`](../python/tests/test_contract_import.py)。

## 校验与代码生成（Strategy A）

日常校验命令（与本地门禁及 CI 保持一致）：

```powershell
buf format --diff --exit-code interface
buf lint interface
buf build interface --exclude-source-info --output crates/ficant-contracts/tests/ficant_descriptor.bin
```

> **Strategy A（既有非 `blotter` 生成物零漂移约定）**：
> 在 [`buf.gen.yaml`](buf.gen.yaml) 启用 `clean: true` 时，直接原地执行全量 `buf generate interface --template interface/buf.gen.yaml` 会引发两项与业务无关的产物变动：
> 1. `neoeinstein-prost` 会按包名字典序重排既有 `crates/ficant-contracts/src/generated/ficant.market.v1.rs` 中的声明块顺序（因 `blotter < market`）；
> 2. `grpc/python` 会在 `python/node-contracts/src/ficant_contracts/generated/ficant/market/v1/` 下额外生成一个空的占位存根 `fact_pb2_grpc.py`。
>
> 为避免无谓噪音并保持既有 `core` / `market` / `rates` / `research` 生成绑定**零字节漂移**：
> - 本地脚本与 CI 门禁通过 `buf format --diff --exit-code`、`buf lint` 及 `buf build` 描述符镜像逐字节比对验证契约，不以原地全量重写作为门禁步骤；
> - 修改 `ficant.blotter.v1` 原型时，请结合 `--path interface/proto/ficant/blotter/v1` 定向生成至隔离输出目录后仅同步 `blotter.v1` 绑定（注意 `buf.gen.yaml` 中 `clean: true` 会先清空目标目录，切勿直接对现存 `generated/` 目录盲目带 `--path` 覆盖），或全量生成后逐项审阅 `git diff` 并还原无关的 `core`/`market`/`rates`/`research` 改动与空 `fact_pb2_grpc.py`，随后同步刷新 `crates/ficant-contracts/tests/ficant_descriptor.bin` 并运行 Rust/Python 契约测试：

```powershell
cargo test --locked -p ficant-contracts
uv run --locked --project python python -m pytest -q python/tests/test_contract_import.py python/tests/test_blotter_contracts.py
```

更多本地开发、服务端运行模式与完整门禁说明见 [`docs/development.md`](../docs/development.md)。
