import { expect, test } from "@playwright/test";
import { loginByApi, openConsolePage } from "./helpers.mjs";

test("mobile dashboard screenshot regression", async ({ page }) => {
  await loginByApi(page, "admin", "admin");
  await openConsolePage(page, "/", "控制台总览");
  await expect(page).toHaveScreenshot("dashboard-mobile.png");
});
