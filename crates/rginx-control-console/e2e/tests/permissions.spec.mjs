import { expect, test } from "@playwright/test";
import { login } from "./helpers.mjs";

test("unauthenticated user is redirected to login page", async ({ page }) => {
  await page.goto("/nodes");
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByRole("heading", { name: "控制台登录" })).toBeVisible();
});

test("legacy users route is not available in single-admin mode", async ({ page }) => {
  await login(page, "admin", "admin");
  await expect(page.getByRole("link", { name: /用户|账号权限/ })).toHaveCount(0);

  await page.goto("/users");
  await expect(page.getByRole("heading", { name: "页面不存在" })).toBeVisible();
  await expect(page.getByRole("button", { name: "创建用户" })).toHaveCount(0);
});
