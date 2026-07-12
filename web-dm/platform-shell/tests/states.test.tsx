import { create } from "@bufbuild/protobuf";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useLayoutEffect } from "react";
import { describe, expect, it, vi } from "vitest";
import {
  ErrorCode,
  GetAppRegistryResponseSchema,
} from "../../packages/contracts-generated/src/ficant/app/v1/registry_pb";
import { PlatformShell } from "../src/app";
import { AppFrame } from "../src/loader";
import {
  currentSession,
  fixtureApp,
  fixtureClient,
  launchGrant,
  registry,
  safeFailure,
} from "./fixtures/platform";
import { RefreshSessionResponseSchema } from "../../packages/contracts-generated/src/ficant/app/v1/registry_pb";

describe("Q2-WEB-01 Platform Shell 业务状态", () => {
  it("默认系统时钟在状态重渲染时保持稳定，不重复读取会话", async () => {
    let sessionReads = 0;
    const client = fixtureClient({
      getCurrentSession: async () => {
        sessionReads += 1;
        return currentSession();
      },
    });
    render(<PlatformShell client={client} />);
    await screen.findByRole("button", { name: "打开利率研究测试应用" });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(sessionReads).toBe(1);
  });

  it("从会话与目录加载态进入合法空目录，而不是注册假应用", async () => {
    let releaseRegistry!: () => void;
    const registryWait = new Promise<void>((resolve) => {
      releaseRegistry = resolve;
    });
    const client = fixtureClient({
      getAppRegistry: async () => {
        await registryWait;
        return registry([]);
      },
    });

    render(<PlatformShell client={client} now={() => new Date()} />);
    expect(await screen.findByText("正在读取应用目录")).toBeInTheDocument();

    releaseRegistry();
    expect(await screen.findByRole("heading", { name: "当前没有可用应用" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /打开/ })).not.toBeInTheDocument();
  });

  it("显示安全错误 code、可复制 trace 与受控恢复动作", async () => {
    const error = safeFailure(ErrorCode.UNAVAILABLE, "应用目录暂时不可用", "trace-registry-031", true);
    const client = fixtureClient({
      getAppRegistry: async () => create(GetAppRegistryResponseSchema, {
        result: { case: "error", value: error },
      }),
    });

    render(<PlatformShell client={client} now={() => new Date()} />);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("应用目录暂时不可用");
    expect(alert).toHaveTextContent("ERROR_CODE_UNAVAILABLE");
    expect(alert).toHaveTextContent("trace-registry-031");
    expect(screen.getByRole("button", { name: "重新读取应用目录" })).toBeInTheDocument();
  });

  it("完成授权后加载测试应用，并把加载失败隔离在应用边界", async () => {
    const client = fixtureClient();
    render(<PlatformShell client={client} now={() => new Date()} />);

    fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));
    const frame = await screen.findByTitle("利率研究测试应用");
    expect(frame).toHaveAttribute("sandbox", "allow-scripts");

    fireEvent.error(frame);
    expect(await screen.findByRole("alert")).toHaveTextContent("应用内容加载失败");
    expect(screen.getByRole("button", { name: "返回应用列表" })).toBeInTheDocument();
  });

  it("iframe 提交后立即发生的加载错误不会落入被动 effect 空窗", () => {
    const authorization = launchGrant();
    expect(authorization.result.case).toBe("grant");
    if (authorization.result.case !== "grant") throw new Error("fixture grant missing");
    const onLoadError = vi.fn();

    function DispatchErrorBeforePassiveEffects() {
      useLayoutEffect(() => {
        document.querySelector("iframe")?.dispatchEvent(new Event("error"));
      }, []);
      return null;
    }

    render(<>
      <AppFrame app={fixtureApp} grant={authorization.result.value} onLoadError={onLoadError} />
      <DispatchErrorBeforePassiveEffects />
    </>);

    expect(onLoadError).toHaveBeenCalledOnce();
  });

  it("授权未返回时保留明确的应用加载态", async () => {
    let release!: (value: ReturnType<typeof launchGrant>) => void;
    const authorization = new Promise<ReturnType<typeof launchGrant>>((resolve) => { release = resolve; });
    const client = fixtureClient({ authorizeAppLaunch: async () => authorization });
    render(<PlatformShell client={client} now={() => new Date()} />);

    fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));
    expect(document.querySelector("[data-shell-state=app-authorizing]")).toBeInTheDocument();
    expect(screen.getByText("正在授权 利率研究测试应用", { selector: ".loading-panel p" })).toBeInTheDocument();

    release(launchGrant());
    expect(await screen.findByTitle("利率研究测试应用")).toBeInTheDocument();
  });

  it("会话过期会清除应用边界并要求重新认证", async () => {
    const client = fixtureClient({
      getCurrentSession: async () => currentSession(-1),
      authorizeAppLaunch: async () => launchGrant(),
    });
    render(<PlatformShell client={client} now={() => new Date()} />);

    expect(await screen.findByRole("heading", { name: "会话已过期" })).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByTitle("利率研究测试应用")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "重新认证" })).toBeInTheDocument();
    expect(screen.getByText("会话", { selector: ".boundary-rail span" })).toHaveAttribute("aria-current", "step");
  });

  it("应用运行期间到达会话绝对期限时撤销 iframe 并重新认证", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const fixedNow = new Date();
      const client = fixtureClient({ getCurrentSession: async () => currentSession(65_000) });
      render(<PlatformShell client={client} now={() => fixedNow} />);
      fireEvent.click(await screen.findByRole("button", { name: "打开利率研究测试应用" }));
      expect(await screen.findByTitle("利率研究测试应用")).toBeInTheDocument();

      act(() => { vi.advanceTimersByTime(66_000); });
      expect(screen.getByRole("heading", { name: "会话已过期" })).toBeInTheDocument();
      expect(screen.queryByTitle("利率研究测试应用")).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("会话即将过期时先刷新，再读取服务端可见应用", async () => {
    let refreshCount = 0;
    const client = fixtureClient({
      getCurrentSession: async () => currentSession(30_000),
      refreshSession: async () => {
        refreshCount += 1;
        return create(RefreshSessionResponseSchema, { result: currentSession(300_000).result });
      },
      getAppRegistry: async () => registry([]),
    });

    render(<PlatformShell client={client} now={() => new Date()} />);
    expect(await screen.findByRole("heading", { name: "当前没有可用应用" })).toBeInTheDocument();
    expect(refreshCount).toBe(1);
  });

  it("会话刷新暂时不可用时保留安全错误，而不误报会话过期", async () => {
    const refreshError = safeFailure(ErrorCode.UNAVAILABLE, "会话刷新暂时不可用", "trace-refresh-503", true);
    const client = fixtureClient({
      getCurrentSession: async () => currentSession(30_000),
      refreshSession: async () => create(RefreshSessionResponseSchema, {
        result: { case: "error", value: refreshError },
      }),
    });
    render(<PlatformShell client={client} now={() => new Date()} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("会话刷新暂时不可用");
    expect(document.querySelector("[data-shell-state=session-error]")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "会话已过期" })).not.toBeInTheDocument();
  });

  it("退出已加载应用时撤销短期启动授权并把焦点送回目录", async () => {
    let revokedAppId = "";
    const client = fixtureClient({
      revokeAppLaunch: async (appId) => {
        revokedAppId = appId;
        throw new Error("fixture only records the revocation request");
      },
    });
    render(<PlatformShell client={client} now={() => new Date()} />);
    const open = await screen.findByRole("button", { name: "打开利率研究测试应用" });
    fireEvent.click(open);
    fireEvent.click(await screen.findByRole("button", { name: "返回应用列表" }));

    await waitFor(() => expect(screen.getByRole("button", { name: "打开利率研究测试应用" })).toHaveFocus());
    expect(revokedAppId).toBe("fixture-rates-lab");
  });
});
