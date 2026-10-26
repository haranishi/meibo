// Optional real-browser regression. All databases, users and traffic are synthetic.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import https from "node:https";
import net from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
assert.ok(process.env.MEIBO_BINARY && process.env.MEIBO_SQLD_BINARY, "explicit fixture binaries are required");
const binary = resolve(process.env.MEIBO_BINARY), sqld = resolve(process.env.MEIBO_SQLD_BINARY);
const output = resolve(process.env.MEIBO_BROWSER_OUTPUT || ".agent-harness/runs/remote-hosted-browser");
const scratch = await mkdtemp(join(tmpdir(), "meibo-remote-browser-"));
const password = "synthetic-remote-browser-password", token = "synthetic-remote-browser-token";
const children = new Set(), proxies = new Set(), agents = new Set(), diagnostics = [], databaseEndpoints = [];
const env = { ...process.env, MEIBO_TEST_REMOTE_LOOPBACK: "1" };
const report = { ok: false, phase: "setup", browser: "", steps: [], widths: [], externalRequests: 0, pageErrors: 0,
  databaseFailures: [], httpFailures: [], databaseExits: [], cliFailures: [],
  databaseTransport: { requests: 0, reusedUpstream: 0 },
  limits: "Local SQLD and Chromium over a generated loopback HTTPS fixture; Cloudflare/provider and physical-device behavior are separate checks." };
let backend, browser, front, activeConfig, backendPort, heldResponse, armed = false, onHeld;
const mark = (step) => { report.steps.push(step); report.phase = step; };
const delay = (ms) => new Promise((done) => setTimeout(done, ms));
function child(command, args, options = {}) {
  const process = spawn(command, args, { stdio: ["pipe", "pipe", "pipe"], ...options });
  children.add(process); process.once("exit", () => children.delete(process));
  let stderr = "";
  process.stderr?.on("data", (bytes) => {
    stderr += bytes.toString(); assert.ok(stderr.length <= 65536, "fixture diagnostic size exceeded");
  });
  process.once("exit", () => diagnostics.push(stderr));
  return process;
}
async function stop(process) {
  if (!process || process.exitCode !== null || process.signalCode !== null) return;
  const exited = once(process, "exit"); process.kill("SIGKILL"); await exited;
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
  process.once("exit", (code, signal) => { report.databaseExits.push({ database: name, code, signal }); });
  process.stdin.end(); process.stdout.resume();
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline && process.exitCode === null) {
    try { if ((await fetch(`${endpoint}/health`, { signal: AbortSignal.timeout(500) })).status === 200) {
      databaseEndpoints.push(endpoint); return { endpoint, address };
    } } catch { /* Only read-only startup health is polled. */ }
    await delay(25);
  }
  throw new Error("synthetic database did not become ready");
}
async function config(name, databases, prefix) {
  const reference = (key, endpoint) => {
    const url_env = `MEIBO_DB_${prefix}_${key}_URL`, token_env = `MEIBO_DB_${prefix}_${key}_TOKEN`;
    env[url_env] = endpoint; env[token_env] = token; return { url_env, token_env };
  };
  const value = { version: 1, control: reference("CONTROL", databases[0]),
    tenants: { alpha: reference("ALPHA", databases[1]), beta: reference("BETA", databases[2]) } };
  const path = join(scratch, `${name}.json`); await writeFile(path, JSON.stringify(value), { mode: 0o600 }); return path;
}
async function cli(config, args, input, success = true) {
  const process = child(binary, ["hosted-remote", config, ...args], { env });
  let stdout = ""; process.stdout.on("data", (bytes) => {
    stdout += bytes.toString(); assert.ok(stdout.length <= 65536, "fixture output size exceeded");
  });
  const exited = once(process, "exit"); process.stdin.end(input === undefined ? undefined : `${input}\n`);
  const timeout = setTimeout(() => process.kill("SIGKILL"), 45000);
  const [code] = await exited; clearTimeout(timeout);
  if (code !== 0) report.cliFailures.push({ command: args[0], code, signal: process.signalCode });
  assert.ok(success ? code === 0 : code !== 0, "synthetic CLI outcome differed");
  diagnostics.push(stdout); return stdout;
}
async function start(config, origin) {
  activeConfig = config;
  backend = child(binary, ["hosted-remote", config, "serve", "--origin", origin, "--bind", "127.0.0.1:0"], { env });
  backend.stdin.end();
  const lines = createInterface({ input: backend.stdout });
  const listening = await Promise.race([once(lines, "line").then(([line]) => line),
    once(backend, "exit").then(() => { throw new Error("fixture backend stopped"); }),
    new Promise((_, reject) => setTimeout(() => reject(new Error("fixture backend startup timed out")), 15000))]);
  assert.ok(/^hosted listening: 127\.0\.0\.1:\d+ /.test(listening), "fixture startup announcement missing");
  backendPort = Number(listening.split(" ")[2].split(":")[1]); lines.close(); backend.stdout.resume();
}
async function closeServer(server) {
  if (!server) return; server.closeAllConnections(); await new Promise((done) => server.close(done));
}
async function databaseProxy(server, label, gateFault = false) {
  // Pool only the fixture's upstream hop. Forwarding a client's Connection: close
  // would multiply short-lived loopback sockets and exhaust macOS test ports.
  // The application's no-pool/no-retry transport remains untouched.
  const agent = new http.Agent({ keepAlive: true, maxSockets: 4, maxFreeSockets: 4 }); agents.add(agent);
  const proxy = http.createServer(async (request, response) => {
    try {
      const chunks = []; let size = 0;
      for await (const chunk of request) { size += chunk.length; assert.ok(size <= 1024 * 1024); chunks.push(chunk); }
      const body = Buffer.concat(chunks), requests = JSON.parse(body).requests;
      // These local calls otherwise exceed macOS's 16K ephemeral-port range in
      // seconds. Model a small remote RTT, without retrying any SQL request or
      // changing the application's transport and timeout rules.
      await delay(5);
      const closing = requests.some((item) => item.type === "close");
      const shouldHold = gateFault && armed && requests.some((item) => item.type === "execute"
        && item.stmt?.sql?.startsWith("INSERT INTO remote_gate")
        && item.stmt?.args?.some((value) => value.type === "text" && value.value === "cli.backup"));
      if (shouldHold) armed = false;
      const headers = { ...request.headers, connection: "keep-alive", "content-length": String(body.length) };
      delete headers["transfer-encoding"]; delete headers["keep-alive"]; delete headers["proxy-connection"];
      const upstream = http.request({ host: "127.0.0.1", port: server.address, method: request.method,
        path: request.url, headers, agent }, (incoming) => {
        report.databaseTransport.requests++; if (upstream.reusedSocket) report.databaseTransport.reusedUpstream++;
        const chunks = []; let size = 0;
        incoming.on("data", (chunk) => {
          size += chunk.length;
          if (size > 16 * 1024 * 1024) { incoming.destroy(); response.destroy(); return; }
          chunks.push(chunk);
        });
        incoming.on("end", () => {
          const body = Buffer.concat(chunks);
          try {
            const parsed = JSON.parse(body);
            const errors = (parsed.results || []).filter((item) => item.type === "error");
            if ((incoming.statusCode !== 200 || errors.length || (!closing && !parsed.baton)) && report.databaseFailures.length < 32) {
              report.databaseFailures.push({ database: label, status: incoming.statusCode, nullBaton: !parsed.baton, closing,
                codes: errors.map((item) => ["STREAM_EXPIRED", "STREAM_NOT_FOUND", "SQLITE_BUSY", "SQLITE_CONSTRAINT"].includes(item.error?.code) ? item.error.code : "server_error") });
            }
          } catch { if (report.databaseFailures.length < 32) report.databaseFailures.push({ database: label, malformed: true }); }
          if (shouldHold) { heldResponse = response; onHeld(); }
          else { response.writeHead(incoming.statusCode, incoming.headers); response.end(body); }
        });
      });
      upstream.on("error", (error) => {
        if (report.databaseFailures.length < 32) report.databaseFailures.push({ database: label,
          network: ["ECONNREFUSED", "ECONNRESET", "EADDRNOTAVAIL", "ETIMEDOUT", "ENOBUFS"].includes(error.code) ? error.code : "network_error" });
        response.writeHead(502); response.end();
      }); upstream.end(body);
    } catch { response.writeHead(502); response.end(); }
  });
  proxies.add(proxy); await new Promise((done) => proxy.listen(0, "127.0.0.1", done));
  const endpoint = `http://127.0.0.1:${proxy.address().port}`; databaseEndpoints.push(endpoint); return endpoint;
}
async function compareSnapshots(source, target) {
  const a = JSON.parse(await readFile(join(source, "manifest.json"))), b = JSON.parse(await readFile(join(target, "manifest.json")));
  assert.ok(a.service.instance !== b.service.instance && a.service.database_id !== b.service.database_id, "new service identities required");
  const markerKeys = ["database_id", "instance", "dummy_hash", "state", "remote_format", "schema_version", "database_kind"];
  const rows = async (root, table) => (await readFile(join(root, table.file), "utf8")).split("\n").filter(Boolean).map((line) => JSON.parse(line));
  for (const [left, right, control] of [[a.control, b.control, true], ...a.tenants.map((item, i) => {
    assert.ok(item.slug === b.tenants[i].slug && item.database.database_id !== b.tenants[i].database.database_id, "tenant mapping was not renewed");
    assert.deepEqual(item.database.sequences, b.tenants[i].database.sequences);
    return [item.database, b.tenants[i].database, false];
  })]) {
    for (let i = 0; i < left.tables.length; i++) {
      const table = left.tables[i], other = right.tables[i]; assert.equal(table.name, other.name);
      let original = await rows(source, table), restored = await rows(target, other);
      if (table.name === "meta") {
        const omitted = control ? markerKeys : ["database_id"];
        original = original.filter((row) => !omitted.includes(row.values[0].value));
        restored = restored.filter((row) => !omitted.includes(row.values[0].value));
      } else if (control && table.name === "remote_tenants") {
        assert.ok(original.length === restored.length && original.every((row, j) => row.values[0].value === restored[j].values[0].value
          && row.values[1].value !== restored[j].values[1].value), "restored tenant registration mismatch"); continue;
      } else if (control && table.name === "audit") {
        assert.equal(restored.length, original.length + 1);
        assert.ok(restored.at(-1).values[4].value === "restore", "restore audit missing"); restored = restored.slice(0, -1);
      }
      assert.ok(JSON.stringify(original) === JSON.stringify(restored), "complete typed-table round trip differed");
    }
  }
}

try {
  await mkdir(output, { recursive: true });
  const source = await Promise.all(["control", "alpha", "beta"].map(database));
  const target = await Promise.all(["new-control", "new-alpha", "new-beta"].map(database));
  // Preserve real protocol bytes and record only fixed status/code classes.
  // One armed fault holds the actual committed CLI backup gate reply.
  const sourceEndpoints = await Promise.all(source.map((server, i) => databaseProxy(server, ["control", "alpha", "beta"][i], i === 0)));
  const targetEndpoints = await Promise.all(target.map((server, i) => databaseProxy(server, ["new-control", "new-alpha", "new-beta"][i])));
  const sourceConfig = await config("source", sourceEndpoints, "SOURCE");
  const targetConfig = await config("target", targetEndpoints, "TARGET");
  await cli(sourceConfig, ["init"]);
  for (const slug of ["alpha", "beta"]) await cli(sourceConfig, ["tenant-add", slug, "--origin", "synthetic", "--demo-rows", "3"]);
  for (const role of ["admin", "editor", "viewer"]) {
    await cli(sourceConfig, ["user-add", role, "--password-stdin"], password);
    for (const slug of ["alpha", "beta"]) await cli(sourceConfig, ["member-set", role, slug, role]);
  }
  const key = join(scratch, "fixture-key.pem"), cert = join(scratch, "fixture-cert.pem");
  assert.equal(spawnSync(process.env.OPENSSL_BINARY || "openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes",
    "-keyout", key, "-out", cert, "-days", "1", "-subj", "/CN=localhost"], { stdio: "ignore" }).status, 0);
  front = https.createServer({ key: await readFile(key), cert: await readFile(cert) }, (request, response) => {
    const upstream = http.request({ host: "127.0.0.1", port: backendPort, path: request.url,
      method: request.method, headers: request.headers }, (incoming) => {
      response.writeHead(incoming.statusCode, incoming.headers); incoming.pipe(response);
    });
    upstream.on("error", () => { response.writeHead(502); response.end(); }); request.pipe(upstream);
  });
  await new Promise((done) => front.listen(0, "127.0.0.1", done));
  const origin = `https://127.0.0.1:${front.address().port}`;
  await start(sourceConfig, origin);
  browser = await chromium.launch({ headless: true }); report.browser = browser.version();
  // Certificate exception is confined to this generated local browser fixture.
  const context = await browser.newContext({ ignoreHTTPSErrors: true, viewport: { width: 1440, height: 1050 } });
  await context.route("**/*", (route) => {
    if (new URL(route.request().url()).origin !== origin) { report.externalRequests++; return route.abort(); }
    return route.continue();
  });
  context.on("page", (page) => page.on("pageerror", () => report.pageErrors++));
  context.on("response", (response) => {
    if (response.status() < 400 || report.httpFailures.length >= 32) return;
    const path = new URL(response.url()).pathname;
    const route = ["/login", "/auth/login", "/auth/logout", "/auth/session", "/auth/audit"].includes(path) ? path : path.startsWith("/api/") ? "/api/*" : "asset";
    report.httpFailures.push({ phase: report.phase, route, status: response.status() });
  });
  const page = await context.newPage(); page.setDefaultTimeout(15000);
  async function login(user, tenant) {
    await page.goto(`${origin}/login`); await page.locator("#login-tenant").fill(tenant);
    await page.locator("#login-username").fill(user); await page.locator("#login-password").fill(password);
    await page.locator("#login-submit").click(); await page.waitForURL(`${origin}/`);
    await page.locator("#company-rows .company-name").first().waitFor();
  }
  async function logout() { await page.getByRole("button", { name: "ログアウト", exact: true }).click(); await page.waitForURL("**/login"); }
  async function api(path, method = "GET", body, headers = {}) {
    return page.evaluate(async ({ path, method, body, headers }) => {
      const session = await (await fetch("/auth/session")).json();
      const response = await fetch(path, { method, headers: { "Content-Type": "application/json", "X-Meibo-Local": "1",
        "X-Meibo-Context": session.storage_scope, ...headers }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
      return { status: response.status, body: await response.json() };
    }, { path, method, body, headers });
  }
  await login("admin", "alpha"); mark("admin-login");
  await page.locator("#company-rows .company-name").first().click();
  const alphaMarker = "合成alpha組織ブラウザ確認";
  await page.getByRole("button", { name: "取引先を編集", exact: true }).click();
  await page.locator("#company-name").fill(alphaMarker); await page.locator("#company-submit").click();
  await page.waitForFunction((name) => document.querySelector("#selected-company-title")?.textContent === name, alphaMarker);
  await page.locator("#activity-note").fill("合成alphaの保存記録");
  await page.locator("#activity-form button[type=submit]").click();
  await page.locator(".timeline-note").filter({ hasText: "合成alphaの保存記録" }).first().waitFor(); mark("admin-ui-write");
  for (const width of [390, 768, 1440]) {
    await page.setViewportSize({ width, height: 1050 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "horizontal overflow");
    await page.screenshot({ path: join(output, `record-${width}.png`), fullPage: true }); report.widths.push(width);
  }
  await page.setViewportSize({ width: 1440, height: 1050 });
  // The page auth wrapper replaces caller context with the current scope;
  // a raw browser request client is needed to exercise a deliberately stale one.
  assert.equal((await page.request.get(`${origin}/api/companies`, { headers: { "X-Meibo-Context": "wrong-context" } })).status(), 409);
  assert.equal((await page.request.post(`${origin}/api/companies/1/activities`, { headers: { Origin: "https://foreign.example", "X-Meibo-Local": "1" }, data: { note: "synthetic rejection" } })).status(), 403);
  mark("csrf-and-context-rejections");
  const old = await context.newPage(); old.setDefaultTimeout(15000); await old.goto(origin);
  await old.locator("#company-rows .company-name").first().waitFor();
  let releaseAuth; const blocked = new Promise((done) => { releaseAuth = done; });
  await old.route("**/auth/session", async (route) => { await blocked; await route.continue(); });
  await old.evaluate(() => { dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true })); dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true })); });
  await logout(); await login("editor", "beta");
  assert.ok(!(await page.locator("body").innerText()).includes(alphaMarker), "cross-tenant business content leaked");
  assert.ok(await old.evaluate(() => document.body.hidden && getComputedStyle(document.body).display === "none"), "old tab was not hidden during recheck");
  releaseAuth(); await old.waitForURL("**/login"); await old.close(); mark("old-tab-and-tenant-isolation");
  await page.locator("#company-rows .company-name").first().click();
  await page.locator("#activity-note").fill("合成betaのeditor保存記録"); await page.locator("#activity-form button[type=submit]").click();
  await page.locator(".timeline-note").filter({ hasText: "合成betaのeditor保存記録" }).first().waitFor();
  assert.equal((await api("/api/companies/1/contacts/1/erase", "POST", { wipe_notes: false })).status, 403); mark("editor-write-and-admin-restriction");
  await logout(); await login("viewer", "alpha"); await page.locator("#company-rows .company-name").first().click();
  await page.locator("#activity-note").fill("拒否される合成viewer入力"); await page.locator("#activity-form button[type=submit]").click();
  await page.locator("#activity-error:not([hidden])").waitFor();
  assert.equal(await page.locator("#activity-note").inputValue(), "拒否される合成viewer入力");
  assert.equal((await api("/auth/audit")).status, 403); mark("viewer-rejection-keeps-input");
  await logout(); assert.equal((await page.request.get(`${origin}/api/summary`)).status(), 401);
  await page.goBack(); await delay(200); assert.ok(!(await page.locator("body").innerText()).includes(alphaMarker)); mark("logout-and-history");
  await login("admin", "alpha"); const originalScope = (await api("/auth/session")).body.storage_scope;
  await stop(backend); await start(sourceConfig, origin); await page.reload();
  await page.locator("#company-rows .company-name").filter({ hasText: alphaMarker }).waitFor();
  assert.ok((await api("/auth/session")).body.storage_scope === originalScope, "restart lost the service/session identity"); mark("process-restart-keeps-session-and-data");
  const held = new Promise((done) => { onHeld = done; }); armed = true;
  const crashing = child(binary, ["hosted-remote", sourceConfig, "backup", join(scratch, "interrupted")], { env });
  crashing.stdin.end(); crashing.stdout.resume();
  await Promise.race([held, delay(15000).then(() => { throw new Error("gate fault was not reached"); })]);
  await stop(crashing); heldResponse.destroy();
  const gate = JSON.parse(await cli(sourceConfig, ["gate-status"])); assert.equal(gate.occupied, true);
  assert.equal(gate.operation, "cli.backup");
  assert.equal((await page.request.get(`${origin}/api/companies`)).status(), 503);
  await stop(backend); await start(sourceConfig, origin);
  assert.equal((await page.request.get(`${origin}/login`)).status(), 503);
  await cli(sourceConfig, ["gate-recover", "0".repeat(64), "--prior-work-terminated"], undefined, false);
  await cli(sourceConfig, ["gate-recover", gate.owner], undefined, false);
  assert.equal(JSON.parse(await cli(sourceConfig, ["gate-status"])).occupied, true);
  await cli(sourceConfig, ["gate-recover", gate.owner, "--prior-work-terminated"]);
  assert.equal(JSON.parse(await cli(sourceConfig, ["gate-status"])).occupied, false); mark("killed-process-gate-persists-and-explicit-recovery");
  await page.reload(); await page.locator("#company-rows .company-name").first().waitFor();
  const snapshot = join(scratch, "snapshot"), restoredSnapshot = join(scratch, "restored-snapshot");
  await cli(sourceConfig, ["backup", snapshot]);
  await cli(sourceConfig, ["restore", snapshot, "--target-config", targetConfig]);
  await cli(targetConfig, ["backup", restoredSnapshot]); await compareSnapshots(snapshot, restoredSnapshot); mark("all-typed-tables-backup-restore-compared");
  await stop(backend); await start(targetConfig, origin);
  assert.equal((await page.request.get(`${origin}/api/summary`)).status(), 401);
  await page.reload(); await page.waitForURL("**/login"); mark("restored-service-rejects-old-session");
  for (const role of ["admin", "editor", "viewer"]) for (const tenant of ["alpha", "beta"]) {
    report.phase = `restored-${role}-${tenant}-login`;
    await login(role, tenant); const session = (await api("/auth/session")).body;
    report.phase = `restored-${role}-${tenant}-session`;
    assert.equal(session.tenant, tenant); assert.equal(session.role, role);
    if (role === "admin" && tenant === "alpha") {
      assert.ok(session.storage_scope !== originalScope, "restore did not renew browser scope");
      report.phase = "restored-admin-alpha-data";
      await page.locator("#company-rows .company-name").filter({ hasText: alphaMarker }).click();
      await page.locator(".timeline-note").filter({ hasText: "合成alphaの保存記録" }).first().waitFor();
    }
    await logout();
  }
  mark("restored-two-tenants-three-roles-relogin");
  for (const text of diagnostics) assert.ok(![password, token, ...databaseEndpoints].some((value) => text.includes(value))
    && !/\b(?:SELECT\s|INSERT INTO\s|UPDATE\s+meta\b)/i.test(text), "private database diagnostic leaked");
  assert.equal(report.externalRequests, 0); assert.equal(report.pageErrors, 0);
  assert.equal(report.databaseFailures.length, 0);
  assert.ok(report.cliFailures.length === 2 && report.cliFailures.every((item) => item.command === "gate-recover" && item.code === 1), "unexpected CLI failure");
  report.ok = true; mark("diagnostic-and-browser-checks-passed");
} catch (error) {
  // Failure reporting deliberately excludes exception text, data, cookies and URLs.
  report.failure = { kind: ["Error", "AssertionError", "TimeoutError"].includes(error?.name) ? error.name : "Error",
    location: error?.stack?.match(/remote-hosted-browser\.mjs:\d+:\d+/)?.[0] || "unavailable" };
  try {
    const gate = JSON.parse(await cli(activeConfig, ["gate-status"]));
    report.failure.gate = { occupied: gate.occupied, operation: ["cli.backup", "http.request"].includes(gate.operation) ? gate.operation : "other" };
  } catch { report.failure.gate = { inspected: false }; }
  report.ok = false; process.exitCode = 1; console.error(`remote browser failed at phase: ${report.phase}`);
} finally {
  heldResponse?.destroy(); await browser?.close();
  for (const agent of agents) agent.destroy();
  for (const process of [...children]) await stop(process);
  await closeServer(front); for (const proxy of proxies) await closeServer(proxy);
  await mkdir(output, { recursive: true }); await writeFile(join(output, "browser.json"), JSON.stringify(report, null, 2));
  await rm(scratch, { recursive: true, force: true });
  console.log(JSON.stringify({ ok: report.ok, steps: report.steps.length, externalRequests: report.externalRequests, pageErrors: report.pageErrors }));
}
