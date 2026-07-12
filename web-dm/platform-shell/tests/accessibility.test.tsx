import axe from "axe-core";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { PlatformShell } from "../src/app";
import styles from "../src/styles.css?raw";
import { fixtureClient } from "./fixtures/platform";

describe("Q2-WEB-04 Platform Shell 可访问性", () => {
  it("按顺序节流状态播报并抑制与当前内容重复的消息", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      render(<PlatformShell client={fixtureClient()} now={() => new Date()} />);
      const status = screen.getByRole("status");
      expect(status).toHaveTextContent("正在建立安全会话");

      await act(async () => { await Promise.resolve(); await Promise.resolve(); });
      expect(status).toHaveTextContent("正在建立安全会话");
      act(() => { vi.advanceTimersByTime(250); });
      expect(status).toHaveTextContent("应用目录加载中");
      act(() => { vi.advanceTimersByTime(250); });
      expect(status).toHaveTextContent("已读取 1 个可用应用");
      act(() => { vi.advanceTimersByTime(1_000); });
      expect(status).toHaveTextContent("已读取 1 个可用应用");
    } finally {
      vi.useRealTimers();
    }
  });

  it("状态变化使用 live region，应用有标题和明确的焦点进出路径", async () => {
    render(<PlatformShell client={fixtureClient()} now={() => new Date()} />);
    const open = await screen.findByRole("button", { name: "打开利率研究测试应用" });
    open.focus();
    fireEvent.click(open);

    expect(await screen.findByTitle("利率研究测试应用")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "进入利率研究测试应用" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "返回应用列表" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveAttribute("aria-live", "polite");
    expect(screen.getByText("应用", { selector: ".boundary-rail span" })).toHaveAttribute("aria-current", "step");
  });

  it("自动检查没有 WCAG 2 A/AA 违规", async () => {
    const { container } = render(<PlatformShell client={fixtureClient()} now={() => new Date()} />);
    await screen.findByRole("button", { name: "打开利率研究测试应用" });
    const result = await axe.run(container, {
      runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21aa"] },
      rules: { "color-contrast": { enabled: false } },
    });
    expect(result.violations).toEqual([]);
  });

  it("关键前景色与背景色满足 WCAG AA 对比度", () => {
    const ink = cssToken("ink");
    const fog = cssToken("fog");
    const copper = cssToken("copper");
    const muted = cssToken("muted");
    const paper = cssToken("paper");
    const teal = cssToken("teal");
    expect(contrastRatio(ink, fog)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(copper, fog)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(muted, fog)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(ink, paper)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(paper, teal)).toBeGreaterThanOrEqual(4.5);
  });
});

function cssToken(name: string): string {
  const match = styles.match(new RegExp(`--${name}:\\s*(#[0-9a-f]{6})`, "i"));
  if (!match) throw new Error(`missing CSS token ${name}`);
  return match[1];
}

function contrastRatio(foreground: string, background: string): number {
  const lighter = Math.max(luminance(foreground), luminance(background));
  const darker = Math.min(luminance(foreground), luminance(background));
  return (lighter + 0.05) / (darker + 0.05);
}

function luminance(hex: string): number {
  const channels = [1, 3, 5].map((offset) => Number.parseInt(hex.slice(offset, offset + 2), 16) / 255);
  return channels.map((value) => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4)
    .reduce((sum, value, index) => sum + value * [0.2126, 0.7152, 0.0722][index], 0);
}
