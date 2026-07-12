import { create } from "@bufbuild/protobuf";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  ErrorCode,
  PlatformService,
  SafeErrorSchema,
  type AppDescriptor,
  type AppLaunchGrant,
  type SafeError,
} from "../../packages/contracts-generated/src/ficant/app/v1/registry_pb";
import type { Session } from "../../packages/contracts-generated/src/ficant/app/v1/session_pb";
import { usePoliteAnnouncements } from "./announcements";
import { SafeErrorPanel } from "./error";
import { AppFrame, validateLaunchBoundary } from "./loader";
import { networkFailure, type PlatformClient } from "./registry";
import { classifySession, sessionExpiryLabel, timestampMilliseconds } from "./session";

type View =
  | { kind: "booting" }
  | { kind: "registry-loading"; session: Session }
  | { kind: "registry-empty"; session: Session }
  | { kind: "registry-ready"; session: Session; apps: AppDescriptor[] }
  | { kind: "registry-error"; session: Session; error: SafeError }
  | { kind: "session-expired" }
  | { kind: "session-error"; error: SafeError }
  | { kind: "app-authorizing"; session: Session; app: AppDescriptor; apps: AppDescriptor[] }
  | { kind: "app-ready"; session: Session; app: AppDescriptor; grant: AppLaunchGrant; apps: AppDescriptor[] }
  | AppFailureView;

interface AppFailureView {
  kind: "app-forbidden" | "app-unavailable" | "app-load-error";
  session: Session;
  app: AppDescriptor;
  apps: AppDescriptor[];
  error: SafeError;
}

interface PendingOperation {
  generation: number;
  controller: AbortController;
}

const systemNow = () => new Date();

export interface PlatformShellProps {
  client: PlatformClient;
  now?: () => Date;
  transport?: "grpc-web" | "test";
}

export function PlatformShell({ client, now = systemNow, transport = "test" }: PlatformShellProps) {
  const [view, setView] = useState<View>({ kind: "booting" });
  const [announcement, announce] = usePoliteAnnouncements("正在建立安全会话");
  const frameRef = useRef<HTMLIFrameElement>(null);
  const lastOpenAppId = useRef("");
  const generationRef = useRef(0);
  const pendingRef = useRef<PendingOperation | undefined>(undefined);
  const mountedRef = useRef(false);
  const viewRef = useRef<View>(view);
  viewRef.current = view;

  const safeRevoke = useCallback((appId: string) => {
    void client.revokeAppLaunch(appId).catch(() => undefined);
  }, [client]);

  const invalidatePending = useCallback(() => {
    generationRef.current += 1;
    pendingRef.current?.controller.abort();
    pendingRef.current = undefined;
  }, []);

  const beginOperation = useCallback((): PendingOperation => {
    invalidatePending();
    const operation = {
      generation: generationRef.current,
      controller: new AbortController(),
    };
    pendingRef.current = operation;
    return operation;
  }, [invalidatePending]);

  const isCurrent = useCallback((operation: PendingOperation, session?: Session): boolean => {
    if (!mountedRef.current
      || operation.generation !== generationRef.current
      || operation.controller.signal.aborted) return false;
    if (!session) return true;
    const timing = classifySession(session, now());
    return timing !== "expired" && timing !== "invalid";
  }, [now]);

  const expireSession = useCallback((appId?: string) => {
    invalidatePending();
    if (appId) safeRevoke(appId);
    if (!mountedRef.current) return;
    setView({ kind: "session-expired" });
    announce("会话已过期");
  }, [announce, invalidatePending, safeRevoke]);

  const loadRegistry = useCallback(async (session: Session) => {
    const operation = beginOperation();
    setView({ kind: "registry-loading", session });
    announce("应用目录加载中");
    try {
      const response = await client.getAppRegistry(operation.controller.signal);
      if (!isCurrent(operation, session)) return;
      if (response.result.case === "error") {
        setView({ kind: "registry-error", session, error: response.result.value });
        announce("应用目录读取失败");
      } else if (response.result.case === "registry") {
        const apps = response.result.value.apps;
        setView(apps.length === 0 ? { kind: "registry-empty", session } : { kind: "registry-ready", session, apps });
        announce(apps.length === 0 ? "当前没有可用应用" : `已读取 ${apps.length} 个可用应用`);
      } else {
        setView({ kind: "registry-error", session, error: localError(ErrorCode.INTERNAL, "应用目录响应不完整") });
        announce("应用目录响应不完整");
      }
    } catch {
      if (!isCurrent(operation, session)) return;
      setView({ kind: "registry-error", session, error: create(SafeErrorSchema, networkFailure()) });
      announce("平台连接暂时不可用");
    }
  }, [announce, beginOperation, client, isCurrent]);

  const bootstrap = useCallback(async () => {
    const operation = beginOperation();
    setView({ kind: "booting" });
    announce("正在建立安全会话");
    try {
      const current = await client.getCurrentSession(operation.controller.signal);
      if (!isCurrent(operation)) return;
      if (current.result.case === "error") {
        if (isAuthenticationError(current.result.value.code)) {
          expireSession();
        } else {
          setView({ kind: "session-error", error: current.result.value });
          announce("会话读取失败");
        }
        return;
      }
      if (current.result.case !== "session") {
        setView({ kind: "session-error", error: localError(ErrorCode.INTERNAL, "会话响应不完整") });
        announce("会话响应不完整");
        return;
      }
      let session = current.result.value;
      const timing = classifySession(session, now());
      if (timing === "expired" || timing === "invalid") {
        expireSession();
        return;
      }
      if (timing === "expiring") {
        announce("会话即将过期，正在刷新");
        const refreshed = await client.refreshSession(operation.controller.signal);
        if (!isCurrent(operation)) return;
        if (refreshed.result.case === "error") {
          if (isAuthenticationError(refreshed.result.value.code)) {
            expireSession();
          } else {
            setView({ kind: "session-error", error: refreshed.result.value });
            announce("会话刷新失败");
          }
          return;
        }
        if (refreshed.result.case !== "session" || classifySession(refreshed.result.value, now()) !== "valid") {
          expireSession();
          return;
        }
        session = refreshed.result.value;
      }
      if (isCurrent(operation, session)) await loadRegistry(session);
    } catch {
      if (!isCurrent(operation)) return;
      setView({ kind: "session-error", error: create(SafeErrorSchema, networkFailure()) });
      announce("平台连接暂时不可用");
    }
  }, [announce, beginOperation, client, expireSession, isCurrent, loadRegistry, now]);

  const refreshAppGrant = useCallback(async (ready: Extract<View, { kind: "app-ready" }>) => {
    const operation = beginOperation();
    try {
      const response = await client.refreshAppLaunch(ready.app.appId, operation.controller.signal);
      if (!isCurrent(operation, ready.session)) {
        safeRevoke(ready.app.appId);
        return;
      }
      if (response.result.case === "error") {
        safeRevoke(ready.app.appId);
        if (isAuthenticationError(response.result.value.code)) {
          expireSession();
        } else {
          setView({
            kind: appFailureKind(response.result.value.code),
            app: ready.app,
            apps: ready.apps,
            session: ready.session,
            error: response.result.value,
          });
          announce(`${ready.app.displayName} 授权刷新失败`);
        }
        return;
      }
      if (response.result.case !== "grant") {
        safeRevoke(ready.app.appId);
        setView({
          kind: "app-load-error",
          app: ready.app,
          apps: ready.apps,
          session: ready.session,
          error: localError(ErrorCode.INTERNAL, "应用授权响应不完整"),
        });
        announce(`${ready.app.displayName} 授权刷新失败`);
        return;
      }
      try {
        const refreshedBoundary = validateLaunchBoundary(ready.app, response.result.value, now().getTime());
        const priorExpiry = timestampMilliseconds(ready.grant.expiresAt);
        if (priorExpiry === undefined || refreshedBoundary.expiresAt <= priorExpiry) {
          throw new Error("刷新后的启动授权没有延长有效期");
        }
      } catch {
        safeRevoke(ready.app.appId);
        setView({
          kind: "app-load-error",
          app: ready.app,
          apps: ready.apps,
          session: ready.session,
          error: localError(ErrorCode.INVALID_REQUEST, "应用启动边界验证失败"),
        });
        announce(`${ready.app.displayName} 启动边界验证失败`);
        return;
      }
      setView({ ...ready, grant: response.result.value });
      announce(`${ready.app.displayName} 授权已刷新`);
    } catch {
      if (!isCurrent(operation, ready.session)) return;
      safeRevoke(ready.app.appId);
      setView({
        kind: "app-unavailable",
        app: ready.app,
        apps: ready.apps,
        session: ready.session,
        error: create(SafeErrorSchema, networkFailure()),
      });
      announce(`${ready.app.displayName} 授权刷新连接失败`);
    }
  }, [announce, beginOperation, client, expireSession, isCurrent, now, safeRevoke]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      invalidatePending();
      const current = viewRef.current;
      if (current.kind === "app-ready" || current.kind === "app-authorizing") safeRevoke(current.app.appId);
    };
  }, [invalidatePending, safeRevoke]);

  useEffect(() => { void bootstrap(); }, [bootstrap]);

  useEffect(() => {
    if (!("session" in view)) return;
    const expiresAt = timestampMilliseconds(view.session.expiresAt);
    if (expiresAt === undefined) return;
    const delay = Math.max(0, Math.min(expiresAt - now().getTime(), 2_147_483_647));
    const timer = window.setTimeout(() => {
      expireSession("app" in view ? view.app.appId : undefined);
    }, delay);
    return () => window.clearTimeout(timer);
  }, [expireSession, now, view]);

  useEffect(() => {
    if (view.kind !== "app-ready") return;
    const expiresAt = timestampMilliseconds(view.grant.expiresAt);
    if (expiresAt === undefined) {
      safeRevoke(view.app.appId);
      setView({
        kind: "app-load-error",
        app: view.app,
        apps: view.apps,
        session: view.session,
        error: localError(ErrorCode.INVALID_REQUEST, "应用启动边界验证失败"),
      });
      return;
    }
    const remaining = expiresAt - now().getTime();
    const refreshLead = Math.min(5_000, Math.max(1_000, Math.floor(remaining / 5)));
    const refreshTimer = window.setTimeout(() => { void refreshAppGrant(view); }, Math.max(0, remaining - refreshLead));
    const expiryTimer = window.setTimeout(() => {
      invalidatePending();
      safeRevoke(view.app.appId);
      setView({
        kind: "app-load-error",
        app: view.app,
        apps: view.apps,
        session: view.session,
        error: localError(ErrorCode.EXPIRED, "应用授权已过期"),
      });
      announce(`${view.app.displayName} 授权已过期`);
    }, Math.max(0, remaining));
    return () => {
      window.clearTimeout(refreshTimer);
      window.clearTimeout(expiryTimer);
    };
  }, [announce, invalidatePending, now, refreshAppGrant, safeRevoke, view]);

  async function openApp(app: AppDescriptor, apps: AppDescriptor[], session: Session, button: HTMLButtonElement) {
    const operation = beginOperation();
    lastOpenAppId.current = button.dataset.openAppId ?? app.appId;
    setView({ kind: "app-authorizing", app, apps, session });
    announce(`正在授权 ${app.displayName}`);
    try {
      const response = await client.authorizeAppLaunch(app.appId, operation.controller.signal);
      if (!isCurrent(operation, session)) {
        safeRevoke(app.appId);
        return;
      }
      if (response.result.case === "grant") {
        try {
          validateLaunchBoundary(app, response.result.value, now().getTime());
        } catch {
          safeRevoke(app.appId);
          setView({
            kind: "app-load-error",
            app,
            apps,
            session,
            error: localError(ErrorCode.INVALID_REQUEST, "应用启动边界验证失败"),
          });
          announce(`${app.displayName} 启动边界验证失败`);
          return;
        }
        setView({ kind: "app-ready", app, apps, session, grant: response.result.value });
        announce(`${app.displayName} 已就绪`);
      } else if (response.result.case === "error") {
        if (isAuthenticationError(response.result.value.code)) {
          safeRevoke(app.appId);
          expireSession();
          return;
        }
        setView({ kind: appFailureKind(response.result.value.code), app, apps, session, error: response.result.value });
        announce(`${app.displayName} 启动被拒绝`);
      } else {
        setView({ kind: "app-load-error", app, apps, session, error: localError(ErrorCode.INTERNAL, "应用授权响应不完整") });
        announce(`${app.displayName} 授权响应不完整`);
      }
    } catch {
      if (!isCurrent(operation, session)) {
        safeRevoke(app.appId);
        return;
      }
      setView({ kind: "app-unavailable", app, apps, session, error: create(SafeErrorSchema, networkFailure()) });
      announce("应用授权连接失败");
    }
  }

  function returnToRegistry(session: Session, apps: AppDescriptor[], revokeAppId?: string) {
    invalidatePending();
    if (revokeAppId) safeRevoke(revokeAppId);
    setView({ kind: "registry-ready", session, apps });
    announce("已返回应用目录");
    requestAnimationFrame(() => {
      const button = [...document.querySelectorAll<HTMLButtonElement>("[data-open-app-id]")]
        .find((candidate) => candidate.dataset.openAppId === lastOpenAppId.current);
      button?.focus();
    });
  }

  function handleFrameFailure(ready: Extract<View, { kind: "app-ready" }>) {
    invalidatePending();
    safeRevoke(ready.app.appId);
    setView({
      kind: "app-load-error",
      session: ready.session,
      app: ready.app,
      apps: ready.apps,
      error: localError(ErrorCode.UNAVAILABLE, "应用内容加载失败", true),
    });
    announce("应用内容加载失败");
  }

  const session = "session" in view ? view.session : undefined;
  const sessionBoundaryActive = view.kind === "booting" || view.kind.startsWith("session");
  return (
    <div className="shell" data-transport={transport} data-contract={PlatformService.typeName} data-shell-state={view.kind}>
      <header className="masthead">
        <div>
          <p className="product-mark">ficant / 固定收益研究</p>
          <h1>应用目录</h1>
        </div>
        <div className="session-readout" aria-label="会话状态">
          <span>SESSION</span>
          <strong>{session ? "ACTIVE" : view.kind === "session-expired" ? "EXPIRED" : "CHECKING"}</strong>
          <small>{session ? `有效至 ${sessionExpiryLabel(session)}` : "安全边界"}</small>
        </div>
      </header>

      <nav className="boundary-rail" aria-label="应用加载边界">
        <span data-active={sessionBoundaryActive} aria-current={sessionBoundaryActive ? "step" : undefined}>会话</span>
        <span data-active={view.kind.startsWith("registry")} aria-current={view.kind.startsWith("registry") ? "step" : undefined}>目录</span>
        <span data-active={view.kind.startsWith("app")} aria-current={view.kind.startsWith("app") ? "step" : undefined}>应用</span>
      </nav>

      <p className="live-status" role="status" aria-live="polite" aria-atomic="true">{announcement}</p>

      <main className="workspace">
        {view.kind === "booting" ? <LoadingPanel label="正在验证会话" /> : null}
        {view.kind === "registry-loading" ? <LoadingPanel label="正在读取应用目录" /> : null}
        {view.kind === "registry-empty" ? (
          <section className="empty-panel">
            <p className="eyebrow">REGISTRY / 0</p>
            <h2>当前没有可用应用</h2>
            <p>平台只显示当前会话由服务端授权的应用。请联系管理员分配应用后重新读取。</p>
            <button className="primary-action" type="button" onClick={() => void loadRegistry(view.session)}>重新读取应用目录</button>
          </section>
        ) : null}
        {view.kind === "registry-ready" ? <RegistryList view={view} onOpen={openApp} /> : null}
        {view.kind === "registry-error" ? (
          <SafeErrorPanel error={view.error} onRetry={() => void loadRegistry(view.session)} retryLabel="重新读取应用目录" />
        ) : null}
        {view.kind === "session-expired" ? (
          <section className="empty-panel">
            <p className="eyebrow">SESSION / EXPIRED</p>
            <h2>会话已过期</h2>
            <p>短期应用材料已经清除。重新认证后，平台会再次读取可见应用和 capability。</p>
            <button className="primary-action" type="button" onClick={() => void bootstrap()}>重新认证</button>
          </section>
        ) : null}
        {view.kind === "session-error" ? <SafeErrorPanel error={view.error} onRetry={() => void bootstrap()} retryLabel="重新读取会话" /> : null}
        {view.kind === "app-authorizing" ? <LoadingPanel label={`正在授权 ${view.app.displayName}`} /> : null}
        {view.kind === "app-ready" ? (
          <section className="app-stage" aria-labelledby="app-stage-title">
            <div className="stage-toolbar">
              <div><p className="eyebrow">APP / AUTHORIZED</p><h2 id="app-stage-title">{view.app.displayName}</h2></div>
              <div className="action-row">
                <button className="secondary-action" type="button" onClick={() => frameRef.current?.focus()}>进入{view.app.displayName}</button>
                <button className="secondary-action" type="button" onClick={() => returnToRegistry(view.session, view.apps, view.app.appId)}>返回应用列表</button>
              </div>
            </div>
            <AppFrame
              ref={frameRef}
              app={view.app}
              grant={view.grant}
              now={now().getTime()}
              onLoadError={() => handleFrameFailure(view)}
            />
          </section>
        ) : null}
        {isAppFailure(view) ? (
          <section>
            <SafeErrorPanel error={view.error} />
            <button className="secondary-action return-action" type="button" onClick={() => returnToRegistry(
              view.session,
              view.apps,
              view.kind === "app-load-error" ? view.app.appId : undefined,
            )}>返回应用列表</button>
          </section>
        ) : null}
      </main>

      <footer className="boundary-note">
        <span>客户端不推导最终授权</span>
        <span>gRPC-Web / short-lived grant</span>
      </footer>
    </div>
  );
}

function LoadingPanel({ label }: { label: string }) {
  return <section className="loading-panel"><span className="loading-rule" aria-hidden="true" /><p>{label}</p></section>;
}

function RegistryList({
  view,
  onOpen,
}: {
  view: Extract<View, { kind: "registry-ready" }>;
  onOpen: (app: AppDescriptor, apps: AppDescriptor[], session: Session, button: HTMLButtonElement) => void;
}) {
  return (
    <section aria-labelledby="available-apps-title">
      <div className="section-heading"><p className="eyebrow">REGISTRY / {view.apps.length}</p><h2 id="available-apps-title">当前可用应用</h2></div>
      <ul className="app-list">
        {view.apps.map((app) => (
          <li key={app.appId}>
            <div><h3>{app.displayName}</h3><p>{app.capabilities.join(" · ") || "无附加 capability"}</p></div>
            <button data-open-app-id={app.appId} className="primary-action" type="button" onClick={(event) => void onOpen(app, view.apps, view.session, event.currentTarget)}>打开{app.displayName}</button>
          </li>
        ))}
      </ul>
    </section>
  );
}

function localError(code: ErrorCode, safeMessage: string, retryable = false): SafeError {
  return create(SafeErrorSchema, { code, safeMessage, traceId: "", retryable });
}

function isAppFailure(view: View): view is AppFailureView {
  return ["app-forbidden", "app-unavailable", "app-load-error"].includes(view.kind);
}

function isAuthenticationError(code: ErrorCode): boolean {
  return code === ErrorCode.UNAUTHENTICATED || code === ErrorCode.EXPIRED;
}

function appFailureKind(code: ErrorCode): AppFailureView["kind"] {
  if (code === ErrorCode.FORBIDDEN) return "app-forbidden";
  if (code === ErrorCode.UNAVAILABLE) return "app-unavailable";
  return "app-load-error";
}
