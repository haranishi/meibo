// Optional actual-browser checks; no production dependency or external service is used.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, readFile, mkdir, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import https from "node:https";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
assert.ok(process.env.MEIBO_BINARY, "MEIBO_BINARY must name the built local fixture binary");
const binary = resolve(process.env.MEIBO_BINARY);
const output = resolve(process.env.MEIBO_BROWSER_OUTPUT || ".agent-harness/runs/public-entry-browser");
const scratch = await mkdtemp(join(tmpdir(), "meibo-public-entry-browser-"));
const root = join(scratch, "synthetic-service");
const password = "synthetic-browser-fixture-password";
const report = { browser: "", widths: [], checks: [], externalRequests: 0, pageErrors: 0,
  limits: "Native share API branches are mocked; no physical-device sheet or live SNS preview/post is tested." };
let backend, browser, front;

function cli(args, input) {
  const result = spawnSync(binary, ["hosted", root, ...args], { input, encoding: "utf8" });
  assert.equal(result.status, 0, "synthetic fixture setup failed");
}

async function startBackend(origin) {
  backend = spawn(binary, ["hosted", root, "serve", "--origin", origin, "--bind", "127.0.0.1:0"],
    { stdio: ["ignore", "pipe", "ignore"] });
  const reader = createInterface({ input: backend.stdout });
  const address = await new Promise((resolveAddress, reject) => {
    const timeout = setTimeout(() => reject(new Error("synthetic fixture startup timed out")), 15000);
    reader.once("line", (line) => {
      clearTimeout(timeout);
      resolveAddress(line.startsWith("hosted listening: ") ? line.slice(18).split(" ")[0] : "");
    });
    backend.once("exit", () => { clearTimeout(timeout); reject(new Error("synthetic fixture stopped")); });
  });
  assert.match(address, /^127\.0\.0\.1:\d+$/);
  return Number(address.split(":")[1]);
}

try {
  await mkdir(output, { recursive: true });
  cli(["init"]);
  cli(["tenant-add", "alpha", "--origin", "synthetic", "--demo-rows", "3"]);
  cli(["user-add", "browser-fixture", "--password-stdin"], password);
  cli(["member-set", "browser-fixture", "alpha", "admin"]);
  const cert = join(scratch, "local-cert.pem"), key = join(scratch, "local-key.pem");
  const certificate = spawnSync(process.env.OPENSSL_BINARY || "openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes",
    "-keyout", key, "-out", cert, "-days", "1", "-subj", "/CN=localhost"], { stdio: "ignore" });
  assert.equal(certificate.status, 0, "local fixture certificate creation failed");
  let backendPort;
  front = https.createServer({ key: await readFile(key), cert: await readFile(cert) }, (request, response) => {
    const upstream = http.request({ host: "127.0.0.1", port: backendPort, method: request.method,
      path: request.url, headers: request.headers }, (incoming) => {
      response.writeHead(incoming.statusCode, incoming.headers);
      incoming.pipe(response);
    });
    upstream.on("error", () => { response.writeHead(502); response.end(); });
    request.pipe(upstream);
  });
  await new Promise((ready) => front.listen(0, "127.0.0.1", ready));
  const origin = `https://127.0.0.1:${front.address().port}`, canonical = `${origin}/login`;
  backendPort = await startBackend(origin);
  browser = await chromium.launch({ headless: true });
  report.browser = browser.version();

  async function context(mode = "unsupported", width = 390, touch = false) {
    // This untrusted certificate exception is limited to our generated loopback fixture.
    const current = await browser.newContext({ ignoreHTTPSErrors: true, hasTouch: touch, isMobile: touch,
      viewport: { width, height: touch ? 844 : width === 768 ? 1024 : 1000 } });
    await current.route("**/*", (route) => {
      if (new URL(route.request().url()).origin !== origin) { report.externalRequests++; return route.abort(); }
      return route.continue();
    });
    await current.addInitScript((shareMode) => {
      window.testShareCalls = [];
      Object.defineProperty(navigator, "share", { configurable: true, value: shareMode === "unsupported" ? undefined : (payload) => {
        window.testShareCalls.push({ payload, active: navigator.userActivation.isActive });
        if (shareMode === "cancel") return Promise.reject(new DOMException("cancelled", "AbortError"));
        if (shareMode === "failure") return Promise.reject(new DOMException("denied", "NotAllowedError"));
        return Promise.resolve();
      } });
      Object.defineProperty(navigator, "canShare", { configurable: true, value: () => shareMode !== "cannot-share" });
    }, mode);
    const page = await current.newPage();
    page.on("pageerror", () => report.pageErrors++);
    return { current, page };
  }

  async function load(page) {
    await page.goto(`${canonical}?tenant=private-org&username=private-user&session=private-session`, { waitUntil: "networkidle" });
    await page.locator("#public-share-copy:enabled").waitFor();
    assert.equal(await page.locator('link[rel="canonical"]').getAttribute("href"), canonical);
    assert.equal(await page.locator("#public-share-status").getAttribute("aria-live"), "polite");
    assert.equal(await page.evaluate(() => window.testShareCalls.length), 0);
  }

  for (const width of [390, 768, 1440]) {
    const { current, page } = await context("unsupported", width);
    await current.grantPermissions(["clipboard-read", "clipboard-write"], { origin });
    await load(page);
    assert.equal(await page.locator("#public-share-native").isVisible(), false);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "horizontal overflow");
    for (const id of ["login-submit", "public-share-x", "public-share-line", "public-share-copy"]) {
      const box = await page.locator(`#${id}`).boundingBox();
      assert.ok(box.width >= 44 && box.height >= 44 && box.x >= 0 && box.x + box.width <= width, "touch target or clipping");
    }
    for (const [id, targetOrigin, path] of [["public-share-x", "https://twitter.com", "/intent/tweet"],
      ["public-share-line", "https://social-plugins.line.me", "/lineit/share"]]) {
      const link = page.locator(`#${id}`), href = new URL(await link.getAttribute("href"));
      assert.equal(href.origin, targetOrigin); assert.equal(href.pathname, path);
      assert.equal(href.searchParams.get("url"), canonical);
      assert.equal(href.searchParams.get("text"), "meibo · 営業ワークスペース");
      assert.equal(href.searchParams.size, 2);
      assert.equal(await link.getAttribute("target"), "_blank");
      assert.equal(await link.getAttribute("rel"), "noopener noreferrer");
    }
    await page.locator(".login-brand").focus();
    for (const id of ["login-tenant", "login-username", "login-password", "login-submit", "public-share-x", "public-share-line", "public-share-copy"]) {
      await page.keyboard.press("Tab");
      assert.equal(await page.evaluate(() => document.activeElement.id), id);
      assert.ok(await page.evaluate(() => getComputedStyle(document.activeElement).outlineStyle !== "none"), "visible keyboard focus");
    }
    await page.screenshot({ path: join(output, `login-${width}.png`), fullPage: true });
    await page.locator("#login-tenant").fill("private-org");
    await page.keyboard.press("Tab");
    await page.locator("#public-share-copy").click();
    await page.waitForFunction(() => document.getElementById("public-share-status").textContent.length > 0);
    assert.equal(await page.locator("#public-share-status").textContent(), "ログインURLをコピーしました。");
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), canonical);
    assert.equal(await page.evaluate(() => document.activeElement.id), "public-share-copy");
    if (width === 390) {
      const decoded = await page.evaluate(async () => {
        const response = await fetch("/public-entry.png?tenant=private-org");
        const image = await createImageBitmap(await response.blob());
        const result = { mime: response.headers.get("Content-Type"), width: image.width, height: image.height };
        image.close(); return result;
      });
      assert.deepEqual(decoded, { mime: "image/png", width: 1200, height: 630 });
      report.checks.push("PNG actually decoded: image/png 1200x630");
      await page.locator("#login-tenant").fill("alpha");
      await page.locator("#login-username").fill("browser-fixture");
      await page.locator("#login-password").fill(password);
      await page.locator("#login-submit").click();
      await page.waitForURL(`${origin}/`);
      await page.waitForFunction(() => !document.documentElement.hasAttribute("data-meibo-auth-pending"));
      assert.equal(await page.locator("#public-entry-share").count(), 0);
      assert.equal(await page.evaluate(async () => (await (await fetch("/auth/session")).json()).tenant), "alpha");
      report.checks.push("real synthetic login and business UI still work");
    }
    report.widths.push({ width, keyboard: true, realClipboard: true, noOverflow: true });
    await current.close();
  }

  for (const mode of ["success", "cancel", "failure", "cannot-share"]) {
    const { current, page } = await context(mode);
    await load(page);
    if (mode === "cannot-share") {
      assert.equal(await page.locator("#public-share-native").isVisible(), false);
    } else {
      if (mode === "success") await page.screenshot({ path: join(output, "login-native-390.png"), fullPage: true });
      await page.locator("#login-username").fill("private-user");
      await page.locator("#public-share-native").click();
      await page.waitForFunction(() => document.getElementById("public-share-status").textContent.length > 0);
      const calls = await page.evaluate(() => window.testShareCalls);
      assert.equal(calls.length, 1); assert.equal(calls[0].active, true);
      assert.deepEqual(calls[0].payload, { title: "meibo · 営業ワークスペース", text: "企業情報と営業活動を、チームのワークスペースで。", url: canonical });
      const status = await page.locator("#public-share-status").textContent();
      assert.match(status, mode === "success" ? /共有操作を終了/ : mode === "cancel" ? /キャンセル/ : /共有できません/);
      assert.ok(!/投稿完了|送信完了/.test(status));
      assert.equal(await page.locator("#public-share-native").isEnabled(), true);
    }
    report.checks.push(`native share mocked: ${mode}`);
    await current.close();
  }

  for (const mode of ["denied", "legacy", "manual"]) {
    const { current, page } = await context();
    await current.grantPermissions(["clipboard-read", "clipboard-write"], { origin });
    await current.addInitScript((copyMode) => {
      window.testReadClipboard = navigator.clipboard.readText.bind(navigator.clipboard);
      if (copyMode === "denied") Object.defineProperty(navigator, "clipboard", { value: { writeText: () => Promise.reject(new DOMException("denied", "NotAllowedError")) } });
      else Object.defineProperty(navigator, "clipboard", { value: undefined });
      if (copyMode === "manual") document.execCommand = () => false;
    }, mode);
    await load(page);
    await page.locator("#public-share-copy").click();
    await page.waitForFunction(() => document.getElementById("public-share-status").textContent.length > 0);
    if (mode === "legacy") {
      assert.equal(await page.evaluate(() => window.testReadClipboard()), canonical);
      assert.match(await page.locator("#public-share-status").textContent(), /コピーしました/);
    } else {
      assert.match(await page.locator("#public-share-status").textContent(), /コピーできませんでした/);
      assert.equal(await page.locator("#public-share-fallback").isVisible(), true);
      assert.equal(await page.locator("#public-share-url").inputValue(), canonical);
      assert.equal(await page.evaluate(() => document.activeElement.id), "public-share-url");
      assert.ok(await page.evaluate(() => { const field = document.getElementById("public-share-url"); return field.selectionStart === 0 && field.selectionEnd === field.value.length; }));
      if (mode === "denied") await page.screenshot({ path: join(output, "login-copy-failure-390.png"), fullPage: true });
    }
    assert.equal(await page.locator("#public-share-copy").isEnabled(), true);
    report.checks.push(`copy fallback: ${mode}`);
    await current.close();
  }
  {
    const { current, page } = await context("success", 390, true);
    await current.grantPermissions(["clipboard-read", "clipboard-write"], { origin });
    await load(page);
    await page.locator("#public-share-copy").tap();
    await page.waitForFunction(() => document.getElementById("public-share-status").textContent.length > 0);
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), canonical);
    await page.locator("#public-share-native").tap();
    await page.waitForFunction(() => window.testShareCalls.length === 1);
    assert.equal(await page.evaluate(() => window.testShareCalls[0].active), true);
    await page.screenshot({ path: join(output, "login-touch-390.png"), fullPage: true });
    report.checks.push("emulated 390x844 touch: real copy and mocked native share callback");
    await current.close();
  }
  assert.equal(report.externalRequests, 0, "unexpected external communication");
  assert.equal(report.pageErrors, 0, "browser JavaScript failure");
  await writeFile(join(output, "browser.json"), JSON.stringify(report, null, 2) + "\n");
  console.log(JSON.stringify(report));
} finally {
  if (browser) await browser.close();
  if (backend && backend.exitCode === null) { backend.kill(); await new Promise((done) => backend.once("exit", done)); }
  if (front) await new Promise((done) => front.close(done));
  await rm(scratch, { recursive: true, force: true });
}
