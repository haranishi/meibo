// Local synthetic authority for the real Rust HTTP and shared browser suite.
// The Rust fixture is compiled only into its test executable, never the app.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { mkdir, readFile, writeFile, chmod } from "node:fs/promises";
import { resolve, join } from "node:path";
import https from "node:https";
import http from "node:http";
import { verifyService } from "../../../cloudflare/scripts/verify.mjs";

const binary = process.env.MEIBO_ACCESS_QA_TEST_BINARY;
const mode = process.env.MEIBO_ACCESS_QA_MODE || "access";
assert.ok(["access", "password"].includes(mode));
assert.ok(binary && resolve(binary) === binary, "explicit Rust test executable required");
const output = resolve(process.env.MEIBO_ACCESS_QA_OUTPUT || ".agent-harness/runs/cloudflare-access-browser");
await mkdir(output, { mode: 0o700 });
async function command(args) {
  const child = spawn("openssl", args, { stdio: ["ignore", "ignore", "ignore"] });
  assert.equal((await once(child, "close"))[0], 0, "local TLS fixture setup failed");
}
await command(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=localhost",
  "-addext", "subjectAltName=DNS:localhost", "-keyout", join(output, "local.key"), "-out", join(output, "local.crt")]);
await chmod(join(output, "local.key"), 0o600);
let port, tokens, runtime, diagnostic = "";
const front = https.createServer({ key: await readFile(join(output, "local.key")), cert: await readFile(join(output, "local.crt")) }, async (req, res) => {
  try {
    const role = /(?:^|;\s*)meibo-fixture-identity=([a-z]+)/.exec(req.headers.cookie || "")?.[1];
    const headers = { ...req.headers, connection: "close" };
    delete headers["cf-access-jwt-assertion"]; delete headers["transfer-encoding"];
    if (tokens?.[role]) headers["cf-access-jwt-assertion"] = tokens[role];
    const chunks = []; let size = 0;
    for await (const chunk of req) { size += chunk.length; assert.ok(size <= 1024 * 1024); chunks.push(chunk); }
    const body = Buffer.concat(chunks);
    if (req.method !== "GET") headers["content-length"] = String(body.length);
    const upstream = http.request({ hostname: "127.0.0.1", port, method: req.method, path: req.url, headers, agent: false }, incoming => {
      res.writeHead(incoming.statusCode, incoming.headers); incoming.pipe(res);
    });
    upstream.on("error", () => { if (!res.headersSent) res.writeHead(502); res.end(); }); upstream.end(body);
  } catch { if (!res.headersSent) res.writeHead(502); res.end(); }
});
await new Promise(done => front.listen(0, "127.0.0.1", done));
const origin = `https://localhost:${front.address().port}`;
const report = { ok: false, mode, sharedCases: 0, loginWidths: [], actualCloudflareLogin: "not exercised; synthetic authority only" };
try {
  runtime = spawn(binary, ["hosted::http::access_tests::access_browser_fixture", "--exact", "--ignored", "--nocapture"], {
    env: { ...process.env, MEIBO_ACCESS_QA_MODE: mode, MEIBO_ACCESS_QA_ORIGIN: origin, MEIBO_ACCESS_QA_TOKENS: join(output, "tokens.private.json") }, stdio: ["ignore", "pipe", "pipe"],
  });
  runtime.stderr.on("data", b => { diagnostic += b.toString(); assert.ok(diagnostic.length < 65536); });
  const lines = createInterface({ input: runtime.stdout });
  port = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("fixture startup deadline")), 30000);
    lines.on("line", line => { const match = /^hosted listening: 127\.0\.0\.1:(\d+)/.exec(line); if (match) { clearTimeout(timer); resolve(Number(match[1])); } });
    runtime.once("exit", () => { clearTimeout(timer); reject(new Error("fixture exited")); });
  });
  tokens = JSON.parse(await readFile(join(output, "tokens.private.json")));
  const loginIdentity = async ({ context, role }) => {
    assert.ok(tokens[role]);
    await context.addCookies([{ name: "meibo-fixture-identity", value: role, url: origin, secure: true, httpOnly: true, sameSite: "Strict" }]);
  };
  const result = await verifyService({ origin, output, profile: "local", credentials: { scope: "synthetic fixture only", password: "synthetic-test-password-only" }, ...(mode === "access" ? { loginIdentity } : {}) });
  assert.ok(result.ok); report.sharedCases = result.steps.length;
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
  const browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext({ ignoreHTTPSErrors: true, serviceWorkers: "block" });
    await loginIdentity({ context, role: "admin" }); const page = await context.newPage();
    for (const width of [390, 768, 1440]) {
      await page.setViewportSize({ width, height: 1050 }); await page.goto(origin + "/login");
      assert.equal(await page.locator("#login-password, #login-username").count(), mode === "access" ? 0 : 2);
      assert.ok(await page.getByRole("button", { name: mode === "access" ? "Cloudflareでログイン" : "ログイン", exact: true }).isVisible());
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await page.screenshot({ path: join(output, `${mode}-login-${width}.png`), fullPage: true }); report.loginWidths.push(width);
    }
    if (mode === "access") {
      await page.route("**/auth/login", route => route.fulfill({ status: 401, contentType: "text/plain", body: "Access session expired" }));
      await page.locator("#login-tenant").fill("alpha");
      const navigation = page.waitForEvent("framenavigated", { predicate: frame => frame === page.mainFrame() });
      await page.locator("#login-submit").click(); await navigation;
      await page.waitForLoadState("domcontentloaded");
      assert.equal(new URL(page.url()).pathname, "/login");
      assert.equal(await page.locator("#login-form[data-cloudflare-access]").count(), 1);
      assert.ok(await page.locator("#login-error").isHidden());
      report.expiredAccessRecovery = true;
    }
  } finally { await browser.close(); }
  report.ok = true;
} catch (error) { report.error = { kind: error.name, location: error.stack?.match(/cloudflare-access-browser\.mjs:\d+:\d+/)?.[0] || "unavailable" }; process.exitCode = 1; }
finally {
  if (runtime && runtime.exitCode === null && runtime.signalCode === null) { const stopped = once(runtime, "exit"); runtime.kill("SIGTERM"); await stopped; }
  front.closeAllConnections(); await new Promise(done => front.close(done));
  await writeFile(join(output, "runtime.private.log"), diagnostic, { mode: 0o600 });
  await writeFile(join(output, "access-browser.json"), JSON.stringify(report, null, 2) + "\n", { mode: 0o600 });
  console.log(JSON.stringify(report));
}
