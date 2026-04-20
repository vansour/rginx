import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, devices } from "@playwright/test";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "../../..");

export default defineConfig({
  testDir: path.join(__dirname, "tests"),
  timeout: 90_000,
  expect: {
    timeout: 15_000,
    toHaveScreenshot: {
      animations: "disabled",
      caret: "hide",
      scale: "device",
    },
  },
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 2 : 0,
  reporter: [["list"], ["html", { open: "never", outputFolder: "playwright-report" }]],
  use: {
    baseURL: process.env.RGINX_CONTROL_E2E_BASE_URL || "http://127.0.0.1:18180",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
    locale: "zh-CN",
  },
  snapshotPathTemplate:
    "{testDir}/__snapshots__/{testFilePath}/{projectName}/{arg}{ext}",
  webServer: {
    command: path.join(repoRoot, "scripts/run-control-console-e2e-env.sh"),
    cwd: repoRoot,
    url: `${process.env.RGINX_CONTROL_E2E_BASE_URL || "http://127.0.0.1:18180"}/healthz`,
    timeout: 300_000,
    reuseExistingServer: !process.env.CI,
    stdout: "pipe",
    stderr: "pipe",
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
      },
      testIgnore: /mobile-visual\.spec\.mjs/,
    },
    {
      name: "mobile-chromium",
      use: {
        ...devices["Pixel 7"],
      },
      testMatch: /mobile-visual\.spec\.mjs/,
    },
  ],
});
