import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { validatePublicOrigin } from "../src/boundary.ts";
import { privateDirectory } from "./artifacts.mjs";
import { readPrivateJson } from "./preflight-input.mjs";

/** The same synthetic UI cases run against local and public HTTPS. Lifecycle
 * adapters belong to the authenticated operator, never a public HTTP endpoint. */
export async function verifyService({ origin, credentials, output, profile, lifecycle = {} }) {
  assert.equal(validatePublicOrigin(origin).origin, origin);
  const target = new URL(origin);
  assert.ok((profile === "public" && /^meibo-beta\.[a-z0-9-]+\.workers\.dev$/.test(target.hostname) && !target.port)
    || (profile === "local" && target.hostname === "localhost"));
  assert.equal(credentials.scope, "synthetic fixture only");
  assert.ok(typeof credentials.password === "string" && credentials.password.length >= 12);
  await privateDirectory(output);
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ ignoreHTTPSErrors: profile === "local", serviceWorkers: "block", viewport: { width: 1440, height: 1050 } });
  const report = { ok: false, phase: "health", steps: [], lifecycle: [], widths: [], externalRequests: 0, assetFailures: 0, badRequests: 0, pageErrors: 0, browser: browser.version() };
  const mark = (name) => { report.phase = name; report.steps.push(name); console.log(`verify: ${name}`); };
  await context.route("**/*", (route) => {
    if (new URL(route.request().url()).origin !== origin) { report.externalRequests++; return route.abort(); }
    return route.continue();
  });
  context.on("page", (page) => {
    page.on("pageerror", () => report.pageErrors++);
    page.on("response", (response) => {
      if (response.status() === 400) report.badRequests++;
      if (/\.(?:css|js|png)$/.test(new URL(response.url()).pathname) && response.status() >= 400) report.assetFailures++;
    });
  });
  const page = await context.newPage(); page.setDefaultTimeout(90000); page.setDefaultNavigationTimeout(90000);
  async function login(role, tenant) {
    await page.goto(`${origin}/login`);
    await page.locator("#login-tenant").fill(tenant); await page.locator("#login-username").fill(role);
    await page.locator("#login-password").fill(credentials.password); await page.locator("#login-submit").click();
    await page.waitForURL((url) => url.origin === origin && url.pathname === "/", { waitUntil: "commit" });
    await page.locator("#company-rows .company-name").first().waitFor();
  }
  async function logout() {
    await page.getByRole("button", { name: "ログアウト", exact: true }).click();
    await page.waitForURL((url) => url.origin === origin && url.pathname === "/login");
  }
  async function api(path, method = "GET", body) {
    return page.evaluate(async ({ path, method, body }) => {
      const identity = await (await fetch("/auth/session")).json();
      const response = await fetch(path, { method, headers: { "Content-Type": "application/json", "X-Meibo-Local": "1", "X-Meibo-Context": identity.storage_scope },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
      return { status: response.status, body: await response.json() };
    }, { path, method, body });
  }
  async function resume(marker, scope) {
    await page.goto(origin);
    await page.locator("#company-rows .company-name").filter({ hasText: marker }).waitFor();
    assert.equal((await api("/auth/session")).body.storage_scope, scope);
  }
  try {
    assert.equal((await page.request.get(`${origin}/health`, { timeout: 90000 })).status(), 200);
    assert.equal((await page.request.get(`${origin}/api/summary`)).status(), 401);
    const loginPage = await page.goto(`${origin}/login`);
    assert.equal(loginPage.status(), 200); assert.ok(loginPage.headers()["content-security-policy"]);
    assert.ok((await page.locator('link[rel="canonical"]').getAttribute("href")).startsWith(origin)); mark("public-boundary-and-metadata");
    await login("admin", "alpha"); mark("admin-login");
    const marker = `合成alpha確認-${randomUUID().slice(0, 8)}`, note = `${marker}の保存確認`, betaNote = `合成beta確認-${randomUUID().slice(0, 8)}`;
    await page.locator("#company-rows .company-name").first().click();
    await page.getByRole("button", { name: "取引先を編集", exact: true }).click();
    await page.locator("#company-name").fill(marker); await page.locator("#company-submit").click();
    await page.waitForFunction((name) => document.querySelector("#selected-company-title")?.textContent === name, marker);
    await page.locator("#activity-note").fill(note); await page.locator("#activity-form button[type=submit]").click();
    await page.locator(".timeline-note").filter({ hasText: note }).first().waitFor(); mark("admin-ui-write");
    for (const width of [390, 768, 1440]) {
      await page.setViewportSize({ width, height: 1050 });
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await page.screenshot({ path: join(output, `record-${width}.png`), fullPage: true }); report.widths.push(width);
    }
    assert.equal((await page.request.get(`${origin}/api/companies`, { headers: { "X-Meibo-Context": "wrong-context" } })).status(), 409);
    assert.equal((await page.request.post(`${origin}/api/companies/1/activities`, { headers: { Origin: "https://foreign.example", "X-Meibo-Local": "1" }, data: { note: "synthetic rejection" } })).status(), 403); mark("csrf-and-context-rejections");
    const old = await context.newPage(); old.setDefaultTimeout(90000); await old.goto(origin);
    await old.locator("#company-rows .company-name").first().waitFor();
    let release; const held = new Promise((done) => { release = done; });
    await old.route("**/auth/session", async (route) => { await held; await route.continue(); });
    await old.evaluate(() => { dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true })); dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true })); });
    await logout(); await login("editor", "beta");
    assert.ok(!(await page.locator("body").innerText()).includes(marker));
    assert.ok(await old.evaluate(() => document.body.hidden && getComputedStyle(document.body).display === "none"));
    release(); await old.waitForURL("**/login", { timeout: 90000 }); await old.close(); mark("old-tab-and-tenant-isolation");
    await page.locator("#company-rows .company-name").first().click();
    await page.locator("#activity-note").fill(betaNote); await page.locator("#activity-form button[type=submit]").click();
    await page.locator(".timeline-note").filter({ hasText: betaNote }).first().waitFor();
    assert.equal((await api("/api/companies/1/contacts/1/erase", "POST", { wipe_notes: false })).status, 403); mark("editor-write-and-admin-restriction");
    await logout(); await login("viewer", "alpha"); await page.locator("#company-rows .company-name").first().click();
    await page.locator("#activity-note").fill("拒否する合成viewer記録"); await page.locator("#activity-form button[type=submit]").click();
    await page.locator("#activity-error:not([hidden])").waitFor();
    assert.equal(await page.locator("#activity-note").inputValue(), "拒否する合成viewer記録");
    assert.equal((await api("/auth/audit")).status, 403); mark("viewer-rejection-keeps-input");
    await logout(); assert.equal((await page.request.get(`${origin}/api/summary`)).status(), 401); mark("logout-revokes-session");
    await login("admin", "alpha"); const originalScope = (await api("/auth/session")).body.storage_scope;
    for (const name of ["restart", "idle", "recreate"]) if (lifecycle[name]) {
      await page.goto("about:blank"); await lifecycle[name](); await resume(marker, originalScope);
      report.lifecycle.push(name); mark(`${name}-keeps-session-and-data`);
    }
    if (lifecycle.faultAndRecover) {
      await page.goto("about:blank");
      await lifecycle.faultAndRecover({
        probeBlocked: async () => assert.equal((await page.request.get(`${origin}/login`, { timeout: 45000 })).status(), 503),
      });
      await resume(marker, originalScope); report.lifecycle.push("faultAndRecover"); mark("gate-persists-and-explicit-recovery");
    }
    if (lifecycle.restore) {
      await page.goto("about:blank"); await lifecycle.restore();
      assert.equal((await page.request.get(`${origin}/api/summary`)).status(), 401);
      for (const role of ["admin", "editor", "viewer"]) for (const tenant of ["alpha", "beta"]) {
        await login(role, tenant); const identity = (await api("/auth/session")).body;
        assert.equal(identity.tenant, tenant); assert.equal(identity.role, role);
        if (role === "admin" && tenant === "alpha") {
          assert.notEqual(identity.storage_scope, originalScope);
          await page.locator("#company-rows .company-name").filter({ hasText: marker }).click();
          await page.locator(".timeline-note").filter({ hasText: note }).first().waitFor();
        }
        await logout();
      }
      report.lifecycle.push("restore"); mark("restored-two-tenants-three-roles");
    } else await logout();
    assert.equal(report.externalRequests, 0); assert.equal(report.assetFailures, 0); assert.equal(report.badRequests, 0); assert.equal(report.pageErrors, 0); report.ok = true;
  } catch (error) {
    report.error = { kind: ["Error", "AssertionError", "TimeoutError"].includes(error?.name) ? error.name : "Error",
      location: error?.stack?.match(/verify\.mjs:\d+:\d+/)?.[0] || "unavailable" };
  } finally {
    await browser.close(); await writeFile(join(output, "browser.json"), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  }
  return report;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const args = process.argv.slice(2), names = ["--origin", "--users", "--out", "--profile"];
    assert.equal(args.length, 8); assert.ok(names.every((name, i) => args[i * 2] === name));
    const result = await verifyService({ origin: args[1], credentials: await readPrivateJson(args[3]), output: resolve(args[5]), profile: args[7] });
    console.log(JSON.stringify({ ok: result.ok, steps: result.steps.length, lifecycle: result.lifecycle }));
    if (!result.ok) process.exitCode = 1;
  } catch { console.error("検証用の設定を確認してください。"); process.exitCode = 1; }
}
