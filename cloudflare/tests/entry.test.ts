import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import test from "node:test";

// Substitute the platform base class only. The production entry, validation and
// coordinator are imported unchanged and exercise the real composition.
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier !== "cloudflare:workers") return nextResolve(specifier, context);
    return {
      url: `data:text/javascript,${encodeURIComponent("export class DurableObject { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }")}`,
      shortCircuit: true,
    };
  },
});
const { default: worker, MeiboContainer } = await import("../src/index.ts");
hooks.deregister();
const origin = "https://meibo-beta.example.workers.dev";
const config = JSON.stringify({ version: 1, control: { url_env: "MEIBO_DB_CONTROL_URL", token_env: "MEIBO_DB_CONTROL_TOKEN" }, tenants: {} });

function request(path = "/health", init = {}) {
  return new Request(`${origin}${path}`, { ...init, headers: { Host: new URL(origin).host, ...init.headers } });
}

function runtime() {
  const forwarded = [];
  const starts = [];
  const fake = {
    running: false,
    start(options) { starts.push(options); this.running = true; },
    async setInactivityTimeout() {},
    getTcpPort(port) {
      assert.equal(port, 4317);
      return { async fetch(req) {
        if (new URL(req.url).pathname === "/health") return new Response("ok");
        forwarded.push({ url: req.url, headers: req.headers, body: await req.text() });
        return new Response("application", { headers: [
          ["Content-Security-Policy", "default-src 'self'"], ["Cache-Control", "no-store"],
          ["Set-Cookie", "session=synthetic; Secure; HttpOnly; SameSite=Lax"],
          ["Set-Cookie", "context=synthetic; Secure; SameSite=Lax"],
        ] });
      } };
    },
  };
  return { fake, starts, forwarded };
}

function environment(namespace = { getByName() { throw new Error("unexpected allocation"); } }) {
  return { MEIBO_ORIGIN: origin, MEIBO_REMOTE_CONFIG: config,
    MEIBO_DB_CONTROL_URL: "https://synthetic.example", MEIBO_DB_CONTROL_TOKEN: "synthetic",
    MEIBO_CONTAINER: namespace, UNRELATED_SECRET: "must-not-forward" };
}

test("actual Worker and DO compose startup, boundaries, headers and secret allowlist", async () => {
  const rt = runtime();
  const env = environment();
  const object = new MeiboContainer({ container: rt.fake }, env);
  const names = [];
  env.MEIBO_CONTAINER = { getByName(name) { names.push(name); return { fetch: (req) => object.fetch(req) }; } };
  const response = await worker.fetch(request("/api/companies?q=one", {
    method: "POST", body: "synthetic-update", headers: {
      Origin: origin, Cookie: "session=synthetic", "X-CSRF-Token": "synthetic-csrf",
      "X-Meibo-Context": "synthetic-context", Forwarded: "host=attacker.invalid",
    },
  }), env);
  assert.equal(response.status, 200);
  assert.equal(await response.text(), "application");
  assert.deepEqual(names, ["meibo-beta-v1"]);
  assert.equal(rt.starts.length, 1);
  assert.equal(rt.starts[0].enableInternet, true);
  assert.deepEqual(Object.keys(rt.starts[0].env).sort(), ["MEIBO_ORIGIN", "MEIBO_REMOTE_CONFIG", "MEIBO_DB_CONTROL_URL", "MEIBO_DB_CONTROL_TOKEN"].sort());
  assert.equal(rt.forwarded.length, 1);
  const sent = rt.forwarded[0];
  assert.equal(sent.url, origin.replace("https:", "http:") + "/api/companies?q=one");
  for (const name of ["Host", "Origin", "Cookie", "X-CSRF-Token", "X-Meibo-Context"]) assert.ok(sent.headers.get(name));
  assert.equal(sent.headers.get("Forwarded"), null);
  assert.equal(sent.body, "synthetic-update");
  assert.equal(response.headers.getSetCookie().length, 2);
  assert.equal(response.headers.get("Content-Security-Policy"), "default-src 'self'");
  assert.equal(response.headers.get("Cache-Control"), "no-store");
});

test("both entry boundaries refuse spoofed requests without namespace or container access", async () => {
  const env = environment();
  const object = new MeiboContainer({ get container() { throw new Error("must not touch runtime"); } }, env);
  for (const entry of [(req) => worker.fetch(req, env), (req) => object.fetch(req)]) {
    const denied = await entry(new Request("https://preview.invalid/api/companies", { headers: { Host: "preview.invalid" } }));
    assert.equal(denied.status, 403);
    const method = await entry(request("/health", { method: "DELETE" }));
    assert.equal(method.status, 405);
    assert.equal(method.headers.get("Allow"), "GET, POST");
    const tooLarge = await entry(request("/api/companies", { method: "POST", headers: { "Content-Length": "1048577" } }));
    assert.equal(tooLarge.status, 413);
  }
});

test("a lost Durable Object response is uncertain and is never retried", async () => {
  let attempts = 0;
  const env = environment({ getByName() { return { async fetch() { attempts++; throw new Error("private-provider-detail"); } }; } });
  const response = await worker.fetch(request("/api/companies", { method: "POST", body: "update" }), env);
  assert.equal(response.status, 503);
  const body = await response.text();
  assert.equal(JSON.parse(body).code, "outcome_unknown");
  assert.equal(body.includes("private-provider-detail"), false);
  assert.equal(attempts, 1);
});

test("unavailable namespace and absent container fail before forwarding", async () => {
  const env = environment();
  assert.equal((await worker.fetch(request(), env)).status, 503);
  assert.equal((await new MeiboContainer({}, env).fetch(request())).status, 503);
});

test("unknown container errors remain uncertain rather than becoming completed boundary errors", async () => {
  const rt = runtime();
  let attempts = 0;
  rt.fake.getTcpPort = () => ({ async fetch(req) {
    if (new URL(req.url).pathname === "/health") return new Response("ok");
    attempts++; throw new Error("secret-runtime-error");
  } });
  const response = await new MeiboContainer({ container: rt.fake }, environment()).fetch(request("/api/companies", { method: "POST", body: "update" }));
  assert.equal(response.status, 503);
  assert.equal((await response.json()).code, "outcome_unknown");
  assert.equal(attempts, 1);
});
