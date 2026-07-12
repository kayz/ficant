import { expect, test } from "@playwright/test";

const longDisplayName = "跨市场固定收益情景分析与压力测试研究工作台InternationalRatesScenarioResearchWorkbench";
const longCapability = "research.cross_market_scenario_analysis_with_replay_and_attribution_without_breakpoints";
const longSafeMessage = "研究任务暂时无法恢复，请核对授权范围后重试。ResearchWorkspaceTemporarilyUnavailableWithoutExposingInternalDetails";
const longTrace = "trace-long-fixture-0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

test.describe("Q2-WEB-01/03/04 Platform Shell 页面边界", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/tests/fixtures/harness.html");
    await page.waitForLoadState("networkidle");
  });

  test("键盘打开应用、iframe 标题与返回焦点路径完整", async ({ page }) => {
    const open = page.getByRole("button", { name: "打开利率研究测试应用" });
    await open.focus();
    await page.keyboard.press("Enter");

    const frame = page.getByTitle("利率研究测试应用");
    await expect(frame).toBeVisible();
    await expect(page.getByRole("button", { name: "进入利率研究测试应用" })).toBeVisible();
    await page.getByRole("button", { name: "返回应用列表" }).click();
    await expect(open).toBeFocused();
  });

  test("200% 缩放与窄视口不产生水平溢出", async ({ page }) => {
    await page.setViewportSize({ width: 640, height: 720 });
    await page.evaluate(() => { document.documentElement.style.zoom = "2"; });
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
    expect(overflow).toBe(false);
  });

  test("长中英文内容在 200% 窄视口下可读、可操作且保持完整焦点路径", async ({ page }) => {
    await page.setViewportSize({ width: 640, height: 900 });
    await page.goto("/tests/fixtures/harness.html?scenario=long");
    await page.waitForLoadState("networkidle");
    await page.evaluate(() => { document.documentElement.style.zoom = "2"; });

    const display = page.getByRole("heading", { name: longDisplayName });
    const capability = page.getByText(longCapability, { exact: false });
    await expect(display).toBeVisible();
    await expect(capability).toBeVisible();
    await expectReadableWrap(display);
    await expectReadableWrap(capability);
    await expectNoHorizontalOverflow(page);

    const open = page.getByRole("button", { name: `打开${longDisplayName}` });
    await open.focus();
    await expect(open).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.getByTitle(longDisplayName)).toBeVisible();
    await expect(page.getByRole("button", { name: `进入${longDisplayName}` })).toBeVisible();
    await expectNoHorizontalOverflow(page);
    await page.getByRole("button", { name: "返回应用列表" }).click();
    await expect(open).toBeFocused();

    await page.goto("/tests/fixtures/harness.html?scenario=long-error");
    await page.waitForLoadState("networkidle");
    await page.evaluate(() => { document.documentElement.style.zoom = "2"; });
    const safeMessage = page.getByRole("heading", { name: longSafeMessage });
    const trace = page.getByText(longTrace, { exact: true });
    await expect(safeMessage).toBeVisible();
    await expect(trace).toBeVisible();
    await expectReadableWrap(safeMessage);
    await expectReadableWrap(trace);
    await expect(page.getByRole("button", { name: "复制追踪编号" })).toBeVisible();
    await expectNoHorizontalOverflow(page);
  });

  test("reduced motion 下不会运行动画或过渡", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    const durations = await page.locator("body *").evaluateAll((elements) =>
      elements.flatMap((element) => {
        const style = getComputedStyle(element);
        return [style.animationDuration, style.transitionDuration];
      }),
    );
    expect(durations.every((duration) => duration === "0s")).toBe(true);
  });
});

async function expectNoHorizontalOverflow(page: import("@playwright/test").Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
}

async function expectReadableWrap(locator: import("@playwright/test").Locator) {
  const metrics = await locator.evaluate((element) => {
    const style = getComputedStyle(element);
    const range = document.createRange();
    range.selectNodeContents(element);
    const lineTops = new Set([...range.getClientRects()].map((rect) => Math.round(rect.top)));
    return {
      fitsOwnBox: element.scrollWidth <= element.clientWidth,
      wrapped: lineTops.size > 1,
      whiteSpace: style.whiteSpace,
    };
  });
  expect(metrics.fitsOwnBox).toBe(true);
  expect(metrics.whiteSpace).not.toBe("nowrap");
  expect(metrics.wrapped).toBe(true);
}
