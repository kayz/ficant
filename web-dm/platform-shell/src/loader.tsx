import { forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef } from "react";
import type {
  AppDescriptor,
  AppLaunchGrant,
  CspDirective,
} from "../../packages/contracts-generated/src/ficant/app/v1/registry_pb";
import { timestampMilliseconds } from "./session";

const ALLOWED_SANDBOX_TOKENS = new Set([
  "allow-downloads",
  "allow-forms",
  "allow-modals",
  "allow-same-origin",
  "allow-scripts",
]);
const ALLOWED_CSP_DIRECTIVES = new Set([
  "default-src",
  "connect-src",
  "font-src",
  "frame-src",
  "img-src",
  "script-src",
  "style-src",
]);

export interface ValidatedLaunchBoundary {
  src: string;
  csp: string;
  sandbox: string;
  credential: Uint8Array;
  scopes: readonly string[];
  allowedOrigin: string;
  expiresAt: number;
}

export function validateLaunchBoundary(app: AppDescriptor, grant: AppLaunchGrant, now = Date.now()): ValidatedLaunchBoundary {
  if (!app.appId || grant.appId !== app.appId) throw new Error("启动授权的应用标识不匹配");
  if (grant.entrypoint !== app.entrypoint || grant.allowedOrigin !== app.allowedOrigin) {
    throw new Error("启动授权放宽了 Registry 固定的入口或 origin");
  }
  const origin = validateOrigin(grant.allowedOrigin);
  const src = validateEntrypoint(grant.entrypoint, origin);
  if (grant.scopes.some((scope) => !app.capabilities.includes(scope))) {
    throw new Error("启动授权包含 Registry 未声明的 capability");
  }
  const issuedAt = timestampMilliseconds(grant.issuedAt);
  const expiresAt = timestampMilliseconds(grant.expiresAt);
  if (issuedAt === undefined || expiresAt === undefined || expiresAt <= issuedAt || expiresAt <= now) {
    throw new Error("启动授权期限无效");
  }
  if (grant.launchCredential.byteLength === 0) throw new Error("启动授权缺少短期凭据");
  const sandbox = validateSandbox(grant.sandboxTokens);
  const csp = validateCsp(grant.cspDirectives);
  return {
    src,
    csp,
    sandbox,
    credential: grant.launchCredential,
    scopes: grant.scopes,
    allowedOrigin: origin,
    expiresAt,
  };
}

function validateOrigin(value: string): string {
  const url = new URL(value);
  if (url.origin !== value || url.pathname !== "/" || url.search || url.hash || url.username || url.password) {
    throw new Error("allowed_origin 必须是精确 origin");
  }
  if (url.protocol !== "https:" && !(url.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname))) {
    throw new Error("allowed_origin 必须使用 HTTPS；本机回环测试除外");
  }
  return url.origin;
}

function validateEntrypoint(value: string, origin: string): string {
  if (!value.startsWith("/") || value.startsWith("//") || value.includes("#") || value.includes("?")) {
    throw new Error("entrypoint 必须是不含 authority、query 或 fragment 的绝对路径");
  }
  const url = new URL(value, origin);
  if (url.origin !== origin) throw new Error("entrypoint 越过授权 origin");
  return url.href;
}

function validateSandbox(tokens: readonly string[]): string {
  const unique = [...new Set(tokens)];
  if (unique.length === 0 || unique.some((token) => !ALLOWED_SANDBOX_TOKENS.has(token))) {
    throw new Error("sandbox 包含未批准或为空的能力集合");
  }
  if (unique.includes("allow-scripts") && unique.includes("allow-same-origin")) {
    throw new Error("sandbox 不得同时放开脚本与同源能力");
  }
  return unique.join(" ");
}

function validateCsp(directives: readonly CspDirective[]): string {
  const seen = new Set<string>();
  for (const directive of directives) {
    if (!ALLOWED_CSP_DIRECTIVES.has(directive.name) || seen.has(directive.name) || directive.values.length === 0) {
      throw new Error("CSP 指令无效、重复或为空");
    }
    if (directive.values.some((value) => !isSafeCspSource(value))) {
      throw new Error("CSP 包含会放宽应用边界的值");
    }
    seen.add(directive.name);
  }
  const defaults = directives.find((directive) => directive.name === "default-src");
  if (!defaults || defaults.values.length !== 1 || defaults.values[0] !== "'none'") {
    throw new Error("CSP 必须以 default-src 'none' 收口");
  }
  return directives.map(({ name, values }) => `${name} ${values.join(" ")}`).join("; ");
}

function isSafeCspSource(value: string): boolean {
  if (value === "'none'" || value === "'self'") return true;
  if (!value || /[\s;]/.test(value) || value.includes("*")) return false;
  try {
    const url = new URL(value);
    if (url.origin !== value || url.username || url.password || url.search || url.hash) return false;
    return url.protocol === "https:"
      || (url.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname));
  } catch {
    return false;
  }
}

interface AppFrameProps {
  app: AppDescriptor;
  grant: AppLaunchGrant;
  now?: number;
  onLoadError: () => void;
}

export const AppFrame = forwardRef<HTMLIFrameElement, AppFrameProps>(function AppFrame(
  { app, grant, now = Date.now(), onLoadError },
  ref,
) {
  const boundary = useMemo(() => validateLaunchBoundary(app, grant, now), [app, grant, now]);
  const internalRef = useRef<HTMLIFrameElement>(null);
  const loadedRef = useRef(false);
  useImperativeHandle(ref, () => internalRef.current as HTMLIFrameElement);

  useLayoutEffect(() => {
    const frame = internalRef.current;
    if (!frame) return;
    frame.addEventListener("error", onLoadError);
    return () => frame.removeEventListener("error", onLoadError);
  }, [onLoadError]);

  useEffect(() => {
    if (loadedRef.current) postCredential(internalRef.current);
  }, [boundary]);

  function handOffCredential(event: React.SyntheticEvent<HTMLIFrameElement>) {
    loadedRef.current = true;
    postCredential(event.currentTarget);
  }

  function postCredential(frame: HTMLIFrameElement | null) {
    frame?.contentWindow?.postMessage({
      type: "ficant.app.launch.v1",
      credential: boundary.credential,
      scopes: [...boundary.scopes],
      expiresAt: boundary.expiresAt,
    }, boundary.allowedOrigin);
  }

  return (
    <iframe
      ref={internalRef}
      className="app-frame"
      src={boundary.src}
      title={app.displayName}
      sandbox={boundary.sandbox}
      referrerPolicy="no-referrer"
      {...{ csp: boundary.csp }}
      onLoad={handOffCredential}
    />
  );
});
