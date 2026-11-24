import { defineConfig } from "@playwright/test";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const port = Number(process.env.MEIBO_DEMO_TEST_PORT || 4441);
const width = Number(process.env.MEIBO_DEMO_WIDTH || 1440);
if (![390, 768, 1440].includes(width)) throw new Error("MEIBO_DEMO_WIDTH must be 390, 768 or 1440");
// MEIBO_DEMO_URL を指定すると、サーバーを起動せず、その公開URL（配備後のデモなど）に同じ試験を向ける。
const remoteURL = process.env.MEIBO_DEMO_URL || "";
const baseURL = remoteURL || `https://127.0.0.1:${port}`;
const outputDir = process.env.MEIBO_DEMO_TEST_OUTPUT || join(tmpdir(), `meibo-demo-browser-results-${width}`);

// 状態を持つデモ（全員が1組織を共有する）なので、幅ごとに新しいサーバーで1周する。
// 周ごとに `MEIBO_DEMO_WIDTH` を変えて呼ぶ（`npm test` が390・768・1440を順に回す）。
export default defineConfig({
  testDir: fileURLToPath(new URL(".", import.meta.url)),
  testMatch: "*.spec.mjs",
  workers: 1,
  fullyParallel: false,
  forbidOnly: true,
  retries: 0,
  timeout: 45_000,
  globalTimeout: 240_000,
  reporter: [["list"], ["json", { outputFile: join(outputDir, "report.json") }]],
  outputDir,
  use: {
    baseURL,
    browserName: "chromium",
    headless: true,
    ignoreHTTPSErrors: true,
    serviceWorkers: "block",
    timezoneId: "Asia/Tokyo",
    locale: "ja-JP",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    viewport: { width, height: 900 },
  },
  projects: [{ name: `chromium-${width}` }],
  webServer: remoteURL ? undefined : {
    command: "node server.mjs",
    cwd: fileURLToPath(new URL(".", import.meta.url)),
    url: `${baseURL}/health`,
    ignoreHTTPSErrors: true,
    reuseExistingServer: false,
    timeout: 60_000,
    stdout: "pipe",
    stderr: "pipe",
  },
});
