import { create } from "@bufbuild/protobuf";
import { createClient } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import {
  AuthorizeAppLaunchRequestSchema,
  ErrorCode,
  GetAppRegistryRequestSchema,
  GetCurrentSessionRequestSchema,
  PlatformService,
  RefreshAppLaunchRequestSchema,
  RefreshSessionRequestSchema,
  RevokeAppLaunchRequestSchema,
  RevokeSessionRequestSchema,
  type AppLaunchAuthorizationResponse,
  type GetAppRegistryResponse,
  type GetCurrentSessionResponse,
  type RefreshSessionResponse,
  type RevokeAppLaunchResponse,
  type RevokeSessionResponse,
} from "../../packages/contracts-generated/src/ficant/app/v1/registry_pb";

export interface PlatformClient {
  getCurrentSession(signal?: AbortSignal): Promise<GetCurrentSessionResponse>;
  refreshSession(signal?: AbortSignal): Promise<RefreshSessionResponse>;
  revokeSession(signal?: AbortSignal): Promise<RevokeSessionResponse>;
  getAppRegistry(signal?: AbortSignal): Promise<GetAppRegistryResponse>;
  authorizeAppLaunch(appId: string, signal?: AbortSignal): Promise<AppLaunchAuthorizationResponse>;
  refreshAppLaunch(appId: string, signal?: AbortSignal): Promise<AppLaunchAuthorizationResponse>;
  revokeAppLaunch(appId: string, signal?: AbortSignal): Promise<RevokeAppLaunchResponse>;
}

export function createGrpcWebPlatformClient(baseUrl: string): PlatformClient {
  const normalizedBaseUrl = validateBaseUrl(baseUrl);
  const transport = createGrpcWebTransport({
    baseUrl: normalizedBaseUrl,
    useBinaryFormat: true,
  });
  const client = createClient(PlatformService, transport);

  return {
    getCurrentSession: (signal) => client.getCurrentSession(create(GetCurrentSessionRequestSchema), { signal }),
    refreshSession: (signal) => client.refreshSession(create(RefreshSessionRequestSchema), { signal }),
    revokeSession: (signal) => client.revokeSession(create(RevokeSessionRequestSchema), { signal }),
    getAppRegistry: (signal) => client.getAppRegistry(create(GetAppRegistryRequestSchema), { signal }),
    authorizeAppLaunch: (appId, signal) => client.authorizeAppLaunch(create(AuthorizeAppLaunchRequestSchema, { appId }), { signal }),
    refreshAppLaunch: (appId, signal) => client.refreshAppLaunch(create(RefreshAppLaunchRequestSchema, { appId }), { signal }),
    revokeAppLaunch: (appId, signal) => client.revokeAppLaunch(create(RevokeAppLaunchRequestSchema, { appId }), { signal }),
  };
}

function validateBaseUrl(value: string): string {
  const url = new URL(value, window.location.origin);
  if (url.username || url.password || url.search || url.hash) {
    throw new Error("gRPC-Web base URL 不得包含凭据、查询参数或片段");
  }
  if (url.protocol !== "https:" && !isLoopbackHttp(url)) {
    throw new Error("gRPC-Web base URL 必须使用 HTTPS；本机回环开发环境除外");
  }
  return url.origin + url.pathname.replace(/\/$/, "");
}

function isLoopbackHttp(url: URL): boolean {
  return url.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
}

export function networkFailure(): {
  code: ErrorCode;
  safeMessage: string;
  traceId: string;
  retryable: boolean;
} {
  return {
    code: ErrorCode.UNAVAILABLE,
    safeMessage: "平台连接暂时不可用",
    traceId: "",
    retryable: true,
  };
}
