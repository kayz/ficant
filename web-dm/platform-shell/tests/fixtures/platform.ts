import { create } from "@bufbuild/protobuf";
import { timestampFromDate, type Timestamp } from "@bufbuild/protobuf/wkt";
import {
  AppDescriptorSchema,
  AppLaunchAuthorizationResponseSchema,
  AppLaunchGrantSchema,
  AppRegistrySchema,
  ErrorCode,
  GetAppRegistryResponseSchema,
  GetCurrentSessionResponseSchema,
  RefreshSessionResponseSchema,
  SafeErrorSchema,
  type AppDescriptor,
  type AppLaunchAuthorizationResponse,
  type GetAppRegistryResponse,
  type GetCurrentSessionResponse,
  type RefreshSessionResponse,
} from "../../../packages/contracts-generated/src/ficant/app/v1/registry_pb";
import { SessionSchema } from "../../../packages/contracts-generated/src/ficant/app/v1/session_pb";
import type { PlatformClient } from "../../src/registry";

export const fixtureApp: AppDescriptor = create(AppDescriptorSchema, {
  appId: "fixture-rates-lab",
  displayName: "利率研究测试应用",
  entrypoint: "/tests/fixtures/embedded.html",
  allowedOrigin: "http://127.0.0.1:4173",
  capabilities: ["market.read", "research.run"],
});

export const longFixtureDisplayName = "跨市场固定收益情景分析与压力测试研究工作台InternationalRatesScenarioResearchWorkbench";
export const longFixtureCapability = "research.cross_market_scenario_analysis_with_replay_and_attribution_without_breakpoints";
export const longFixtureSafeMessage = "研究任务暂时无法恢复，请核对授权范围后重试。ResearchWorkspaceTemporarilyUnavailableWithoutExposingInternalDetails";
export const longFixtureTrace = "trace-long-fixture-0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

export const longFixtureApp: AppDescriptor = create(AppDescriptorSchema, {
  appId: "fixture-long-rates-research-workbench",
  displayName: longFixtureDisplayName,
  entrypoint: "/tests/fixtures/embedded.html",
  allowedOrigin: "http://127.0.0.1:4173",
  capabilities: [longFixtureCapability, "跨市场压力测试与归因分析能力"],
});

export function safeFailure(
  code: ErrorCode,
  safeMessage: string,
  traceId = "trace-fixture-001",
  retryable = false,
) {
  return create(SafeErrorSchema, { code, safeMessage, traceId, retryable });
}

export function currentSession(expiresInMs = 300_000, now = new Date()): GetCurrentSessionResponse {
  return create(GetCurrentSessionResponseSchema, {
    result: {
      case: "session",
      value: create(SessionSchema, {
        sessionId: "session-fixture",
        subjectId: "researcher-fixture",
        scopes: ["market.read", "research.run"],
        issuedAt: timestampFromDate(new Date(now.getTime() - 60_000)),
        expiresAt: timestampFromDate(new Date(now.getTime() + expiresInMs)),
      }),
    },
  });
}

export function registry(apps: AppDescriptor[] = [fixtureApp]): GetAppRegistryResponse {
  return create(GetAppRegistryResponseSchema, {
    result: { case: "registry", value: create(AppRegistrySchema, { apps }) },
  });
}

export function launchGrant(
  overrides: LaunchGrantOverrides = {},
  app: AppDescriptor = fixtureApp,
  now = new Date(),
): AppLaunchAuthorizationResponse {
  const grant = create(AppLaunchGrantSchema, {
    appId: app.appId,
    entrypoint: app.entrypoint,
    allowedOrigin: app.allowedOrigin,
    scopes: app.capabilities,
    issuedAt: timestampFromDate(now),
    expiresAt: timestampFromDate(new Date(now.getTime() + 30_000)),
    launchCredential: new TextEncoder().encode("short-lived-fixture-credential"),
    cspDirectives: [
      { name: "default-src", values: ["'none'"] },
      { name: "script-src", values: ["'self'"] },
      { name: "style-src", values: ["'self'"] },
    ],
    sandboxTokens: ["allow-scripts"],
    ...overrides,
  });
  return create(AppLaunchAuthorizationResponseSchema, {
    result: { case: "grant", value: grant },
  });
}

interface LaunchGrantOverrides {
  appId?: string;
  entrypoint?: string;
  allowedOrigin?: string;
  scopes?: string[];
  issuedAt?: Timestamp;
  expiresAt?: Timestamp;
  launchCredential?: Uint8Array;
  cspDirectives?: Array<{ name: string; values: string[] }>;
  sandboxTokens?: string[];
}

export function fixtureClient(
  overrides: Partial<PlatformClient> = {},
): PlatformClient {
  const refresh: RefreshSessionResponse = create(RefreshSessionResponseSchema, {
    result: currentSession().result,
  });
  return {
    getCurrentSession: async () => currentSession(),
    refreshSession: async () => refresh,
    revokeSession: async () => {
      throw new Error("fixture does not revoke unless a test asks it to");
    },
    getAppRegistry: async () => registry(),
    authorizeAppLaunch: async () => launchGrant(),
    refreshAppLaunch: async () => launchGrant(),
    revokeAppLaunch: async () => {
      throw new Error("fixture does not revoke unless a test asks it to");
    },
    ...overrides,
  };
}
