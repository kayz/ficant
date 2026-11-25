# ficant

> **开源的中国本币市场固收及衍生品「实时估值定价 + 盘中实时簿记（Desk Blotter）」内核**
> 面向金融机构前台交付「投资经理与交易台盘中副屏」，不承担柜台报单（OMS/EMS）、清算结算或中后台会计总账。

## 核心底座现状（归零收敛基线）

本仓库已收敛为**零外部重型基础设施依赖**（无需 PostgreSQL 或 Ceph 对象存储即可直接运行与验证）的纯净核心起点：

- **C++20 固收数值内核 + 安全 Rust FFI 封装**（`cpp/fixed-income-kernel`、`crates/ficant-fixed-income-native`）：固定利率与贴现国债现金流、应计利息、净价/全价、到期收益率、麦考利/修正久期、凸性、DV01、参考曲线插值、持有期 Carry / Roll-down、国债期货可交割券转换因子/基差/净基差/IRR/CTD 及 DV01 套保手数。
- **中国本币制度规则包（RulePack）及失败关闭解析器**（`domain-packs/`、`crates/ficant-*-pack`）：中金所 `TS/TF/T/TL` 交割细则、国债利息增值税新老划断（2025-08-08）双口径税则、DR/R 机构资金分层、监管限额定义。
- **核心领域模型**（`crates/ficant-domain`）：市场工具与日历、机构主体（`Subject` 版本化身份 + `SubjectStateSnapshot` 额度状态）、全局因子与关键期限 DV01（`FactorId`）、持仓会计三态（`AC / FVOCI / FVTPL`）与回购质押穿透语义。
- **单进程轻量服务端、内嵌副屏与可恢复 Sidecar**（`binaries/ficant-server`、`apps/desk-web`、`interface/`、`python/sdk`）：单一二进制在同一监听端口同时提供原生 HTTP/2 gRPC、浏览器 `gRPC-Web`、免鉴权 `/healthz` 与可选内嵌静态副屏，内置 `--sqlite-rollover` / `--verify-sqlite-archive` 离线换库审计命令，并配套 `ficant-sidecar`（`quotes` / `trade-feed` / `nlp-draft`）Python 参考客户端。
- **独立数值对账基准**（`tests/oracle/china-rates`、`tests/golden-cases/china-rates`）：Python 高精度 Decimal 与 QuantLib 独立 Oracle 黄金样例。

## 下一步建设主航道

1. **盘中实时簿记（Desk Blotter，盘中真实底账）**：锚定交易系统账面底数，支持多人低摩擦占仓标记、入库自动冲销核销与 T+0/T+1 资金头寸推算。
2. **本币实时多曲线与全谱系定价扩展**：由盘中实时锚点驱动动态曲线与持仓定制曲线，逐步扩展人民币利率互换（IRS）、政金债与含权债估值。
3. **盘中交易方案推演、半程敞口风控与收盘前多维归因**：服务投资经理副屏决策闭环，后续支持外部投研因子与策略挂载至实时监控。

## 快速开始

```powershell
.\scripts\check-fast.ps1          # 快速检查（分层门禁、Buf、Rust 全量测试、Python 实时 SDK）
.\scripts\check.ps1               # 完整本地回归（含严格 Clippy、C++ 内核测试、跨编译器数值一致性与独立 Oracle）
.\scripts\dev-up.ps1              # 编译并启动单进程 ficant-server（默认 127.0.0.1:50051）
.\scripts\package-intranet.ps1    # 打包内嵌前端资产的内网单文件 ficant-server 及 .sha256 清单
.\scripts\check-bp4-intranet.ps1  # 双构建隔离实证与单文件无代理 Chromium 跨日 E2E 验收
```

详细本地工具链与开发命令见 [`docs/development.md`](docs/development.md)；内网单文件部署、同源 HTTPS、角色 ACL 与离线换库手册见 [`docs/delivery/intranet-single-binary.md`](docs/delivery/intranet-single-binary.md)；协作与发布边界见 [`AGENTS.md`](AGENTS.md)。

## 许可证

源代码采用 [MIT License](LICENSE)。
