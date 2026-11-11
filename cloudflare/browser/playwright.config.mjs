import { defineConfig } from '@playwright/test';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const port = process.env.MEIBO_TEST_PORT || '4331';
if (!/^\d+$/.test(port) || Number(port) < 1024 || Number(port) > 65535) {
  throw new Error('MEIBO_TEST_PORT must be a port from 1024 to 65535');
}
const baseURL = `http://127.0.0.1:${port}`;
const outputDir = process.env.MEIBO_TEST_OUTPUT || join(tmpdir(), 'meibo-browser-results');

export default defineConfig({
  testDir: fileURLToPath(new URL('.', import.meta.url)),
  testMatch: '*.spec.mjs',
  workers: 1,
  fullyParallel: false,
  forbidOnly: true,
  retries: 0,
  timeout: 30_000,
  globalTimeout: 180_000,
  reporter: [['list'], ['json', { outputFile: join(outputDir, 'report.json') }]],
  outputDir,
  use: {
    baseURL,
    browserName: 'chromium',
    headless: true,
    serviceWorkers: 'block',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [390, 768, 1440].map((width) => ({
    name: `chromium-${width}`,
    use: { viewport: { width, height: 900 } },
  })),
  webServer: {
    command: 'node browser/server.mjs',
    cwd: fileURLToPath(new URL('..', import.meta.url)),
    url: `${baseURL}/health`,
    reuseExistingServer: false,
    gracefulShutdown: { signal: 'SIGTERM', timeout: 5000 },
    timeout: 30_000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
