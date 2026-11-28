// Optional real-browser check of the admin usage page (usage-dashboard 4.2).
// Databases, users, usage rows and traffic are all synthetic and loopback only.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import net from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
assert.ok(process.env.MEIBO_BINARY && process.env.MEIBO_SQLD_BINARY, "explicit fixture binaries are required");
const binary = resolve(process.env.MEIBO_BINARY), sqld = resolve(process.env.MEIBO_SQLD_BINARY);
const output = resolve(process.env.MEIBO_BROWSER_OUTPUT || ".agent-harness/runs/usage-browser");
const scratch = await mkdtemp(join(tmpdir(), "meibo-usage-browser-"));
const password = "synthetic-usage-browser-password", token = "synthetic-usage-browser-token";
const env = { ...process.env, MEIBO_TEST_REMOTE_LOOPBACK: "1", MEIBO_USAGE_CAP_JPY: "300" };
const children = new Set();
const report = { ok: false, steps: [], shots: [], externalRequests: 0, pageErrors: 0, denied: [] };
const delay = (ms) => new Promise((done) => setTimeout(done, ms));

function child(command, args, options = {}) {
  const process = spawn(command, args, { stdio: ["pipe", "pipe", "pipe"], ...options });
  children.add(process); process.once("exit", () => children.delete(process));
  process.stderr.resume();
  return process;
}
async function port() {
  const server = net.createServer(); await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const value = server.address().port; await new Promise((done) => server.close(done)); return value;
}
async function database(name) {
  const directory = join(scratch, name); await mkdir(directory, { mode: 0o700 });
  const address = await port(), endpoint = `http://127.0.0.1:${address}`;
  const process = child(sqld, ["--db-path", join(directory, "synthetic.sqld"), "--http-listen-addr", `127.0.0.1:${address}`,
    "--grpc-listen-addr", "127.0.0.1:0", "--no-welcome", "--disable-metrics"],
  { cwd: directory, env: { RUST_LOG: "off", TOKIO_WORKER_THREADS: "2", TMPDIR: directory } });
  process.stdin.end(); process.stdout.resume();
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline && process.exitCode === null) {
    try { if ((await fetch(`${endpoint}/health`, { signal: AbortSignal.timeout(500) })).status === 200) return endpoint; } catch { /* starting */ }
    await delay(25);
  }
  throw new Error("synthetic database did not become ready");
}
async function cli(config, args, input) {
  const process = child(binary, ["hosted-remote", config, ...args], { env });
  process.stdout.resume();
  const exited = once(process, "exit"); process.stdin.end(input === undefined ? undefined : `${input}\n`);
  const [code] = await exited;
  assert.equal(code, 0, `setup command failed: ${args[0]}`);
}
async function sql(endpoint, statements) {
  const requests = statements.map(([sql, args]) => ({ type: "execute", stmt: { sql, args: args.map((value) =>
    typeof value === "number" ? { type: "integer", value: String(value) } : { type: "text", value }) } }));
  const response = await fetch(`${endpoint}/v2/pipeline`, { method: "POST", headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({ baton: null, requests: [...requests, { type: "close" }] }) });
  assert.equal(response.status, 200);
  const body = await response.json();
  assert.ok(body.results.every((result) => result.type === "ok"), "synthetic usage rows");
}
const japanDay = (offset) => new Date(Date.now() + 9 * 3600e3 - offset * 86400e3).toISOString().slice(0, 10);

try {
  await mkdir(output, { recursive: true });
  const control = await database("control"), alpha = await database("alpha");
  const config = join(scratch, "remote.json");
  env.MEIBO_DB_CONTROL_URL = control; env.MEIBO_DB_CONTROL_TOKEN = token;
  env.MEIBO_DB_ALPHA_URL = alpha; env.MEIBO_DB_ALPHA_TOKEN = token;
  await writeFile(config, JSON.stringify({ version: 1, control: { url_env: "MEIBO_DB_CONTROL_URL", token_env: "MEIBO_DB_CONTROL_TOKEN" },
    tenants: { alpha: { url_env: "MEIBO_DB_ALPHA_URL", token_env: "MEIBO_DB_ALPHA_TOKEN" } } }), { mode: 0o600 });
  await cli(config, ["init"]);
  await cli(config, ["tenant-add", "alpha", "--origin", "synthetic", "--demo-rows", "40"]);
  for (const [user, role] of [["owner", "admin"], ["editor", "editor"], ["viewer", "viewer"]]) {
    await cli(config, ["user-add", user, "--password-stdin"], password);
    await cli(config, ["member-set", user, "alpha", role]);
  }
  await cli(config, ["usage-enable"]);
  report.steps.push("setup");
  // Synthetic saved usage for the last 31 days (deterministic shapes).
  const groups = [["list", 900, 140], ["ui", 600, 60], ["detail", 300, 110], ["write", 120, 260], ["plan", 80, 120], ["login", 20, 400], ["denied", 15, 5]];
  const rows = [];
  for (let back = 30; back >= 0; back--) {
    const weekday = new Date(Date.now() + 9 * 3600e3 - back * 86400e3).getUTCDay();
    const scale = (weekday === 0 || weekday === 6 ? 0.25 : 1) * (1 + ((30 - back) % 7) / 10);
    for (const [route, requests, ms] of groups) {
      const count = Math.round(requests * scale);
      rows.push(["INSERT INTO usage_daily(day,route,requests,ms,round_trips,rows_read,rows_written) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        [japanDay(back), route, count, count * ms, count * 4, count * 35, route === "write" ? count * 6 : 0]]);
    }
  }
  await sql(control, rows);
  report.steps.push("usage-rows");

  const listen = await port(), origin = `http://127.0.0.1:${listen}`;
  const server = child(binary, ["hosted-remote", config, "serve", "--origin", origin, "--bind", `127.0.0.1:${listen}`, "--allow-http-loopback"], { env });
  server.stdin.end();
  const [first] = await once(createInterface({ input: server.stdout }), "line");
  assert.match(first, /^hosted listening: /);
  report.steps.push("serve");

  const browser = await chromium.launch({ headless: true });
  try {
    async function session(user, width) {
      const context = await browser.newContext({ viewport: { width, height: 900 } });
      await context.route("**/*", (route) => {
        if (new URL(route.request().url()).origin === origin) return route.continue();
        report.externalRequests += 1; return route.abort();
      });
      const page = await context.newPage();
      page.on("pageerror", () => { report.pageErrors += 1; });
      await page.goto(`${origin}/login`);
      await page.locator("#login-tenant").fill("alpha");
      await page.locator("#login-username").fill(user);
      await page.locator("#login-password").fill(password);
      await page.locator("#login-submit").click(); await page.waitForURL(`${origin}/`);
      return { context, page };
    }
    async function usageShot(page, name) {
      await page.goto(`${origin}/usage`);
      await page.locator("#usage-content").waitFor({ state: "visible" });
      const scroll = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      assert.ok(scroll <= 0, `no horizontal scroll in ${name}`);
      await page.screenshot({ path: join(output, `${name}.png`), fullPage: true });
      report.shots.push(`${name}.png`);
    }
    async function tabRevalidation() {
      const { context, page } = await session("owner", 390);
      const other = await context.newPage();
      await other.goto(`${origin}/login`);
      await page.bringToFront();
      await page.goto(`${origin}/usage`);
      const content = page.locator("#usage-content");
      await content.waitFor({ state: "visible" });
      async function cleared() {
        assert.ok(await content.isHidden());
        assert.ok(await page.locator("#usage-unconfigured").isHidden());
        await page.locator('[data-range="recent"]').evaluate((button) => button.click());
        assert.equal(await page.locator("#month-name").textContent(), "");
        assert.equal(await page.locator("#cap-text").textContent(), "");
        assert.equal(await page.locator("#free-list li, #group-rows tr, #day-rows tr, #day-chart li").count(), 0);
      }
      async function recheck(trigger) {
        const checked = page.waitForResponse((reply) => reply.url() === `${origin}/auth/session`);
        await trigger();
        assert.equal((await checked).status(), 200);
        await content.waitFor({ state: "visible" });
      }
      await page.evaluate(() => window.dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true })));
      await cleared();
      await recheck(() => page.evaluate(() => window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }))));
      await page.evaluate(() => {
        Object.defineProperty(document, "hidden", { configurable: true, value: true });
        document.dispatchEvent(new Event("visibilitychange"));
      });
      await cleared();
      await recheck(() => page.evaluate(() => {
        Object.defineProperty(document, "hidden", { configurable: true, value: false });
        document.dispatchEvent(new Event("visibilitychange"));
      }));
      await recheck(() => page.evaluate(() => window.dispatchEvent(new Event("focus"))));
      // Hold a real admin response after its body arrived. Deliver it after the next login.
      await page.evaluate(() => {
        const fetch = window.fetch.bind(window);
        window.fetch = async (...args) => {
          const response = await fetch(...args);
          if (args[0] === "/api/admin/usage") {
            window.fetch = fetch;
            const json = response.json.bind(response);
            response.json = async () => {
              const value = await json();
              await new Promise((resolve) => { window.releaseUsage = resolve; });
              return value;
            };
          }
          return response;
        };
      });
      await page.locator("#usage-refresh").click();
      await page.waitForFunction(() => typeof window.releaseUsage === "function");
      await cleared();
      const changed = page.waitForResponse((reply) => reply.url() === `${origin}/auth/session`);
      await other.evaluate((password) => {
        document.querySelector("#login-tenant").value = "alpha";
        document.querySelector("#login-username").value = "viewer";
        document.querySelector("#login-password").value = password;
        document.querySelector("#login-form").requestSubmit();
      }, password);
      await other.waitForURL(`${origin}/`);
      assert.equal((await (await changed).json()).role, "viewer");
      await page.getByText("この画面は管理者だけが使えます。", { exact: true }).waitFor();
      await page.evaluate(async () => {
        window.releaseUsage();
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
      await cleared();
      await page.getByText("この画面は管理者だけが使えます。", { exact: true }).waitFor();
      await context.close();
      report.steps.push("tabRevalidation");
    }

    async function fractionalYen() {
      const { context, page } = await session("owner", 390);
      await page.route("**/api/admin/usage", async (route) => {
        const response = await route.fetch();
        const data = await response.json();
        data.free = [{ key: "vcpu_seconds", label: "CPU（vCPU秒）", used: 180008, limit: 180000, ratio: 1, exceeded: true }];
        data.totals.jpy_after_free = 0.02976;
        await route.fulfill({ response, json: data });
      });
      await page.goto(`${origin}/usage`);
      await page.locator("#usage-content").waitFor({ state: "visible" });
      assert.match(await page.locator("#month-verdict").textContent(), /無料枠を超えた項目があります.*見込みは1円未満です/);
      assert.equal(await page.locator("#month-after-free").textContent(), "1円未満");
      assert.match(await page.locator("#free-list").textContent(), /無料枠を超えています/);
      await context.close();
      report.steps.push("fractionalYen");
    }
    const searchWidth = (page) => page.locator("#global-search-form").evaluate((form) => form.getBoundingClientRect().width);
    for (const width of [1280, 390]) {
      const { context, page } = await session("owner", width);
      await page.locator("#app-launcher").waitFor({ state: "visible" });
      await page.locator(".hosted-session").waitFor({ state: "visible" });
      if (width >= 640) {
        const entry = page.locator(".global-utilities > a.usage-entry");
        await entry.waitFor({ state: "visible" });
        const box = await entry.boundingBox();
        assert.ok(box && box.height >= 36, "entry is tappable");
      } else {
        assert.equal(await page.locator(".global-utilities > a.usage-entry").isVisible(), false, "no top entry below 640px");
        const withEntry = await searchWidth(page);
        await page.screenshot({ path: join(output, "home-390-after.png") });
        // The same page with the added entries removed is the pre-change layout.
        await page.evaluate(() => document.querySelectorAll("a.usage-entry, a.usage-menu-link").forEach((node) => node.remove()));
        const before = await searchWidth(page);
        await page.screenshot({ path: join(output, "home-390-before.png") });
        report.searchWidth = { before, after: withEntry };
        assert.equal(withEntry, before, "search keeps its width");
        await page.reload(); await page.locator(".hosted-session").waitFor({ state: "visible" });
      }
      await page.screenshot({ path: join(output, `home-${width}.png`) });
      await page.locator("#app-launcher").click();
      const link = page.locator("#app-menu > a.usage-menu-link");
      await link.waitFor({ state: "visible" });
      assert.equal(await page.locator("#app-menu > :last-child").getAttribute("href"), "/usage", "menu link is last");
      const linkBox = await link.boundingBox();
      assert.ok(linkBox && linkBox.height >= 44, "menu link is finger-sized");
      if (width === 390) await page.screenshot({ path: join(output, "home-390-menu.png") });
      await link.click(); await page.waitForURL(`${origin}/usage`);
      await page.locator("#usage-content").waitFor({ state: "visible" });
      assert.match(await page.locator("#month-verdict").textContent(), /無料枠の内側です。今月の請求の見込みは0円です。/);
      assert.ok((await page.locator("#group-rows > tr:not(.usage-detail-row)").count()) >= 7);
      assert.equal(await page.locator("#day-chart > li").count(), 31);
      assert.equal(await page.locator("#day-rows > tr").count(), 7);
      assert.equal(await page.locator(".usage-day-value").count() >= 1, true);
      await usageShot(page, `usage-${width}`);
      if (width === 390) {
        const toggles = page.locator("#group-rows td.toggle-col button");
        const lefts = new Set();
        for (let i = 0; i < await toggles.count(); i++) {
          const size = await toggles.nth(i).boundingBox();
          assert.ok(size && size.width >= 44 && size.height >= 44, "detail toggle is finger-sized");
          lefts.add(Math.round(size.x));
        }
        assert.equal(lefts.size, 1, "toggles share one column");
        await toggles.first().click();
        await page.locator("#group-rows .usage-detail-row:not([hidden])").first().waitFor();
        await page.locator("#day-all").click();
        assert.equal(await page.locator("#day-rows > tr").count(), 31);
        await page.screenshot({ path: join(output, "usage-390-open.png"), fullPage: true });
        report.shots.push("usage-390-open.png");
        // Free-tier rows: name and percent on one line at 390px.
        const heads = await page.locator(".usage-meter-head").evaluateAll((rows) => rows.map((row) => row.getBoundingClientRect().height));
        assert.ok(heads.every((height) => height < 30), "free-tier heads do not wrap");
      }
      await context.close();
    }
    for (const user of ["editor", "viewer"]) {
      const { context, page } = await session(user, 390);
      await page.locator(".hosted-session").waitFor({ state: "visible" });
      assert.equal(await page.locator("a.usage-entry, a.usage-menu-link").count(), 0, `${user} has no entry`);
      await page.locator("#app-launcher").click();
      await page.screenshot({ path: join(output, `home-390-${user}.png`) });
      const response = await page.goto(`${origin}/usage`);
      await page.getByRole("heading", { name: "管理者だけが開ける画面です" }).waitFor();
      const scope = (await (await page.request.get(`${origin}/auth/session`)).json()).storage_scope;
      const api = await page.request.get(`${origin}/api/admin/usage`, { headers: { "X-Meibo-Context": scope } });
      report.denied.push({ user, page: response.status(), api: api.status() });
      assert.equal(response.status(), 403); assert.equal(api.status(), 403);
      await page.screenshot({ path: join(output, `usage-denied-${user}-390.png`) });
      await context.close();
    }
    await tabRevalidation();
    await fractionalYen();
    // Synthetic heavy months: one free tier at 80% or more, then over 100%.
    const add = (requests, ms) => sql(control, [["INSERT INTO usage_daily(day,route,requests,ms,round_trips,rows_read,rows_written) VALUES(?1,'list',?2,?3,0,0,0) ON CONFLICT(day,route) DO UPDATE SET requests=requests+excluded.requests,ms=ms+excluded.ms", [japanDay(0), requests, ms]]]);
    await add(1650000, 100000000);
    for (const width of [390, 1280]) {
      const { context, page } = await session("owner", width);
      await usageShot(page, `usage-${width}-near`);
      assert.match(await page.locator("#month-verdict").textContent(), /80%を超えた項目があります/);
      await context.close();
    }
    await add(500000, 150000000);
    for (const width of [390, 1280]) {
      const { context, page } = await session("owner", width);
      await usageShot(page, `usage-${width}-over`);
      assert.match(await page.locator("#month-verdict").textContent(), /無料枠を超えた項目があります/);
      assert.doesNotMatch(await page.locator("#month-after-free").textContent(), /^0円$/);
      await context.close();
    }
  } finally { await browser.close(); }
  assert.equal(report.externalRequests, 0); assert.equal(report.pageErrors, 0);
  report.ok = true;
} finally {
  for (const process of children) process.kill("SIGKILL");
  await delay(100);
  await rm(scratch, { recursive: true, force: true });
  console.log(JSON.stringify(report));
}
