import { expect, test } from "@playwright/test";
import { login, openConsolePage } from "./helpers.mjs";

test("admin can navigate core console pages", async ({ page }) => {
  await login(page, "admin", "admin");
  await openConsolePage(page, "/", "控制台总览", "admin");
  await expect(page.getByRole("link", { name: /用户|账号权限/ })).toHaveCount(0);
  await expect(page.getByText("节点", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("告警", { exact: true })).toHaveCount(0);
  await expect(page.getByText("审计", { exact: true })).toHaveCount(0);
  await expect(page.getByText("配置与发布", { exact: true })).toHaveCount(0);

  await openConsolePage(page, "/nodes", "节点概览", "admin");
  await expect(page.locator("main").getByRole("heading", { name: "节点列表" })).toBeVisible();
});
