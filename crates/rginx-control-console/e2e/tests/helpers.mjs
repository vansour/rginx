import { expect } from "@playwright/test";

const AUTH_TOKEN_STORAGE_KEY = "rginx-control-plane-token";

async function waitForStoredToken(page) {
  await expect
    .poll(
      () => page.evaluate((storageKey) => window.localStorage.getItem(storageKey), AUTH_TOKEN_STORAGE_KEY),
      { message: "expected login token to be persisted in localStorage" },
    )
    .not.toBeNull();
}

async function waitForAuthenticatedShell(page, username) {
  await expect(page.getByRole("heading", { name: "控制台登录" })).toHaveCount(0);

  const userCard = page.locator(".app-user-card");
  if ((await userCard.count()) > 0 && (await userCard.first().isVisible())) {
    await expect(userCard).toContainText(username);
    return;
  }

  await expect(page.locator("main .hero-meta")).toContainText(username);
}

export async function login(page, username, password) {
  await page.goto("/login");
  await expect(page.getByRole("heading", { name: "控制台登录" })).toBeVisible();
  await page.getByLabel("用户名").fill(username);
  await page.getByLabel("密码").fill(password);
  const loginResponsePromise = page.waitForResponse(
    (response) =>
      response.url().includes("/api/v1/auth/login") &&
      response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "登录控制台" }).click();
  const loginResponse = await loginResponsePromise;
  expect(loginResponse.ok()).toBeTruthy();
  await waitForStoredToken(page);
  await waitForAuthenticatedShell(page, username);
  await expect(page.locator(".app-topbar").getByRole("heading", { name: "控制台总览" })).toBeVisible();
}

export async function loginByApi(page, username, password) {
  const loginResponse = await page.request.post("/api/v1/auth/login", {
    data: { username, password },
  });
  expect(loginResponse.ok()).toBeTruthy();

  const body = await loginResponse.json();
  await page.addInitScript(
    ([storageKey, token]) => window.localStorage.setItem(storageKey, JSON.stringify(token)),
    [AUTH_TOKEN_STORAGE_KEY, body.token],
  );
  const sessionResponsePromise = page.waitForResponse(
    (response) =>
      response.url().includes("/api/v1/auth/me") &&
      response.request().method() === "GET",
  );
  await page.goto("/");
  const sessionResponse = await sessionResponsePromise;
  expect(sessionResponse.ok()).toBeTruthy();
  await waitForStoredToken(page);
  await waitForAuthenticatedShell(page, username);
}

export async function openConsolePage(page, path, heading, username = "admin") {
  const currentUrl = new URL(page.url());
  const targetUrl = new URL(path, currentUrl);
  const sessionResponsePromise = page.waitForResponse(
    (response) =>
      response.url().includes("/api/v1/auth/me") &&
      response.request().method() === "GET",
  );
  if (
    targetUrl.pathname === currentUrl.pathname &&
    targetUrl.search === currentUrl.search &&
    targetUrl.hash === currentUrl.hash
  ) {
    await page.reload();
  } else {
    await page.goto(path);
  }
  const sessionResponse = await sessionResponsePromise;
  expect(sessionResponse.ok()).toBeTruthy();
  await expect(page.locator(".app-topbar").getByRole("heading", { name: heading })).toBeVisible();
  await waitForStoredToken(page);
  await waitForAuthenticatedShell(page, username);
}
