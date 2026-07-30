# 后台接口与 Protobuf 合同

本目录是 ficant 后台跨边界合同的唯一来源。Rust、Python 和 TypeScript 生成物均由这里的同一份 descriptor 派生；各语言不得手写同名 DTO、Decimal、时间、错误或对象结构。

## 包与边界

- `ficant.core.v1`：ULID、版本引用、SHA-256、所有者、单位、Decimal、市场时间、血缘、分页与稳定错误。
- `ficant.market.v1`：Instrument、Bond、FuturesContract、Cashflow、Calendar、Unit、Quote、Trade、Valuation、CurveSnapshot、MarketRulePack，以及由 MarketRulePack 内容引用的强类型市场规则包。
- `ficant.research.v1`：DataSnapshot、UniverseSnapshot、ExperimentRun、Artifact、SignalSet、RunJournal。
- `ficant.app.v1`：Platform Shell 使用的 App Registry、Session 与短期应用启动授权；它们不计入 17 个 Phase 1 领域对象。
- `ficant.rates.v1`：Phase 2E 的固定收益参考分析调用合同；只承载强类型请求/结果并复用 `ficant.core.v1` 身份、Decimal、时间和错误，不把 Python 变成数值实现或控制平面。

`DecimalValue` 的唯一表示是 `coefficient(string) + scale(uint32) + UnitRef`。时间 instant 使用 Protobuf `Timestamp`，并显式携带 IANA 市场时区和本地交易日期。Valuation、CurveSnapshot 与 Cashflow 只登记外部输入事实和来源，不提供定价、曲线、现金流生成、久期、DV01 或其他 Phase 2 算法。

`MarketRulePack.content` 是加法式 `google.protobuf.Any`：`content_hash` 绑定其确定性 `value` bytes，而 `type_url` 只标识 L3 内容 schema。现有内容为 `CgbFuturesDeliveryRulePack`、`FundingRulePack` 与 `TaxRulePack`：前者由 `cgb-futures` adapter 解析，后两者分别按精确 Subject 的 `FundingTier` 与完整 VAT / income profile pair 解析；`TaxRulePack` 还校验 Bond 的首发日期间与券级税收属性。core 与通用 market 合同不解释其中的市场规则数值。

iteration-3 Phase 2A 生成的现金流、估值和风险结果保持内部 `BondAnalyticsResult` 语义，并以内容寻址 Artifact 绑定输入与算法版本；本小迭代不扩展公共 Protobuf，也不得将派生结果写入上述外部事实消息。依据见 `docs/architecture/adr/0002-fixed-income-kernel-and-ffi-safety-boundary.md`。

## 查询与平台安全切片

- Definition 支持按 ID + version 精确读取、按 UTC instant 解析 as-of 版本，以及用 cursor 稳定分页读取历史版本。
- Market Fact 支持按精确 Instrument version + 时间窗口分页查询，并支持按 ID 读取已发布 CurveSnapshot。
- Artifact 与 SignalSet 分别支持读取对象及其分页血缘；不暴露 MinIO 凭据或物理存储接口。
- `ficant.research.v1.ExperimentService.ReadNodeOutput` 是 Phase 5A 的加法式只读观测合同：只按 Run/Node 读取已持久化且经 Artifact/Ceph required-read、envelope hash 与 manifest 端口绑定共同校验的有界输出，不发布、修改或重新解释研究结果，也不暴露对象存储凭据。
- `ficant.app.v1.PlatformService` 固定为 Registry、当前会话、会话刷新/撤销、应用启动授权/刷新/撤销七个 RPC。启动授权只在成功分支返回短期 credential、服务端裁剪 scopes、精确 origin、CSP、sandbox 与签发/过期时间；安全失败统一走 `SafeError`。
- 为严格遵循 Interface 冻结的授权响应复用，Buf lint 仅对 `proto/ficant/app/v1/registry.proto` 忽略 `RPC_REQUEST_RESPONSE_UNIQUE` 和 `RPC_RESPONSE_STANDARD_NAME`；descriptor test 仍精确固定七个方法及输入/输出。

## 固定生成器

| 语言 | BSR remote plugin | revision | 上游与许可证 |
|---|---|---:|---|
| Rust message | `buf.build/community/neoeinstein-prost:v0.5.0` | 2 | `neoeinstein/protoc-gen-prost`；Apache-2.0；对应 prost 0.14.x。 |
| Rust gRPC | `buf.build/community/neoeinstein-tonic:v0.5.0` | 4 | `neoeinstein/protoc-gen-prost`；Apache-2.0；对应 tonic/tonic-prost 0.14.x。 |
| Python | `buf.build/protocolbuffers/python:v31.1` | 2 | `protocolbuffers/protobuf`；BSD-3-Clause。 |
| Python gRPC | `buf.build/grpc/python:v1.73.1` | 1 | `grpc/grpc`；Apache-2.0。 |
| TypeScript | `buf.build/bufbuild/es:v2.5.2` | 1 | `bufbuild/protobuf-es`；Apache-2.0；`target=ts`。 |

version 与 revision 同时固定，禁止省略版本、改用 `latest` 或浮动 branch。插件元数据以 Buf 1.56.0 的 `buf registry sdk info` 和 `bufbuild/plugins` 对应版本清单为来源。

## 验证合同

1. 在 Ubuntu 24.04 固定工具环境运行 `buf format --diff --exit-code interface` 与 `buf lint interface`。
2. 先把 `interface/buf.gen.yaml` 机械改写为 WSL 内部 `/tmp/ficant-task2d-*` 输出，连续生成两棵树并比较规范化 SHA-256；不得直接把未核对输出写入工作树。
3. 确定性成立后，只把三种语言的生成目录机械同步到其分配路径。
4. Rust descriptor inventory 必须从本目录重新构建 descriptor，验证 17 个对象、共享类型、字段类型、禁止浮点和平行包。
5. Python consumer 必须真实导入生成 message；若需要共享 Python runtime/test 依赖，只能由 W1 更新 `python/pyproject.toml` 与 `uv.lock`。
6. TypeScript 生成树在本任务验证确定性；consumer 编译由 W4 的 pnpm workspace/lock gate 完成。

首次合同前的 `main` 为 `42f570f309e20c867f65cffbce76e7f6d64d65d5`，该提交不存在 `interface/buf.yaml`，因此首次 breaking 状态只能是 `not-applicable-initial-contract-baseline`，不能记作通过。
