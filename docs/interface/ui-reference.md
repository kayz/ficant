# Platform Shell 与多 WebApp 界面参考

**状态（2026-09-04）：** Platform Shell 与 Phase 5A 非业务观测面板已实现；DMQuant 和相邻 Portfolio 业务 WebApp 尚未在本仓库落地或完成接线

**详细页面设计：** `web-dm/webapps/<app-id>/design.md`

**后台合同：** `interface/README.md` 与 `interface/proto/`

## 目录与所有权

```text
web-dm/
├── platform-shell/                 # 共享宿主、会话、Registry、加载和错误边界
├── packages/contracts-generated/   # 根 interface/ 生成的 TypeScript consumer
└── webapps/
    └── dmquant/design.md            # 首个 WebApp 的中文目标设计

interface/                           # Rust/Python/TypeScript 共用后台合同
```

未来新增 WebApp 的页面设计、源码和测试都进入 `web-dm/webapps/<app-id>/`。当前该目录只有 `dmquant/design.md`，实际前端实现仍位于 `platform-shell/`。共享 Shell 不承载完整研究产品流程，根 `interface/` 不放页面设计；WebApp 也不得建立平行后台 DTO 或服务。

## 已实现 Platform Shell 流程

```text
读取当前会话
→ 必要时刷新即将过期的会话
→ 读取服务端裁剪后的 App Registry
→ 请求指定应用的短期启动授权
→ 校验启动边界
→ iframe 加载并用 postMessage 交付短期 credential
→ 到期前刷新，退出/过期/失败时撤权
```

Shell 已区分以下可观察状态：

- 会话检查、有效、即将过期刷新、过期和读取失败；
- Registry 加载、空、成功和失败；
- 应用授权中、就绪、禁止、不可用和内容加载失败；
- 操作切换或组件卸载时取消过期请求，迟到响应不能把旧授权重新写回页面；
- 返回应用目录后恢复到先前打开按钮的键盘焦点。

客户端只消费服务端返回的可见应用、capability 和 scopes，不根据 `researcher`、`viewer` 等前端字符串推导最终授权。

## gRPC-Web 合同

浏览器使用从 `interface/` 机械生成的 `ficant.app.v1.PlatformService` consumer，真实 Rust 服务提供七个 RPC：

1. `GetAppRegistry`
2. `GetCurrentSession`
3. `RefreshSession`
4. `RevokeSession`
5. `AuthorizeAppLaunch`
6. `RefreshAppLaunch`
7. `RevokeAppLaunch`

gRPC-Web base URL 禁止 userinfo、query 和 fragment，生产使用 HTTPS，本机回环开发可使用 HTTP。Rust transport 只允许配置中的精确 CORS origin；preflight 只开放 POST 和冻结 header 集合。

## iframe 启动边界

Shell 在创建 iframe 前同时校验 Registry descriptor 与短期 grant：

- `app_id`、`entrypoint`、`allowed_origin` 必须精确一致；entrypoint 是同 origin 下不含 query/fragment 的绝对路径；
- grant scopes 只能是 Registry capabilities 的子集；credential 非空且 issued/expires 时间有效；
- origin 必须是 HTTPS 精确 origin，本机回环测试除外；token 不进入 URL、referrer 或 `localStorage`；
- sandbox 只能使用批准 token，且禁止同时开放 `allow-scripts` 与 `allow-same-origin`；
- CSP 必须以 `default-src 'none'` 收口，拒绝 wildcard、重复/未知指令和非安全来源；
- iframe 使用 `referrerPolicy=no-referrer`，加载后只向精确 `allowed_origin` 发送 `ficant.app.launch.v1` credential；
- grant 刷新必须延长有效期。边界失败、iframe error、过期、返回目录或卸载都会撤销应用授权。

## 错误与恢复

Platform Service 返回 `SafeError(code, safe_message, trace_id, retryable)`。Shell：

- 显示稳定错误码和安全消息；存在 `trace_id` 时允许复制；
- 只在 `retryable=true` 且当前流程提供恢复动作时显示重试；
- 区分认证过期、禁止、资源不存在、暂时不可用和本地边界校验失败；
- 网络失败只显示安全的“平台连接暂时不可用”，不暴露 raw cause、stack、credential 或服务端内部信息。

Phase 1 领域错误使用 `ficant.core.v1.ErrorDetail` 的独立 core mapper。Platform Shell 当前只额外装配 Phase 5A 观测面板：它读取已持久化 Run 输出并解码 `AnalyzeBondResult` / `RiskSummary`，不是面向用户提交分析的通用 Phase 1 业务页，也不伪造后台结果。

## 可访问性现状

当前 Shell 已实现语义化状态/错误区域、polite live region、可见焦点、键盘进入/退出 iframe、iframe title、非颜色状态表达、200% 缩放和 reduced-motion 支持。自动化界面验收覆盖 Registry/会话/授权状态、权限边界、焦点与真实 gRPC-Web 路径；后续 WebApp 仍须为自己的页面补充独立可访问性验收。

## DMQuant 与明确延期

DMQuant 的登录后研究流程仍以 `web-dm/webapps/dmquant/design.md` 为唯一页面设计。AI 草稿、参数编辑、策略版本、异步回测、通用指标/曲线/Artifact 浏览、归因、寻优和多 run 对比尚未实现；旧静态原型的评审工具条、手工状态开关和硬编码示例数据不得进入产品。

Phase 5A 观测面板能展示既有 Run 中经完整性校验的 `AnalyzeBondResult` / `RiskSummary`，但没有直接发起 Phase 2 分析的交互流程；DV01、基差、IRR、CTD 等其他结果仍无业务页面。界面不得展示看似真实但没有后台合同与业务闭环的结果。

## 相邻 Portfolio WebApp 接线

金证FICC合同管理系统的 24 页工作台不在 `web-dm/webapps/`。FICANT 只提供 `ficant.portfolio.v1` 只读 RPC 与 `portfolio-workbench.v1` PageEnvelope；相邻仓库安装本地 `@ficant/contracts-generated@0.0.0` artifact，不得继续 alias 到本树 `web-dm/packages/contracts-generated/src`。

Docker 开发 gRPC-Web 默认 `http://127.0.0.1:18080`，允许 origin 精确包含 `http://127.0.0.1:5173`。`127.0.0.1:50051` 只用于本机 native gRPC。R8A/R8B 已提供 D01/P01/P02/P03/P04 所需的真实 DTO/BFF 合同，但尚未修改或接入相邻 WebApp；下游接线后这五页必须使用真实 DTO，其余页面由 WebApp 自己标 demo/partial。BFF 失败必须呈现 typed error，不得用 mock 冒充 backend success。

## Validity

Valid: long-term until superseded
