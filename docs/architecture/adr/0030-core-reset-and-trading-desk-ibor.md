# ADR-0030：归零收敛核心底座，转向「投资经理与交易台盘中副屏 + 盘中实时簿记（Desk Blotter）」

- 状态：Accepted
- 日期：2026-11-19
- 决策者：Human
- 取代 / 归档：ADR-0001、ADR-0005、ADR-0006、ADR-0007、ADR-0008、ADR-0009、ADR-0010、ADR-0014、ADR-0016、ADR-0017、ADR-0018、ADR-0022、ADR-0023、ADR-0024、ADR-0025、ADR-0027、ADR-0028、ADR-0029；同时归档四个历史衍生仓库（`kayz/ficant-authority`、`kayz/ficant-fixin`、`kayz/ficant-portfolio`、`kayz/ficant-research`）

## 背景

早期迭代中，`ficant` 同时尝试建设重型离线投研基础设施（PostgreSQL + Ceph RGW 对象存储、Worker 队列、有向实验图、K 线回放及多套策略前端 `web-dm`），仓库膨胀至约 25 万行。

从机构固收自营与资管投资经理的真实工作场景审视（桌面上已具备恒生 O32、衡泰 xIR、万得终端、森浦 QB 行情及自动做市报价工具）：

1. **T-1 盘后信息充分的批处理问题**（如质押券盘后排券、清算交收、总账估值核算）早已被恒生 O32 与衡泰 xIR 覆盖，重复建设无落地价值；
2. 投资经理盘中真正的真空地带在于**「盘中动态变化的行情 × 盘中尚未入库稳态的真实意图与持仓」**：
   - 多人协同口头/IM 达成但尚未登记 O32 的真实可用持仓（占券 Earmark）与盘中真实 DV01 / 资金缺口；
   - 结合本账户真实持仓定制的盘中实时动态曲线与流动性监测；
   - 交易决策执行前后的成交后敞口推演（What-If）、半程敞口补对冲建议；
   - 收盘前按团队成员、账户或策略维度的实时盈亏归因。

## 决策

1. **战略聚焦（第一阶段切入点）**
   - `ficant` 定位为开源的中国本币市场固收及衍生品**「实时估值定价 + 盘中实时簿记（Desk Blotter）」**内核，直接驱动金融机构交易内网中的**「投资经理与交易台盘中副屏」**。
   - **T-1 充分性剃刀**：凡在 T-1 收盘时信息已完备的静态批处理工作一概不做；只解决盘中动态行情与盘中非稳态持仓/交易意图交叉的实时问题。不承担柜台报单（OMS/EMS）、清算结算或中后台会计总账。
   - 第二阶段再支持将外部投研因子与策略挂载至实时市场与持仓监控中，不在一期自建离线因子库或通用回测平台。

2. **架构归零收敛**
   - 移除 PostgreSQL / Ceph 对象存储适配器、数据库 migration、`ficant-worker`、`ficant-bootstrap`、离线实验图、旧版 K 线回放、冻结的 `PortfolioWorkbench` 以及旧策略前端目录 `web-dm/`，前端技术栈与端到端交互将围绕「交易台盘中副屏」重新设计。
   - 100% 保留经过独立高精度 Decimal Oracle 与 QuantLib 1.42.1 对账的核心资产：
     - `cpp/fixed-income-kernel/`（C++20 债券现金流/YTM/久期/凸性/DV01、参考曲线插值、Carry/Roll-down、国债期货交割篮子/CTD、DV01 套保比例内核）与安全 Rust FFI 封装；
     - `domain-packs/` 与 `crates/ficant-*-pack`（中金所国债期货交割细则、2025-08-08 国债利息增值税新老划断税则、机构资金分层、监管限额规则包）；
     - `crates/ficant-domain` 核心原语（12 位定点小数、分层法则、Subject 身份与额度快照、全局 FactorId、会计分类显式三态与正逆回购穿透语义）；
     - 单进程无外部存储依赖的 `binaries/ficant-server`（同一端口同时承载原生 gRPC 与 `gRPC-Web`）与 `python/sdk`。

3. **仓库主权收口**
   - 原四个外围/治理分支仓库（`kayz/ficant-authority`、`kayz/ficant-fixin`、`kayz/ficant-portfolio`、`kayz/ficant-research`）统一在 GitHub 归档（Archive）并清理本地副本；`kayz/ficant` 主仓库成为唯一活跃源码、契约与文档权威来源。

## 后果

- 仓库净删减约 24.5 万行历史包袱，Rust 工作区收敛至约 1.8 万行，任意开发机无需启动外部数据库或对象存储即可在数秒内完成编译、单元测试与实时服务联调。
- 后续新功能（实时影子持仓账本（盘中实时簿记 / Desk Blotter）、多人语音/一句话占券与自动冲销、盘中动态曲线、What-If 风控与收盘前归因、新一代副屏前端）均在此纯净基线上演进，并继续严格遵守 [ADR-0013 分层法则](0013-layering-law-shape-in-core-content-in-rulepack.md)。
