# 本地开发说明

## 工具链最低要求

日常开发在 Windows 11 + PowerShell 7 上进行，无需启动外部数据库或对象存储即可完成全部编译、单元测试、盘中实时簿记（Desk Blotter）持久化验证与实时 SDK 联调（开发机检查工具族与最低版本，精确版本只在 CI 与发布侧锁定，见 [ADR-0020](architecture/adr/0020-dev-toolchain-minimum-versions.md)）：

- **PowerShell 7**（`pwsh`）
- **Rust `>= 1.96.0`**（根目录 [`rust-toolchain.toml`](../rust-toolchain.toml) 固定 CI 基准 `1.96.1`）
- **LLVM / Clang++ `>= 18.0`**、**CMake**、**Ninja**（编译 `cpp/fixed-income-kernel`）
- **Buf `>= 1.56.0`**（校验与构建 `interface/` Protobuf 契约及编码 `domain-packs/`）
- **uv `>= 0.7`** 与 **Python `3.12`**（运行 `python/` SDK、`python/tests` 及 `tests/oracle/china-rates/` 独立数值对账）

## BP-2 盘中实时簿记（Desk Blotter）架构与组件

在保持 BP-1 利率估值底座零外置依赖的前提下，BP-2 新增以下模块共同构成单进程可运行的盘中实时簿记后端：

- **`crates/ficant-domain`（L1）与 `crates/ficant-application`（L2）**：纯同步状态机、宽整数账本运算、编排用例与端口抽象，不依赖任何数据库、网络或异步运行时。
- **`crates/ficant-blotter-store`**（入口 [`crates/ficant-blotter-store/src/lib.rs`](../crates/ficant-blotter-store/src/lib.rs)）：单写者独占 WAL `SQLite` 持久化实现（基于 `rusqlite` 内嵌引擎，禁止 URI 打开并拒绝 `:memory:`、`file:` URI、空路径与目录；强制校验 `locking_mode = EXCLUSIVE`、`journal_mode = WAL`、`synchronous = FULL`、`foreign_keys = ON` 与业务版本字段 `blotter_meta.schema_version`；留存窗口裁剪仅修剪 `blotter_event_journal`），并提供四套开源参考 SPI 适配器（`Mock*` 与 `Csv*`）。
- **`crates/ficant-api`**：单端口双协议传输层（借助 `tonic` 与 `tonic_web::GrpcWebLayer` 在同一监听端口同时服务原生 HTTP/2 gRPC 与 HTTP/1.1 `gRPC-Web`），封装 `RatesAnalyticsService`、`DeskBlotterService`、四个版本化 SPI 服务端、双协议凭证校验（[`crates/ficant-api/src/auth.rs`](../crates/ficant-api/src/auth.rs)）及后台生命周期看门狗（`WatchdogController` / `spawn_blotter_watchdog`）。
- **`binaries/ficant-server`**（装配逻辑 [`binaries/ficant-server/src/lib.rs`](../binaries/ficant-server/src/lib.rs)）：单进程零外置依赖服务二进制，始终挂载 `ficant.rates.v1.RatesAnalyticsService` 并支持 `--health-check` TCP 探测；按环境变量装配存储模式、策略、凭证与 SPI，并在开始接收请求前执行 fail-closed 校验与启动期策略对账（`reconcile_startup_policies`）。

## 服务端存储运行模式与环境变量

### 三种存储运行模式（`BlotterStorageMode`）

`ficant-server` 在启动时采用 fail-closed 配置解析（见 [`binaries/ficant-server/src/lib.rs`](../binaries/ficant-server/src/lib.rs)）：

1. **`Disabled`（默认安全模式，BP-1 兼容）**：未设置 `FICANT_BLOTTER_DB_PATH` 且未启用 `FICANT_BLOTTER_EPHEMERAL_IN_MEMORY` 时自动进入。`RatesAnalyticsService` 与 `--health-check` 正常提供服务；`DeskBlotterService` 与四个 SPI 服务统一返回 `STORAGE_UNAVAILABLE`，且磁盘上**零文件创建**。
2. **`PersistentWal`（生产持久独占 WAL 模式）**：设置 `FICANT_BLOTTER_DB_PATH` 指向数据库文件路径时启用。必须同时配置 `FICANT_BLOTTER_CREDENTIALS_PATH`，否则 fail-closed 拒绝启动。
3. **`EphemeralInMemory`（显式隔离内存测试模式）**：将 `FICANT_BLOTTER_EPHEMERAL_IN_MEMORY` 设为 `1` / `true` / `TRUE` 时启用（布尔开关接受 `0`/`false`/`FALSE`/空串为禁用，其他值直接报错）；复用与生产完全相同的 DDL 与约束，同样强制要求配置 `FICANT_BLOTTER_CREDENTIALS_PATH`。

> **互斥保护**：同时设置 `FICANT_BLOTTER_DB_PATH` 且启用 `FICANT_BLOTTER_EPHEMERAL_IN_MEMORY` 会直接 fail-closed 拒绝启动。

### 环境变量一览

服务端运行时环境变量（全部定义于 [`binaries/ficant-server/src/lib.rs`](../binaries/ficant-server/src/lib.rs)）：

| 环境变量 | 默认值 | 说明 |
|---|---|---|
| `FICANT_SERVER_ADDR` | `127.0.0.1:50051` | 单端口监听地址（原生 gRPC + gRPC-Web 共用）；`--health-check` 亦读取该地址（若绑定 `0.0.0.0` 等未指定地址则回环探测 `127.0.0.1:<port>`） |
| `FICANT_BLOTTER_DB_PATH` | 未设置 | `PersistentWal` 模式的独占 WAL `SQLite` 文件路径（与 `FICANT_BLOTTER_EPHEMERAL_IN_MEMORY` 互斥） |
| `FICANT_BLOTTER_EPHEMERAL_IN_MEMORY` | `false` | 显式启用 `EphemeralInMemory` 模式（接受 `1`/`true`/`TRUE` 或 `0`/`false`/`FALSE`/空串；与 `FICANT_BLOTTER_DB_PATH` 互斥） |
| `FICANT_BLOTTER_POLICY_PATH` | 内嵌 Policy B | 外置盘中策略 JSON 文件路径（见下文「外置策略配置」） |
| `FICANT_BLOTTER_CREDENTIALS_PATH` | 未设置 | 启动期凭证摘要 JSON 文件路径（`schema_version: 1`）；启用 `PersistentWal` 或 `EphemeralInMemory` 时必需 |
| `FICANT_BLOTTER_EVENT_RETENTION_WINDOW` | `100000` | `blotter_event_journal` 保留的最近事件条数（`DEFAULT_EVENT_RETENTION_WINDOW`） |
| `FICANT_BLOTTER_WATCHDOG_INTERVAL_MS` | `5000` | 后台生命周期看门狗扫描间隔（毫秒）；设为 `0` 关闭周期扫描，仅响应显式触发 |
| `FICANT_SPI_POSITIONS_ACCOUNTS_CSV` | 未设置 | 底仓账户 CSV 路径（**必须与 `FICANT_SPI_POSITIONS_BALANCES_CSV` 成对同时设置**，否则 fail-closed 报错；均未设置时回退内置 `MockPositionBaseProvider`） |
| `FICANT_SPI_POSITIONS_BALANCES_CSV` | 未设置 | 底仓余额 CSV 路径（需与 `FICANT_SPI_POSITIONS_ACCOUNTS_CSV` 成对设置） |
| `FICANT_SPI_TRADE_FEED_CSV` | 未设置 | 成交回报源 CSV 路径（未设置时使用 `MockTradeFeedReconciler`） |
| `FICANT_SPI_QUOTES_CSV` | 未设置 | 行情流 CSV 路径（未设置时使用 `MockMarketQuoteStream`） |
| `FICANT_SPI_NLP_CSV` | 未设置 | NLP 模板解析 CSV 路径（未设置时使用 `MockNlpIntentParser`） |

本地检查与跨编译器门禁环境变量（[`scripts/check.ps1`](../scripts/check.ps1) / [`scripts/check-cross-clang.ps1`](../scripts/check-cross-clang.ps1)）：

| 环境变量 | 默认值 | 说明 |
|---|---|---|
| `FICANT_CXX` | 自动探测 VS LLVM `clang++.exe` 或 `clang++` | 覆盖 Windows 侧 C++20 编译器路径 |
| `FICANT_WSL_DISTRO` | `ficant-ubuntu-24.04` | 跨编译器原始数值比对使用的 WSL 发行版名称 |
| `FICANT_LINUX_CXX` | `/usr/bin/clang++-18` | WSL Linux 侧 `clang++` 路径 |
| `FICANT_REQUIRE_CROSS_CLANG` | 未设置 | 设为 `1` 时与 `-RequireCrossClang` 等效，Linux 编译器不可用时直接失败 |

### 外置策略配置（Policy B 默认 vs Policy A 运行时覆盖）

套保允许品种白名单、对账容差、预收盘告警时间、行情/回报陈旧阈值、账户限额上限与监管规则包绑定全部外置在严格模式（`deny_unknown_fields`、`schema_version: 1`）策略 JSON 中（加载器见 [`binaries/ficant-server/src/policy.rs`](../binaries/ficant-server/src/policy.rs)）：

- **Policy B（默认内嵌）**：[`binaries/ficant-server/policy/desk-blotter-policy-v1.json`](../binaries/ficant-server/policy/desk-blotter-policy-v1.json) 定义 `policy_profile: "B"`，默认 `allowed_hedge_product_codes` 为 `["T", "TF", "TL", "TS"]`（包含 `TS`），`clean_price_tolerance: "0.05"`，`yield_tolerance: "0.0005"`，`committed_overdue_seconds: 3600`，`pre_close_alert_time: "16:30:00"`，`feed_staleness_seconds: 300`，`default_quote_source_id: "primary-quote-feed"`，`quote_staleness_seconds: 300`，`default_draft_hold_ttl_seconds: 1800`。
- **Policy A / 定制策略（运行时覆盖）**：设置 `FICANT_BLOTTER_POLICY_PATH` 指向外部 JSON 文件即可无需改代码或重新编译切换策略（例如切换回保守收缩为 `["T", "TF", "TL"]` 的 `policy_profile: "A"`、调整 `allowed_hedge_product_codes`、通过 `accounts.<account_id>` 配置单账户覆盖、`limit_ceilings` 及经 SHA-256 验签的 `regulatory_rule_pack` 绑定）。服务端启动时会自动通过 `reconcile_startup_policies` 对已持久化账户同步策略状态。

### 双协议凭证校验（Native gRPC & gRPC-Web）

启用 Desk Blotter 存储后，所有 `DeskBlotterService` 与四个 SPI RPC 均通过 [`crates/ficant-api/src/auth.rs`](../crates/ficant-api/src/auth.rs) 执行统一元数据鉴权：

- **支持的请求头（二选一且唯一）**：
  - 主契约：`authorization: Bearer <token>`（必须严格包含 `"Bearer "` 前缀，且 `<token>` 非空、无 ASCII 空白或控制字符）；
  - 备用自定义头：`x-ficant-credential: <token>` 或 `x-ficant-credential: Bearer <token>`。
- **Fail-closed 拒绝条件**：缺少凭证头、同时携带两个头、同一请求头重复出现、令牌摘要不匹配、凭证已过期或被运行时吊销时，统一拒绝并返回 `UNAUTHENTICATED`。
- **摘要存储与启动校验**：凭证文件仅存放 64 位十六进制 SHA-256 摘要（`token_sha256_hex`）、角色（`TRADER` / `NLP_PARSER` / `FEED_ADAPTER` / `OPERATOR`）、`allowed_accounts`、`allowed_sources`、`expires_at_rfc3339` 与 `revoked`；服务端内存不存明文令牌，采用无早退常量时间比较（`constant_time_eq_32`），启动时直接拒绝空表、重复摘要、已吊销或已过期（`expires_at <= now`）条目。

## Python SDK 与实时子进程 E2E 测试

[`python/sdk/src/ficant_sdk/__init__.py`](../python/sdk/src/ficant_sdk/__init__.py) 导出同步客户端与事件归约工具：

- **`DeskBlotterClient`**：覆盖 `DeskBlotterService` 全部 18 个 RPC 与四个 SPI 服务，自动通过 `authorization: Bearer <token>` 附加凭证（支持 `insecure=True` 本地明文联调或 TLS `root_certificates`）；
- **`BlotterError`**：结构化业务错误封装（在 `ConfirmHold` 惰性过期提交返回 `reconfirmation_error` 时同步携带 `committed_seq` 及已提交状态视图）；
- **`BlotterEventApplier`**：整事务原子校验并应用快照，严格单调推进游标（配合 `BlotterEventStream`、`MarketQuoteStream`、`TradeFeedStream`）；
- **`RatesClient` / `RatesError`**：保持既有利率定价 SDK 兼容。

常用 Python 验证命令（均通过 `--locked` 锁定环境执行）：

```powershell
# 1. 契约描述符一致性与分层不变量（含目标过滤 normal 依赖闭包检查）
uv run --locked --project python python -m pytest -q python/tests/test_contract_import.py python/tests/test_blotter_contracts.py python/tests/test_layering_invariants.py

# 2. Desk Blotter 实时子进程 E2E（启动真实编译的 ficant-server 验证原生 gRPC、gRPC-Web、WAL 重启、鉴权、Policy A/B 与 SPI）
uv run --locked --project python python -m pytest -q python/tests/test_blotter_sdk_live.py

# 3. 全部 Python 契约与实时 SDK 测试
uv run --locked --project python python -m pytest -q python/tests
```

## 分层门禁与纯层依赖闭包校验

- **8 根目录静态分层门禁**：[`scripts/check-layering.ps1`](../scripts/check-layering.ps1) 在原有 7 个生产源码根目录基础上纳入第 8 根 `crates/ficant-blotter-store/src`（与 `cpp/fixed-income-kernel/{include,src}`、`crates/ficant-kernel-sys/src`、`crates/ficant-fixed-income-native/src`、`crates/ficant-domain/src`、`crates/ficant-application/src`、`crates/ficant-api/src` 并列），强制阻断硬编码市场分支、规则数值、发行人/期货品种字面分支，并要求 [`scripts/layering-allowlist.json`](../scripts/layering-allowlist.json) 保持空数组 `[]`。
- **跨运行时共享样例验证**：[`scripts/test-layering-check.ps1`](../scripts/test-layering-check.ps1) 与 [`python/tests/test_layering_invariants.py`](../python/tests/test_layering_invariants.py) 共用同一套正/负样例目录（覆盖第 8 根 `crates/ficant-blotter-store/src`）验证 PowerShell 与 Python 扫描器一致。
- **目标过滤已启用 `normal` 依赖闭包检查**：[`python/tests/test_layering_invariants.py`](../python/tests/test_layering_invariants.py) 基于 `cargo metadata` 对纯核心层（`ficant-domain`、`ficant-application`）计算面向当前宿主目标且仅计已启用特性的 `normal` 传递依赖闭包，禁止引入 `rusqlite`、`libsqlite3-sys`、`tokio`、`tokio-stream`、`tonic`、`tonic-prost`、`tonic-web`、`prost`、`prost-types`、`hyper`、`http`、`serde_json`、`ficant-blotter-store`、`ficant-contracts`、`ficant-api`（同时正确放行未启用可选依赖、弱特性 `dep?/feat`、非宿主目标依赖与 `dev-dependencies`）。

## 常用开发与本地门禁命令

查看检查步骤（不实际执行）：

```powershell
.\scripts\check-fast.ps1 -ListOnly
.\scripts\check.ps1 -ListOnly
```

提交前快速检查（[`scripts/check-fast.ps1`](../scripts/check-fast.ps1)：分层门禁、Buf lint、Rust 格式化/检查/全量测试、Python 契约与实时 SDK）：

```powershell
.\scripts\check-fast.ps1
```

完整本地回归（[`scripts/check.ps1`](../scripts/check.ps1)：含 8 根分层门禁与共享样例测试、Buf format/lint、RulePack 二进制漂移校验、严格 Clippy、Rust 全量构建与测试、C++ 内核 CTest、跨编译器数值一致性、全部独立 Decimal Oracle 及 Python 测试）：

```powershell
.\scripts\check.ps1
```

强制执行 Windows/WSL Linux 跨 Clang 位级数值一致性校验（WSL 发行版或编译器不可用时直接失败，而非跳过）：

```powershell
$env:FICANT_WSL_DISTRO = 'ficant-ubuntu-24.04'
.\scripts\check.ps1 -RequireCrossClang
```

专项基线与文档门禁：

```powershell
.\scripts\check.ps1 -Bp1ProtectedFinal   # 校验相对历史基线 8a917997 的 6 个受保护根及 8/8 白名单新文件
.\scripts\check.ps1 -Bp1Docs             # 校验定位文档规范术语及全部受追踪 Markdown 文件的本地链接零断链
```

> **本地检查与远端 CI 边界**：以上脚本均为本地开发门禁，其通过结果**不能**替代或等同于远端 Linux GitHub CI 与版本发布工作流的验证结论（见 [`AGENTS.md`](../AGENTS.md)）。

本地启动单进程服务（[`scripts/dev-up.ps1`](../scripts/dev-up.ps1)，默认 `Disabled` 模式监听 `127.0.0.1:50051`，可通过前述环境变量切换为 `PersistentWal` 或 `EphemeralInMemory`）：

```powershell
.\scripts\dev-up.ps1
```

## Protobuf 契约校验与 Strategy A 说明

修改 `interface/proto/**` 后执行格式与规范校验：

```powershell
buf format -w interface
buf lint interface
```

> **Strategy A `buf generate` 行为提示**：由于 [`interface/buf.gen.yaml`](../interface/buf.gen.yaml) 启用 `clean: true`，直接原地执行全量 `buf generate interface --template interface/buf.gen.yaml` 会重排既有 `ficant.market.v1.rs` 声明块并额外生成空存根 `fact_pb2_grpc.py`。本地检查与 CI 均以 `buf format --diff --exit-code`、`buf lint` 及 `buf build` 描述符镜像比对作为门禁，保持既有 `core`/`market`/`rates`/`research` 生成绑定零漂移；定向更新 `ficant.blotter.v1` 绑定及刷新 `ficant_descriptor.bin` 的操作步骤详见 [`interface/README.md`](../interface/README.md)。

## 交易台盘中副屏 Web 应用（`apps/desk-web`）

[`apps/desk-web`](../apps/desk-web/package.json) 是基于 React、严格 TypeScript 与 Vite 的盘中副屏单页应用，通过同源二进制 `gRPC-Web` 代理连接单进程 `ficant-server`（[`apps/desk-web/vite.config.ts`](../apps/desk-web/vite.config.ts)），访问凭证仅驻留内存，所有定价、税基、覆盖率、资金缺口、套保手数与核销分摊均由服务端计算。

### 前端工具链最低要求

- **Node.js `>= 22.13.0`**（含内置 `node:sqlite`，用于持久化 WAL 端到端回归测试）
- **pnpm `>= 11.0.0`**（独立前端工作区锁文件 [`apps/desk-web/pnpm-lock.yaml`](../apps/desk-web/pnpm-lock.yaml)，通过 `--frozen-lockfile` 安装）
- **Buf `>= 1.56.0`**（通过独立 Web 模板 [`apps/desk-web/buf.gen.web.yaml`](../apps/desk-web/buf.gen.web.yaml) 校验 `apps/desk-web/src/generated/**`）
- **已安装的本机 Chromium / Chrome / Edge 浏览器**（完整端到端测试与生产包挂载冒烟直接复用本机浏览器，不自动下载浏览器二进制）

### 前端环境变量

| 环境变量 | 默认值 | 说明 |
|---|---|---|
| `FICANT_UPSTREAM_ORIGIN` | `http://127.0.0.1:50051` | Vite `dev` / `preview` 代理 `^/ficant\.` 请求转发的目标 `ficant-server` 上游地址 |
| `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH` | 自动探测本机浏览器 | 显式指定无头浏览器可执行文件路径；未设置时依次查找已有 Playwright 本地缓存、本机 Chrome 或 Edge，缺失时直接报错退出 |

### 本地检查与端到端回归命令

[`scripts/check-desk-web.ps1`](../scripts/check-desk-web.ps1) 提供前端与真实服务端联调的统一本地入口（Playwright 配置见 [`apps/desk-web/playwright.config.ts`](../apps/desk-web/playwright.config.ts)，结果输出固定在 `apps/desk-web/coverage/playwright-results`）：

```powershell
# 查看完整检查计划或跳过真实服务 E2E 的轻量检查计划（不实际执行）
.\scripts\check-desk-web.ps1 -ListOnly
.\scripts\check-desk-web.ps1 -ListOnly -SkipE2E

# 轻量本地检查（冻结安装、代码生成校验、TypeScript、ESLint、Vitest 单元/组件测试、生产构建与路径范围校验；不探测 Rust，不启动浏览器）
.\scripts\check-desk-web.ps1 -SkipE2E

# 完整本地检查（额外执行生产包挂载冒烟、cargo check/build --locked -p ficant-server 与四组真实服务 Playwright E2E）
.\scripts\check-desk-web.ps1
```

单独运行前端开发服务或查看测试收集清单：

```powershell
# 核对 Vitest 与 Playwright 各自的非空互斥测试清单
pnpm --dir apps/desk-web exec vitest list --filesOnly
pnpm --dir apps/desk-web exec playwright test --config playwright.config.ts --list

# 启动生产构建预览（指向本地 ficant-server）
$env:FICANT_UPSTREAM_ORIGIN = 'http://127.0.0.1:50051'
pnpm --dir apps/desk-web run build
pnpm --dir apps/desk-web exec vite preview --host 127.0.0.1 --port 4173 --strictPort
```

> **本地检查与远端 CI 边界**：`scripts/check-desk-web.ps1` 的本地执行结果仅代表当前开发机上的验证状态，不能替代或等同于远端 Linux GitHub CI 与正式版本发布工作流的结论（见 [`AGENTS.md`](../AGENTS.md)）。
