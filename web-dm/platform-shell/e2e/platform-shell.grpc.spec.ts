import { expect, test } from "@playwright/test";

test("Q2-WEB-02 通过生成 client 调用真实 Rust Registry/session", async ({ page }) => {
  const currentSession = page.waitForRequest((request) => request.url().includes("/ficant.app.v1.PlatformService/GetCurrentSession"));
  const appRegistry = page.waitForRequest((request) => request.url().includes("/ficant.app.v1.PlatformService/GetAppRegistry"));
  await page.goto("/");
  await page.waitForLoadState("networkidle");

  const [sessionRequest, registryRequest] = await Promise.all([currentSession, appRegistry]);
  expect(sessionRequest.method()).toBe("POST");
  expect(registryRequest.method()).toBe("POST");

  await expect(page.getByRole("heading", { name: "应用目录" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "当前没有可用应用" })).toBeVisible();
  await expect(page.locator("[data-transport=grpc-web]")).toHaveAttribute("data-contract", "ficant.app.v1.PlatformService");
});
