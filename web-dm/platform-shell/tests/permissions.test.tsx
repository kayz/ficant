import { create } from "@bufbuild/protobuf";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  AppLaunchAuthorizationResponseSchema,
  ErrorCode,
} from "../../packages/contracts-generated/src/ficant/app/v1/registry_pb";
import { PlatformShell } from "../src/app";
import { validateLaunchBoundary } from "../src/loader";
import { currentSession, fixtureApp, fixtureClient, launchGrant, safeFailure } from "./fixtures/platform";

describe("Q2-WEB-03 权限与应用边界安全", () => {
  it("会话在授权返回前过期时拒绝迟到 grant 并尽力撤权", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-07-12T02:00:00.000Z"));
    let iframeCreations = 0;
    const observer = new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) {
          if (node instanceof HTMLIFrameElement) iframeCreations += 1;
          if (node instanceof HTMLElement) iframeCreations += node.querySelectorAll("iframe").length;
        }
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
    try {
      let release!: (value: ReturnType<typeof launchGrant>) => void;
      const authorization = new Promise<ReturnType<typeof launchGrant>>((resolve) => { release = resolve; });
      const revokeAppLaunch = vi.fn(async () => { throw new Error("fixture records best-effort revoke"); });
      const client = fixtureClient({
        getCurrentSession: async () => currentSession(65_000),
        authorizeAppLaunch: async () => authorization,
        revokeAppLaunch,
      });
      render(<PlatformShell client={client} now={() => new Date()} />);

      fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));
      act(() => { vi.advanceTimersByTime(66_000); });
      expect(screen.getByRole("heading", { name: "会话已过期" })).toBeInTheDocument();

      await act(async () => { release(launchGrant()); });
      expect(screen.getByRole("heading", { name: "会话已过期" })).toBeInTheDocument();
      expect(screen.queryByTitle("利率研究测试应用")).not.toBeInTheDocument();
      expect(iframeCreations).toBe(0);
      await waitFor(() => expect(revokeAppLaunch).toHaveBeenCalledWith("fixture-rates-lab"));
    } finally {
      observer.disconnect();
      vi.useRealTimers();
    }
  });

  it("在 grant 独立到期前刷新并重新验证完整启动边界", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-07-12T02:00:00.000Z"));
    try {
      const refreshAppLaunch = vi.fn(async () => launchGrant({
        allowedOrigin: "https://attacker.invalid",
      }));
      const revokeAppLaunch = vi.fn(async () => { throw new Error("fixture records best-effort revoke"); });
      const client = fixtureClient({ refreshAppLaunch, revokeAppLaunch });
      render(<PlatformShell client={client} now={() => new Date()} />);
      fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));
      expect(await screen.findByTitle("利率研究测试应用")).toBeInTheDocument();
      await act(async () => { await Promise.resolve(); });

      act(() => { vi.advanceTimersByTime(26_000); });
      await waitFor(() => expect(refreshAppLaunch).toHaveBeenCalledWith("fixture-rates-lab", expect.any(AbortSignal)));
      expect(await screen.findByRole("alert")).toHaveTextContent("应用启动边界验证失败");
      expect(screen.queryByTitle("利率研究测试应用")).not.toBeInTheDocument();
      expect(revokeAppLaunch).toHaveBeenCalledWith("fixture-rates-lab");
    } finally {
      vi.useRealTimers();
    }
  });

  it("grant 刷新返回未认证时立即卸载 iframe 并要求重新认证", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-07-12T02:00:00.000Z"));
    try {
      const refreshAppLaunch = vi.fn(async () => create(AppLaunchAuthorizationResponseSchema, {
        result: {
          case: "error",
          value: safeFailure(ErrorCode.UNAUTHENTICATED, "应用授权已经失效", "trace-grant-expired"),
        },
      }));
      const revokeAppLaunch = vi.fn(async () => { throw new Error("fixture records best-effort revoke"); });
      render(<PlatformShell client={fixtureClient({ refreshAppLaunch, revokeAppLaunch })} now={() => new Date()} />);
      fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));
      expect(await screen.findByTitle("利率研究测试应用")).toBeInTheDocument();
      await act(async () => { await Promise.resolve(); });

      act(() => { vi.advanceTimersByTime(26_000); });
      expect(await screen.findByRole("heading", { name: "会话已过期" })).toBeInTheDocument();
      expect(screen.queryByTitle("利率研究测试应用")).not.toBeInTheDocument();
      expect(revokeAppLaunch).toHaveBeenCalledWith("fixture-rates-lab");
    } finally {
      vi.useRealTimers();
    }
  });

  it("grant 刷新暂时失败时卸载 iframe、撤权并显示安全错误", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-07-12T02:00:00.000Z"));
    try {
      const refreshAppLaunch = vi.fn(async () => create(AppLaunchAuthorizationResponseSchema, {
        result: {
          case: "error",
          value: safeFailure(ErrorCode.UNAVAILABLE, "应用授权刷新暂时不可用", "trace-grant-refresh-503", true),
        },
      }));
      const revokeAppLaunch = vi.fn(async () => { throw new Error("fixture records best-effort revoke"); });
      render(<PlatformShell client={fixtureClient({ refreshAppLaunch, revokeAppLaunch })} now={() => new Date()} />);
      fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));
      expect(await screen.findByTitle("利率研究测试应用")).toBeInTheDocument();
      await act(async () => { await Promise.resolve(); });

      act(() => { vi.advanceTimersByTime(26_000); });
      const alert = await screen.findByRole("alert");
      expect(alert).toHaveTextContent("应用授权刷新暂时不可用");
      expect(alert).toHaveTextContent("trace-grant-refresh-503");
      expect(screen.queryByTitle("利率研究测试应用")).not.toBeInTheDocument();
      expect(revokeAppLaunch).toHaveBeenCalledWith("fixture-rates-lab");
    } finally {
      vi.useRealTimers();
    }
  });

  it("合法刷新 grant 延长期限并把新短期凭据交接给已加载应用", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-07-12T02:00:00.000Z"));
    try {
      const refreshedCredential = new TextEncoder().encode("refreshed-short-lived-credential");
      const refreshAppLaunch = vi.fn(async () => launchGrant({ launchCredential: refreshedCredential }));
      render(<PlatformShell client={fixtureClient({ refreshAppLaunch })} now={() => new Date()} />);
      fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));
      const frame = await screen.findByTitle("利率研究测试应用") as HTMLIFrameElement;
      const postMessage = vi.spyOn(frame.contentWindow!, "postMessage");
      fireEvent.load(frame);
      await act(async () => { await Promise.resolve(); });

      act(() => { vi.advanceTimersByTime(26_000); });
      await waitFor(() => expect(refreshAppLaunch).toHaveBeenCalledWith("fixture-rates-lab", expect.any(AbortSignal)));
      await waitFor(() => expect(postMessage.mock.calls.some(([message]) =>
        Array.from(message.credential as Uint8Array).join(",") === Array.from(refreshedCredential).join(","),
      )).toBe(true));
      expect(screen.getByTitle("利率研究测试应用")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("刷新请求悬挂超过 grant 硬期限时立即卸载 iframe 并取消刷新", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-07-12T02:00:00.000Z"));
    try {
      let release!: (value: ReturnType<typeof launchGrant>) => void;
      let refreshSignal: AbortSignal | undefined;
      const refresh = new Promise<ReturnType<typeof launchGrant>>((resolve) => { release = resolve; });
      const revokeAppLaunch = vi.fn(async () => { throw new Error("fixture records best-effort revoke"); });
      const client = fixtureClient({
        refreshAppLaunch: async (_appId, signal) => {
          refreshSignal = signal;
          return refresh;
        },
        revokeAppLaunch,
      });
      render(<PlatformShell client={client} now={() => new Date()} />);
      fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));
      expect(await screen.findByTitle("利率研究测试应用")).toBeInTheDocument();
      await act(async () => { await Promise.resolve(); });
      act(() => { vi.advanceTimersByTime(26_000); });
      await waitFor(() => expect(refreshSignal).toBeInstanceOf(AbortSignal));

      act(() => { vi.advanceTimersByTime(5_000); });
      expect(await screen.findByRole("alert")).toHaveTextContent("应用授权已过期");
      expect(screen.queryByTitle("利率研究测试应用")).not.toBeInTheDocument();
      expect(refreshSignal?.aborted).toBe(true);
      expect(revokeAppLaunch).toHaveBeenCalledWith("fixture-rates-lab");

      await act(async () => { release(launchGrant()); });
      expect(screen.queryByTitle("利率研究测试应用")).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("离开应用会取消 pending refresh，迟到结果不能恢复 iframe", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-07-12T02:00:00.000Z"));
    try {
      let release!: (value: ReturnType<typeof launchGrant>) => void;
      let refreshSignal: AbortSignal | undefined;
      const refresh = new Promise<ReturnType<typeof launchGrant>>((resolve) => { release = resolve; });
      const client = fixtureClient({
        refreshAppLaunch: async (_appId, signal) => {
          refreshSignal = signal;
          return refresh;
        },
      });
      render(<PlatformShell client={client} now={() => new Date()} />);
      fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));
      expect(await screen.findByTitle("利率研究测试应用")).toBeInTheDocument();
      await act(async () => { await Promise.resolve(); });
      act(() => { vi.advanceTimersByTime(26_000); });
      await waitFor(() => expect(refreshSignal).toBeInstanceOf(AbortSignal));

      fireEvent.click(screen.getByRole("button", { name: "返回应用列表" }));
      expect(refreshSignal?.aborted).toBe(true);
      await act(async () => { release(launchGrant()); });
      expect(screen.queryByTitle("利率研究测试应用")).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("组件卸载会取消 pending 平台请求", async () => {
    let requestSignal: AbortSignal | undefined;
    let release!: (value: ReturnType<typeof currentSession>) => void;
    const current = new Promise<ReturnType<typeof currentSession>>((resolve) => { release = resolve; });
    const client = fixtureClient({
      getCurrentSession: async (signal) => {
        requestSignal = signal;
        return current;
      },
    });
    const { unmount } = render(<PlatformShell client={client} now={() => new Date()} />);
    await waitFor(() => expect(requestSignal).toBeInstanceOf(AbortSignal));
    unmount();
    expect(requestSignal?.aborted).toBe(true);
    release(currentSession());
  });

  it("只按服务端授权结果打开应用，拒绝时不创建 iframe", async () => {
    const client = fixtureClient({
      authorizeAppLaunch: async () => create(AppLaunchAuthorizationResponseSchema, {
        result: {
          case: "error",
          value: safeFailure(ErrorCode.FORBIDDEN, "当前会话没有启动此应用的权限", "trace-auth-403"),
        },
      }),
    });
    render(<PlatformShell client={client} now={() => new Date()} />);

    fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("当前会话没有启动此应用的权限");
    expect(document.querySelector("[data-shell-state=app-forbidden]")).toBeInTheDocument();
    expect(screen.queryByTitle("利率研究测试应用")).not.toBeInTheDocument();
  });

  it("把服务端不可用与 iframe 加载失败保留为不同状态", async () => {
    const client = fixtureClient({
      authorizeAppLaunch: async () => create(AppLaunchAuthorizationResponseSchema, {
        result: {
          case: "error",
          value: safeFailure(ErrorCode.UNAVAILABLE, "应用运行环境暂时不可用", "trace-app-503", true),
        },
      }),
    });
    render(<PlatformShell client={client} now={() => new Date()} />);
    fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("应用运行环境暂时不可用");
    expect(document.querySelector("[data-shell-state=app-unavailable]")).toBeInTheDocument();
  });

  it("授权 grant 越界时只关闭该应用，不使 Shell 崩溃", async () => {
    const client = fixtureClient({
      authorizeAppLaunch: async () => launchGrant({ allowedOrigin: "https://attacker.invalid" }),
    });
    render(<PlatformShell client={client} now={() => new Date()} />);
    fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("应用启动边界验证失败");
    expect(document.querySelector("[data-shell-state=app-load-error]")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "应用目录" })).toBeInTheDocument();
  });

  it("拒绝 origin、entrypoint、scope、期限、CSP 或 sandbox 被放宽的 grant", () => {
    const cases = [
      launchGrant({ allowedOrigin: "https://attacker.invalid" }),
      launchGrant({ entrypoint: "https://attacker.invalid/steal" }),
      launchGrant({ scopes: ["platform.admin"] }),
      launchGrant({ expiresAt: undefined }),
      launchGrant({ cspDirectives: [{ name: "script-src", values: ["'unsafe-eval'"] }] }),
      launchGrant({ cspDirectives: [
        { name: "default-src", values: ["'none'"] },
        { name: "script-src", values: ["'self'; connect-src *"] },
      ] }),
      launchGrant({ sandboxTokens: ["allow-top-navigation"] }),
      launchGrant({ sandboxTokens: ["allow-scripts", "allow-same-origin"] }),
    ];

    for (const response of cases) {
      expect(() => validateLaunchBoundary(fixtureApp, response.result.case === "grant" ? response.result.value : neverGrant())).toThrow();
    }
  });

  it("短期启动凭据不进入 URL 或 localStorage", async () => {
    localStorage.setItem("unrelated", "keep");
    render(<PlatformShell client={fixtureClient()} now={() => new Date()} />);
    fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));

    const frame = await screen.findByTitle("利率研究测试应用") as HTMLIFrameElement;
    expect(frame).toHaveAttribute("csp", "default-src 'none'; script-src 'self'; style-src 'self'");
    const postMessage = vi.spyOn(frame.contentWindow!, "postMessage");
    fireEvent.load(frame);
    expect(frame.getAttribute("src")).not.toContain("credential");
    expect(frame.getAttribute("src")).not.toContain("token");
    expect(JSON.stringify({ ...localStorage })).not.toContain("short-lived-fixture-credential");
    expect(postMessage).toHaveBeenCalledTimes(1);
    const [message, targetOrigin] = postMessage.mock.calls[0];
    expect(message.type).toBe("ficant.app.launch.v1");
    expect(Array.from(message.credential as Uint8Array)).toEqual(Array.from(new TextEncoder().encode("short-lived-fixture-credential")));
    expect(targetOrigin).toBe(fixtureApp.allowedOrigin);
  });
});

function neverGrant(): never {
  throw new Error("test fixture must contain a grant");
}
