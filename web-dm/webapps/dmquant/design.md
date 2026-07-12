# DMQuant 页面与交互设计

## 状态与范围

本文件由原 `docs/interface/ui-reference.md` 的详细设计收敛而来，并与未来 DMQuant WebApp 源码、manifest 和测试共置。iteration-2 只实现 Platform Shell 与多 WebApp 加载边界，不实现完整 DMQuant 策略生成、回测或 Artifact 浏览业务。

`UI-DM/dmquant-extended-design.html` 仍是静态评审输入；其顶部 `REVIEW / 仅评审` 工具条、手工状态开关和硬编码示例数据不属于产品。

## 多 WebApp 目录

```text
web-dm/
├── platform-shell/                # 登录后宿主、registry、路由、会话与错误边界
├── packages/
│   └── contracts-generated/       # 从根 interface/ 生成，禁止手写
└── webapps/
    └── dmquant/
        ├── design.md              # 本文件，中文当前设计
        ├── app.yaml               # 未来应用 manifest
        ├── src/                   # 未来 DMQuant 源码
        └── tests/                 # 未来单元/浏览器测试
```

## iteration-2 Platform Shell 能力

- App Registry：空、加载、成功、失败。
- App 加载：未授权、加载中、就绪、拒绝、不可用、加载失败。
- 会话：有效、即将过期、已过期；不得把主 token 放入 URL 或 localStorage。
- iframe/应用边界：验证 origin、CSP、sandbox、短期 App Token、撤权与重新认证。
- 错误：显示安全消息、稳定 `code`、可复制 `trace_id` 和契约允许的恢复动作。
- 权限：只消费服务端返回的应用可见性和 capability，不从前端角色字符串推导最终授权。
- 可访问性：键盘主路径、可见焦点、live region、iframe title/进出路径、非颜色状态、WCAG 2.2 AA、200% 缩放和 reduced motion。
- 真实接口：通过根 `interface/` 生成的 gRPC-Web client 调用 Rust 服务；禁止 mock 成功响应作为验收。

## DMQuant 后续目标体验

详细目标流程仍是登录 → AI 草稿 → 参数应用/编辑 → 版本保存 → 异步回测 → 状态/指标/产物/血缘 → 编辑新版本重跑。该流程属于后续迭代，iteration-2 不得创建看似可用但无真实后台闭环的假页面。

## 文档唯一性

`docs/interface/ui-reference.md` 只保留本文件和根 `interface/README.md` 的索引，不再复制状态、权限、API 映射或无障碍事实。任何页面事实变更先更新对应 `web-dm/webapps/<app-id>/design.md`。

## Validity

Valid: iteration-2 design until implementation evidence supersedes it
