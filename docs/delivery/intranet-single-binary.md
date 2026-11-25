# 内网单文件部署、角色运维与离线换库手册

本手册面向机构交易内网运维与开发团队，规范 `ficant` **盘中实时簿记（Desk Blotter）**单二进制交付物（`ficant-server`）的本地打包、完整性校验、同源 HTTPS 接入、四角色最小权限与文件系统 ACL、盘后受控停机与离线换库审计、维护待决故障恢复、次日底仓水位衔接及历史归档恢复操作。

## 治理与发布边界

- **本手册覆盖范围**：内网单文件构建打包、目标机二进制完整性核验、回环联调与非回环同源 TLS 反向代理配置、最小权限凭证与目录 ACL、离线 SQLite 归档/重置/复核与故障恢复、次日开账与历史归档恢复。
- **与正式发布合同的边界**：正式版本候选触发（由 Human 创建指向 `main` 精确提交的不可变 `v*` tag）、GitHub Linux Runner 版本 CI、GHCR 不可变镜像（`ghcr.io/kayz/ficant-server:sha-<40位CommitSHA>`）、SBOM/SLSA/Trivy 扫描及 Linux 测试机 Compose 运行约定，仍由 [`docs/delivery/test-environment.md`](test-environment.md) 与 [`AGENTS.md`](../../AGENTS.md) 独占管理。
- **本地门禁与远端 CI 边界**：[`scripts/package-intranet.ps1`](../../scripts/package-intranet.ps1)、[`scripts/check-bp4-intranet.ps1`](../../scripts/check-bp4-intranet.ps1) 等本地脚本的执行结论仅代表本机验证结果，不得替代远端 Linux GitHub CI 或作为生产发布授权。

---

## 1. 单文件打包与完整性校验

### 1.1 Windows 签出换行前置要求

前端构建（`apps/desk-web/vite.config.ts`）对 `apps/desk-web/src/assets/rulepacks/cgb-futures-v2.manifest.json` 校验仓库原始 LF 字节的固定 SHA-256（953 字节）。在 Windows 上若 Git 开启了 `core.autocrlf=true`，该文件会被转换为 CRLF（981 字节）导致 `vite build` 失败。打包前须确保仓库使用 LF 签出：

```powershell
git config core.autocrlf false
git checkout -- apps/desk-web/src/assets/rulepacks/cgb-futures-v2.manifest.json
```

### 1.2 执行单文件打包（`package-intranet.ps1`）

使用 [`scripts/package-intranet.ps1`](../../scripts/package-intranet.ps1) 执行锁定离线单文件打包（默认输出目录 `target/intranet-package`，独立 Cargo 目标目录 `target/intranet-embed-release`）：

```powershell
# 先查看打包计划（不执行构建）
.\scripts\package-intranet.ps1 -ListOnly

# 执行单文件内嵌构建与 .sha256 生成
.\scripts\package-intranet.ps1
# 如需指定输出目录：
.\scripts\package-intranet.ps1 -OutputDir D:\ficant-release\pkg -CargoTargetDir D:\ficant-release\target
```

脚本按顺序完成：

1. 使用 `pnpm --dir apps/desk-web install --frozen-lockfile` 与 `pnpm --dir apps/desk-web run build` 生成前端产物；
2. 将 `apps/desk-web/dist` 复制到无重解析点的私有临时暂存目录，校验非空 `index.html` 与固定 159 字节 `/rulepacks/cgb-futures-v2.bin`（`SHA-256 = 8d8a0b944557cf87ed3d1f598ab3e02fa7f0aec125c1c26c17280acbd24367c5`）；
3. 解析 40 位小写十六进制 `FICANT_CODE_COMMIT_SHA`（`HEAD`）与 `FICANT_CODE_TREE_SHA`（`HEAD^{tree}`），设置 `FICANT_EMBED_WEB_DIST_DIR` 并运行 `cargo build --release --locked --offline --bin ficant-server`；
4. 输出 `ficant-server.exe`（Linux 为 `ficant-server`）及对应 `.sha256` 清单文件，并在 `finally` 中删除临时暂存目录与恢复环境变量。

### 1.3 目标机二进制与内嵌 RulePack 完整性核验

将 `ficant-server(.exe)` 与 `ficant-server(.exe).sha256` 传入内网主机后，启动前必须核对 SHA-256：

```powershell
# Windows (PowerShell 7)
$expected = ((Get-Content -LiteralPath .\ficant-server.exe.sha256 -Raw).Trim() -split '\s+')[0]
$actual   = (Get-FileHash -LiteralPath .\ficant-server.exe -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -cne $expected) { throw "SHA-256 mismatch: $actual != $expected" }
```

```bash
# Linux
sha256sum -c ficant-server.sha256
```

服务启动后（无需源码树或外部 `dist` 目录），可通过同端口 HTTP 独立核验内嵌交割规则包：

- `GET /rulepacks/cgb-futures-v2.bin` 必须返回精确 **`159` 字节**，SHA-256 严格等于 `8d8a0b944557cf87ed3d1f598ab3e02fa7f0aec125c1c26c17280acbd24367c5`，并携带 `X-Content-Type-Options: nosniff` 且无任何 `Access-Control-Allow-*` 响应头。

### 1.4 Web UI 四级优先级与 Fail-Closed 规则

服务端按严格四级优先级解析静态 Web UI 来源（实现见 [`binaries/ficant-server/src/web_ui.rs`](../../binaries/ficant-server/src/web_ui.rs)）：

1. **`FICANT_SERVE_WEB_UI=false` / `0`**（不区分大小写）：强制关闭 UI（`web_ui_mode = "disabled"`），优先级高于外部目录与内嵌资产；
2. **非空 `FICANT_STATIC_WEB_DIR`**：启动时一次性校验目录树并加载为只读内存 `WebAssetCatalog`（`web_ui_mode = "static_dir"`），上限 `2048` 个文件、`64 MiB` 总量；
3. **编译期内嵌资产表**（构建时设置了 `FICANT_EMBED_WEB_DIST_DIR`）：直接服务内嵌只读资产（`web_ui_mode = "embedded"`）；
4. **回退关闭**：以上均未提供时进入 `disabled`（所有静态路径 `GET`/`HEAD` 返回 `404`）。

Fail-closed 拒绝启动条件：非法布尔值（仅允许 `true`/`false`/`1`/`0`/空串）、纯空白 `FICANT_STATIC_WEB_DIR`、显式 `FICANT_SERVE_WEB_UI=true/1` 但无可用资产、目录或祖先路径包含符号链接或 Windows 重解析点（`0x0400`）、缺少非空 `index.html`、或 `/rulepacks/cgb-futures-v2.bin` 缺失/字节或 SHA-256 不匹配。资产一旦装入只读内存后，磁盘上的后续修改不会影响已提供响应。

### 1.5 `/healthz` 8 字段契约与 `--health-check`

`GET /healthz` 与 `HEAD /healthz` 免鉴权（其他 HTTP 方法统一返回 `405 Method Not Allowed` 且携带 `Allow: GET, HEAD`），响应不含任何宿主路径、凭证摘要、归档摘要或 `Access-Control-Allow-*` 头。JSON 固定包含 8 个字段：

```json
{
  "status": "ok",
  "service": "ficant-server",
  "storage_mode": "persistent_wal",
  "blotter_healthy": true,
  "ledger_epoch": 1,
  "ledger_instance_id": "01M8A1F3BGC74AHDSRHXHEFTNX",
  "web_ui_mode": "embedded",
  "embedded_assets_count": 4
}
```

| 字段 | 取值说明 |
|---|---|
| `status` | 存储健康时为 `"ok"`（HTTP `200`）；存储进入 halted 故障态时为 `"degraded"`（HTTP `503`） |
| `service` | 固定 `"ficant-server"` |
| `storage_mode` | `"disabled"` / `"persistent_wal"` / `"ephemeral_in_memory"` |
| `blotter_healthy` | 布尔值；`disabled` 模式下为 `true`，存储 halted 时为 `false` |
| `ledger_epoch` | 持久或内存模式下为正整数纪元；`disabled` 模式下为 `null` |
| `ledger_instance_id` | 26 位大写 Crockford Base32 ULID；`disabled` 模式下为 `null` |
| `web_ui_mode` | `"disabled"` / `"embedded"` / `"static_dir"` |
| `embedded_assets_count` | 编译期内嵌静态文件数量（无嵌入构建时为 `0`） |

本地或容器健康探针直接执行：

```powershell
$env:FICANT_SERVER_ADDR = '127.0.0.1:50051'
.\ficant-server.exe --health-check
```

`--health-check` 会将未指定监听地址 `0.0.0.0:<port>` 自动映射到 `127.0.0.1:<port>`、将 `[::]:<port>` 映射到 `[::1]:<port>`，在 **2 秒**有界超时内发起 `GET /healthz`，仅当 HTTP `200`、`status == "ok"` 且 `blotter_healthy == true` 时退出 `0`，否则退出非零。

---

## 2. 回环联调 vs 非回环内网同源 HTTPS 与流式透传

### 2.1 本机回环联调（`127.0.0.1` / `localhost`）

- 浏览器将 `http://127.0.0.1:<port>/` 视为安全上下文（Secure Context），前端单页应用可直接调用 `window.crypto.subtle`（Web Crypto）完成规则包 SHA-256 验签。
- 单端口同时提供静态 SPA、`/healthz`、原生 HTTP/2 `gRPC` 与 HTTP/1.1 `gRPC-Web`，无需启动 Vite 或额外开发代理。
- 三个 Python Sidecar 仅在回环地址联调时使用 `--insecure` 参数。

### 2.2 非回环内网同源 HTTPS 强制要求

当交易员浏览器通过非回环局域网地址（例如 `https://blotter.desk.internal/`）访问时，**严禁**以明文 HTTP 或跨源拆分部署，原因如下：

1. **零 CORS 设计**：`ficant-server` 在 `/healthz`、静态资源、错误页与全部 gRPC/gRPC-Web 响应中均**不输出任何 `Access-Control-Allow-*` 响应头**；前端 SPA 与 `/ficant.*` API 必须位于完全相同的协议、主机与端口（Same-Origin）。
2. **浏览器 Web Crypto 安全上下文限制**：非回环 `http://` 来源下浏览器会禁用 `window.crypto.subtle`，导致前端无法执行规则包 SHA-256 校验与安全摘要计算。
3. **传输凭证保护**：`Authorization: Bearer <token>` 与 `x-ficant-credential` 不得在非回环内网中明文传输。

### 2.3 内网反向代理（TLS 终结 + 流式透传）配置要求

建议将 `ficant-server` 仅绑定本机回环 `127.0.0.1:50051`，前置内网 CA 签发证书的 L7 反向代理（如 Nginx / Envoy）对外暴露单一 HTTPS 来源，并满足以下全部硬性条件：

- **同源转发**：将静态路径（`/`、`/assets/*`、`/rulepacks/*`、`/healthz`）与 RPC 路径（`/ficant.*`）转发到同一 `ficant-server` 后端；**不得**注入宽泛的 `Access-Control-Allow-*` 响应头。
- **关闭流式缓冲**：`DeskBlotterService/SubscribeEvents` 与 SPI 服务端流要求逐帧实时送达，必须关闭代理响应与请求缓冲（如 Nginx `proxy_buffering off;`、`proxy_request_buffering off;`、`grpc_buffering off;`）。
- **空闲超时大于心跳周期**：`SubscribeEvents` 默认流心跳间隔为 **`15s`**（[`crates/ficant-api/src/blotter.rs`](../../crates/ficant-api/src/blotter.rs) `DEFAULT_STREAM_HEARTBEAT_INTERVAL`），代理读/写超时必须严格大于 `15s`（建议 `60s`–`300s`），避免长连接在静默期间被代理误断。
- **完整透传头与尾部帧**：透传 `Authorization`、`x-ficant-credential`、`x-ficant-expected-ledger-epoch`、`x-ficant-expected-ledger-instance`、`Content-Type`（`application/grpc` / `application/grpc-web+proto`）及 HTTP/2 / `gRPC-Web` trailers。
- **Sidecar TLS 连接**：非回环内网中 Sidecar 必须使用 `--endpoint https://<fqdn>:<port> --ca-cert <intranet-ca.pem>`（同时用于 gRPC 通道与 `/healthz` 探活），禁止组合 `https://` 与 `--insecure`。

Nginx 最小参考配置片段（按机构证书与上游端口调整）：

```nginx
server {
    listen 443 ssl http2;
    server_name blotter.desk.internal;

    ssl_certificate     /etc/ficant/tls/server.crt;
    ssl_certificate_key /etc/ficant/tls/server.key;
    ssl_protocols       TLSv1.2 TLSv1.3;

    # 1. 静态 SPA 与健康探针（与 RPC 同源）
    location / {
        proxy_pass http://127.0.0.1:50051;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_buffering off;
    }

    # 2. 浏览器 gRPC-Web 流量（HTTP/1.1，关闭缓冲，超时 > 15s 心跳）
    location ^~ /ficant. {
        if ($content_type ~* "^application/grpc($|\+)") {
            # 原生 HTTP/2 gRPC 客户端走 grpc_pass（或拆分到独立 h2 location）
            error_page 418 = @native_grpc;
            return 418;
        }
        proxy_pass http://127.0.0.1:50051;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_buffering off;
        proxy_request_buffering off;
        proxy_read_timeout 180s;
        proxy_send_timeout 180s;
    }

    # 3. 原生 HTTP/2 gRPC Sidecar / SDK 流量
    location @native_grpc {
        grpc_pass grpc://127.0.0.1:50051;
        grpc_read_timeout 180s;
        grpc_send_timeout 180s;
    }
}
```

---

## 3. 四角色最小权限与文件系统 ACL

### 3.1 四角色职责与权限隔离

`FICANT_BLOTTER_CREDENTIALS_PATH`（`schema_version: 1`）仅保存 64 位十六进制 `token_sha256_hex`、角色、账户白名单（`allowed_accounts`）、数据源白名单（`allowed_sources`）、到期时间（`expires_at_rfc3339`）与 `revoked` 标志。服务端内存不存明文令牌，采用常量时间比较，启动时对空表、重复摘要、已吊销或已过期条目直接 fail-closed。

| 角色 | 适用主体 | 允许范围与关键隔离约束 |
|---|---|---|
| `TRADER` | 浏览器交易台副屏用户 | 仅在浏览器内存中持有令牌；可查询快照/事件流、操作占仓（Hold/Committed）、确认 NLP 草案（`ConfirmDraftEarmark`）与处理挂起成交（`ResolvePendingMatch` / `ACCEPT_ORPHAN`），受 `allowed_accounts` 限制 |
| `OPERATOR` | 盘前开账与台账管理员 | 导入每日底仓（`ImportOpeningBase`）、配置成交源控制（`SetFeedControl`）与运维只读核对，受 `allowed_accounts` 限制 |
| `FEED_ADAPTER` | `ficant-sidecar quotes` 与 `trade-feed` | 调用 `IngestMarketQuotes` / `IngestTradeFeed`，同时受 `allowed_accounts` 与 `allowed_sources` 限制；默认 **0 次** `GetAccountSnapshot` 调用（仅在显式授权 `--allow-snapshot-discovery` 时启用） |
| `NLP_PARSER` | `ficant-sidecar nlp-draft` | **结构级双方法隔离**：[`NlpDraftOnlyClient`](../../python/sdk/src/ficant_sdk/sidecar/nlp_gateway.py) 在 gRPC 通道上仅注册 `/ficant.blotter.v1.IntranetNlpIntentParserService/ParseNlpIntent` 与 `/ficant.blotter.v1.DeskBlotterService/SubmitDraftEarmark` 两条 unary RPC，零快照调用、零记账/确认接口；服务端 RBAC 亦对 `ConfirmDraftEarmark`、`CreateHold`、`CreateCommitted`、`ResolvePendingMatch` 一律返回 `FORBIDDEN`，草案只能由浏览器 `TRADER` 人工确认 |

凭证传递支持 `authorization: Bearer <token>`（默认 `--header-mode bearer`）或 `x-ficant-credential: <token>`（`--header-mode x-ficant-credential`），两头互斥且不可重复；Sidecar 进程通过 `--token-env` 或 `--token-file` 加载令牌，**严禁**将明文令牌写入命令行参数或检查点文件。

### 3.2 可信目录与文件系统 ACL（含锁与临时空间要求）

`ficant` 工作区强制 `unsafe_code = "forbid"`，在安全 Rust 范围内对文件与目录执行了符号链接/Windows 重解析点（`0x0400`）、物理文件身份、硬链接数（`nlink == 1`）与读前后复核校验；但对于特权管理员瞬间目录交换再还原等系统级威胁，仍依赖操作系统可信目录 ACL 共同形成纵深防御：

| 路径类别 | 建议权限与约束 |
|---|---|
| 凭证摘要文件（`FICANT_BLOTTER_CREDENTIALS_PATH`）与 Sidecar `--token-file` | 仅运行账户只读（Linux `0400`/`0600`；Windows 仅授专用服务账户与 `Administrators`），目录禁止其他用户列表或写入 |
| 外置前端目录（`FICANT_STATIC_WEB_DIR`，若启用）及其全部祖先目录 | 仅可信部署账户可写，服务运行账户只读；禁止任何符号链接或 Windows Junction；生产优先使用 `embedded` 单文件模式以消除磁盘资产面 |
| SQLite 数据目录（`<db>.sqlite`、`-wal`、`-shm`、`<db>.rollover.lock`、`<db>.rollover.state.json`） | 必须位于本地持久化文件系统（禁止网络共享盘、符号链接、重解析点或额外硬链接，源库与伴生锁库强制 `nlink == 1`）；仅服务账户与离线维护账户可读写；可用磁盘空间至少保留当前 DB + WAL 总大小的 **2–3 倍**（用于私有临时恢复副本、`VACUUM` 重建与归档写入） |
| 归档目录（`--archive-dir`） | 与现网库目录物理隔离，由可信维护账户写入；已发布的 `<stem>.sqlite` 与 `<stem>.manifest.json` 视作不可变审计证据，禁止覆盖或就地打开写入 |
| Sidecar 检查点目录（`--checkpoint`） | 单个检查点文件受单写者文件锁保护，仅对应 Sidecar 运行账户可读写；检查点（`ficant.sidecar.checkpoint.v1`）采用同目录临时文件 `fsync + os.replace` 原子落盘，严禁包含任何令牌字段 |

---

## 4. 盘后排空、离线换库、独立复核与待决恢复

离线换库与归档复核均由同一 `ficant-server` 二进制的同步离线子模式执行（[`binaries/ficant-server/src/cli.rs`](../../binaries/ficant-server/src/cli.rs)），不启动网络监听、不初始化 Tokio 运行时。

### 4.1 盘后停止 Sidecar 与检查未结清条目

1. **停止全部写入 Sidecar**：向 `ficant-sidecar quotes`、`trade-feed`、`nlp-draft` 发送 `SIGINT` / `SIGTERM`，等待当前在途批次确认并原子更新各自检查点后退出。
2. **核查未结清占仓与挂起成交**：
   - 默认换库（不带 `--allow-unresolved`）发现任意未结清 `COMMITTED` 占仓（`unresolved_committed > 0`）或 `OPEN` 挂起成交（`open_pending_match > 0`）时，会在私有临时副本上完成检查并**直接阻断退出非零**，源库主文件与 `-wal` 保持 **0 字节改动**。
   - **常规盘后**：在副屏或管理流程中将当日 `COMMITTED` 全部核销/取消、将 `OPEN` 挂起成交全部处理完毕后再执行换库。
   - **仅做只读审计快照（不清空账本、不推进 `ledger_epoch`）**：使用 `--archive-only`（与 `--allow-unresolved` 互斥）。
   - **经审批的应急强制换库**：显式附加 `--allow-unresolved`，清单会记录 `allow_unresolved_used: true` 供事后审计。

### 4.2 受控停机（`SHUTDOWN`）

启动 `ficant-server` 时若设置了 `FICANT_ENABLE_STDIN_SHUTDOWN=1`，可通过独占控制管道写入一行 `SHUTDOWN`（或关闭 stdin EOF）触发有界排空停机：

- 服务立即停止接受新请求，向全部活跃原生 HTTP/2 与 HTTP/1.1 `gRPC-Web` `SubscribeEvents` 及 SPI 流广播取消信号（慢消费者满队列亦会被强制解阻退出），停止看门狗，在 **5 秒**预算内排空关闭 SQLite 连接并释放伴生锁 `<db>.rollover.lock`，最终以退出码 **`0`** 结束。
- **在线互斥保护**：只要 `ficant-server` 仍在运行，`<db>.rollover.lock` 被 SQLite `BEGIN EXCLUSIVE` 持有；此时执行 `--sqlite-rollover` 会以 `busy_timeout=0`、零退避睡眠立即因 `BUSY/LOCKED` 失败退出，且不干扰在线服务。
- **异常强杀恢复**：若服务曾因进程强杀或宿主机断电终止，操作系统会回收 `<db>.rollover.lock`；随后执行的 `--sqlite-rollover` 会自动在互斥保护下恢复有效 WAL 帧再进行归档与重置。

### 4.3 执行离线换库与独立清单复核

```powershell
# 1. 标准盘后归档并重置现网库（ledger_epoch: E -> E+1，生成新 ledger_instance_id）
.\ficant-server.exe --sqlite-rollover `
    --db-path D:\ficant-data\blotter.sqlite `
    --archive-dir D:\ficant-archives

# 可选：仅归档不重置（不修改现网业务表与 ledger_epoch）
.\ficant-server.exe --sqlite-rollover `
    --db-path D:\ficant-data\blotter.sqlite `
    --archive-dir D:\ficant-archives `
    --archive-only

# 可选：显式指定自定义 stem（ASCII 字母数字开头，仅含字母数字/下划线/连字符，禁含点号/.sqlite/保留设备名）
.\ficant-server.exe --sqlite-rollover `
    --db-path D:\ficant-data\blotter.sqlite `
    --archive-dir D:\ficant-archives `
    --archive-name desk_eod_20261125

# 2. 独立审计复核（必须退出 0）
.\ficant-server.exe --verify-sqlite-archive `
    --manifest D:\ficant-archives\blotter-20261125T100000Z-e1.manifest.json
```

- `--sqlite-rollover` 成功时向 stdout 输出格式化 JSON 报告（包含 `operation_id`、`source_ledger_epoch`、`next_ledger_epoch`、`next_ledger_instance_id`、`wal_truncated`、`manifest_path`），并在 `<archive-dir>` 原子发布不可覆盖的 `<stem>.sqlite` 与 `<stem>.manifest.json`（schema `ficant.blotter.archive-manifest.v1`）。
- `--verify-sqlite-archive` 独立重算归档字节长度、整文件 `archive_sha256_hex`、17 张业务表逐列双源投影审计与规范全表 `content_sha256_hex`、`accounts`、`counts`（10 项计数）以及五态字段矩阵，任一字节或字段被篡改即退出非零。

### 4.4 换库中断与 `RESET_COMMITTED_MAINTENANCE_PENDING` 故障恢复

离线换库采用持久化状态机 `<db>.rollover.state.json`（实现见 [`crates/ficant-blotter-store/src/rollover.rs`](../../crates/ficant-blotter-store/src/rollover.rs)）：

`PREPARED` -> `ARCHIVE_PUBLISHED` -> `RESET_COMMITTED_MAINTENANCE_PENDING` -> `COMPLETED_RESET`（或 `COMPLETED_ARCHIVE_ONLY`）。

当 `<db>.rollover.state.json` 处于任意未完成阶段时，`ficant-server` **会拒绝启动**，防止在半换库状态上错误开账。如遇进程崩溃、磁盘满或维护步骤中断，按以下步骤安全续跑恢复：

1. 确认无活跃 `ficant-server` 或其他维护进程，并先排除根因（如清理同卷磁盘空间、恢复归档目录可写权限）。
2. **严禁**手工删除 `<db>.rollover.state.json`、`<db>.rollover.lock` 或已发布的 `<stem>.sqlite` / 待决清单文件。
3. 使用**相同的 `--db-path`、`--archive-dir`、模式及 `--allow-unresolved` 意图，且不要传 `--archive-name`**，直接重跑 `--sqlite-rollover`：

   ```powershell
   .\ficant-server.exe --sqlite-rollover `
       --db-path D:\ficant-data\blotter.sqlite `
       --archive-dir D:\ficant-archives
   ```

4. 引擎在生成默认时间戳前会优先读取 `<db>.rollover.state.json`，自动接续同一 `operation_id`、原 `stem` 与预定的 `next_ledger_instance_id`：
   - **中断于 `PREPARED` / `ARCHIVE_PUBLISHED`**：验证或完成归档发布与 17 表一致性复核，在唯一 `BEGIN IMMEDIATE` 事务内清空业务表并将账本原子推进至 `(E+1, N)`，再执行维护与终态发布。（注意：对未完成 reset 传入 `--archive-only` 会报模式冲突，永不触发删除。）
   - **中断于 `RESET_COMMITTED_MAINTENANCE_PENDING`**（或源事务刚提交但状态文件尚未更新，引擎通过源库 `blotter_meta.last_reset_operation_id` 与 `(E+1, N)` 自动识别修复）：**跳过删除与纪元递增（绝不会二次增加 `ledger_epoch`）**，仅补跑或重跑 `VACUUM`、最终 `wal_checkpoint(TRUNCATE)`（校验 `busy=0, log>=0, checkpointed=log`）与关库 0 字节/无残留 WAL 校验，成功后才发布 `COMPLETED_RESET` 最终清单并退役活动状态指针。
5. 恢复完成后运行 `--verify-sqlite-archive --manifest <最终清单>` 确认退出 `0`，再启动 `ficant-server` 并核实 `/healthz` 中的 `ledger_epoch == E+1` 与 `ledger_instance_id == N`。

---

## 5. 次日开账底仓水位与 Sidecar 跨日续接

### 5.1 启动次日服务并导入 Day-2 底仓

1. 启动 `ficant-server`（指向已换库重置的 `<db>.sqlite`），调用 `GET /healthz` 确认 `status == "ok"`、`blotter_healthy == true`、`ledger_epoch == E+1`、`ledger_instance_id == N`。
2. 使用 `OPERATOR` 凭证调用 `ImportOpeningBase` 导入次日 `OpeningPositionBase`，重点核对以下四个字段：
   - `trading_date`：推进至次日交易日（须严格晚于前一日）；
   - `next_settlement_date`：对应次日 T+1 结算日；
   - `source_start_after_feed_seq`：次日成交回报源起始水位之前的最大已处理序号（例如次日回报从 `feed_seq = 1` 重新编号时设为 `0`）；
   - `watermark_source_seq`：对应底仓快照截止的源流水水位（与 `source_start_after_feed_seq` 配合允许次日首笔新流水接入）。

### 5.2 准备次日新输入文件并启动 Sidecar

Sidecar 检查点（`ficant.sidecar.checkpoint.v1`）持久化绑定 `(ledger_epoch, ledger_instance_id, trading_date, input_fingerprint)`。当 `/healthz` 返回新账本 `(E+1, N)` 时，合法跨日推进必须同时满足三项条件：

1. 服务端 `ledger_epoch` 前进且 `ledger_instance_id` 变更；
2. CLI 传入推进后的次日 `--trading-date`；
3. `--input` 指向**次日新文件**（其 SHA-256 `input_fingerprint` 必须与昨日不同）。

> **为什么昨日输入文件会被拒收**：若继续将昨日旧文件传给次日 Sidecar，相同的 `input_fingerprint` 会被检查点状态机立即拒绝，避免昨日行情或成交被错误重放到新交易日。昨日检查点中若残留未确认的 `in_flight` 请求，会被隔离记录在 `previous_epoch_summary` 供人工核查，**绝不会**仅替换请求头后向新账本重发。

携带原 `--checkpoint` 文件、次日 `--trading-date`、次日新 `--input` 以及可选的成对预期账本参数启动三个 Sidecar（完整参数见 [`python/sdk/src/ficant_sdk/sidecar/cli.py`](../../python/sdk/src/ficant_sdk/sidecar/cli.py)）：

```powershell
# 行情 Sidecar（Day-2 首笔从 quote_seq=1 起正常应用）
ficant-sidecar quotes `
    --endpoint https://blotter.desk.internal:443 `
    --ca-cert D:\ficant-tls\intranet-ca.pem `
    --token-file D:\ficant-secrets\feed-adapter.token `
    --checkpoint D:\ficant-state\quotes.checkpoint.json `
    --trading-date 2026-11-26 `
    --sidecar-id quotes-primary `
    --input D:\ficant-feeds\2026-11-26\quotes.jsonl `
    --expected-ledger-epoch 2 `
    --expected-ledger-instance 01M8A1F9NBY54SQX6ZMGV4E2TY `
    --poll-interval-ms 1000

# 成交回报 Sidecar（Day-2 首笔从 feed_seq=1 起推进，昨日回执与业务修订零重放）
ficant-sidecar trade-feed `
    --endpoint https://blotter.desk.internal:443 `
    --ca-cert D:\ficant-tls\intranet-ca.pem `
    --token-file D:\ficant-secrets\feed-adapter.token `
    --checkpoint D:\ficant-state\trade-feed.checkpoint.json `
    --trading-date 2026-11-26 `
    --sidecar-id trade-feed-primary `
    --source-id primary-trade-feed `
    --input D:\ficant-feeds\2026-11-26\trades.jsonl `
    --expected-ledger-epoch 2 `
    --expected-ledger-instance 01M8A1F9NBY54SQX6ZMGV4E2TY `
    --poll-interval-ms 1000

# NLP 草案网关 Sidecar（固定使用 NlpDraftOnlyClient 与 NLP_PARSER 角色）
ficant-sidecar nlp-draft `
    --endpoint https://blotter.desk.internal:443 `
    --ca-cert D:\ficant-tls\intranet-ca.pem `
    --token-file D:\ficant-secrets\nlp-parser.token `
    --checkpoint D:\ficant-state\nlp-draft.checkpoint.json `
    --trading-date 2026-11-26 `
    --sidecar-id nlp-gateway-primary `
    --adapter webhook `
    --webhook-url https://nlp.desk.internal/parse `
    --input D:\ficant-feeds\2026-11-26\nlp-intents.jsonl `
    --expected-ledger-epoch 2 `
    --expected-ledger-instance 01M8A1F9NBY54SQX6ZMGV4E2TY `
    --poll-interval-ms 1000
```

### 5.3 服务端账本写围栏（Write Fence Metadata）

所有三个 Sidecar 写入口（`IngestMarketQuotes`、`IngestTradeFeed`、`SubmitDraftEarmark`）均固定携带成对 gRPC metadata 请求头：

- `x-ficant-expected-ledger-epoch`：正整数规范十进制 ASCII（无前导 `+` 或 `0`）；
- `x-ficant-expected-ledger-instance`：规范 26 位大写 ULID。

服务端在鉴权通过后、且在单写者互斥锁内**早于幂等回执查询（`query_idempotency_row`）**校验该组围栏头：

- 任意半对缺失、重复出现或格式非法：返回 `VALIDATION_FAILED`；
- 与当前绑定存储实例不匹配（例如昨日残留进程向 Day-2 发送迟到请求）：返回不可重试的 `CONCURRENCY_CONFLICT`（`retryable=false`），确保新账本业务行、幂等回执、事件日志与 `head_seq` 增量为零。

---

## 6. 恢复历史已验证归档

当因审计复盘或灾备回退需要重新挂载某一历史交易日归档时，必须遵循“**先验签、再复制、隔离旧检查点、保原归档不可变**”原则：

1. **停止现网进程**：按第 4 节步骤停止所有 `ficant-sidecar` 进程并对现网 `ficant-server` 执行受控 `SHUTDOWN`。
2. **先独立验证目标归档**（必须退出 `0`）：

   ```powershell
   .\ficant-server.exe --verify-sqlite-archive `
       --manifest D:\ficant-archives\blotter-20261124T100000Z-e1.manifest.json
   ```

   从输出 JSON 记录目标归档的 `source_ledger_epoch`（记作 `RESTORED_E`）与 `source_ledger_instance_id`（记作 `RESTORED_I`）。
3. **复制到独立工作库路径（严禁原地挂载归档文件）**：
   - 将 `<archive-dir>\<stem>.sqlite` 复制到全新的独立工作文件路径（例如 `D:\ficant-restore\blotter-restore.sqlite`），**保持 `<archive-dir>` 下的原始 `<stem>.sqlite` 与 `<stem>.manifest.json` 绝对只读不可变**。
   - 确保目标工作目录干净，无残留的 `-wal`、`-shm`、`.rollover.state.json` 或旧锁文件。
4. **启动服务端并核验 `/healthz`**：将 `FICANT_BLOTTER_DB_PATH` 指向复制后的工作库启动 `ficant-server`，请求 `GET /healthz` 确认返回 `ledger_epoch == RESTORED_E` 与 `ledger_instance_id == RESTORED_I`。
5. **隔离高纪元 Sidecar 检查点并重建恢复基线**：
   - 默认情况下，Sidecar 探测到服务端 `ledger_epoch` 回退或同纪元实例变化时会立即停机（保护账本不被错位重放）。
   - 经人工核实目标恢复账本身份无误后，对需要接入恢复实例的 Sidecar **单次**附加 `--reset-checkpoint-for-restore`，并显式成对传入 `--expected-ledger-epoch <RESTORED_E> --expected-ledger-instance <RESTORED_I>` 及对应的基线输入文件：

     ```powershell
     ficant-sidecar quotes `
         --endpoint https://blotter.desk.internal:443 `
         --ca-cert D:\ficant-tls\intranet-ca.pem `
         --token-file D:\ficant-secrets\feed-adapter.token `
         --checkpoint D:\ficant-state\quotes.checkpoint.json `
         --trading-date 2026-11-24 `
         --sidecar-id quotes-primary `
         --input D:\ficant-feeds\2026-11-24\quotes-restore.jsonl `
         --expected-ledger-epoch 1 `
         --expected-ledger-instance 01M8A1F3BGC74AHDSRHXHEFTNX `
         --reset-checkpoint-for-restore `
         --once
     ```

   - 该参数会将原高纪元检查点安全隔离归档，随后为已验证的目标恢复账本建立干净检查点（不继承高纪元偏移，也不重发高纪元在途请求）。后续正常运行务必移除 `--reset-checkpoint-for-restore`。

---

## 相关文档

- 本地开发与门禁说明：[`docs/development.md`](../development.md)
- GitHub test 环境发布合同：[`docs/delivery/test-environment.md`](test-environment.md)
- 仓库协作与发布边界：[`AGENTS.md`](../../AGENTS.md)
