import assert from "node:assert/strict";
import { setImmediate } from "node:timers";
import test from "node:test";
import { ContainerCoordinator, runtimeEnvironment } from "../src/container.ts";
import type { CoordinatorClock } from "../src/container.ts";
import type { ValidatedRequest } from "../src/contracts.ts";

const ORIGIN = { origin: "https://meibo.example", host: "meibo.example" };
const MIB = 1024 * 1024;
const CONFIG = {
  version: 1,
  control: { url_env: "MEIBO_DB_CONTROL_URL", token_env: "MEIBO_DB_CONTROL_TOKEN" },
  tenants: { alpha: { url_env: "MEIBO_DB_ALPHA_URL", token_env: "MEIBO_DB_ALPHA_TOKEN" } },
};

test("explicit Cloudflare Access config is forwarded without leaking other bindings", () => {
  const access = JSON.stringify({ version: 1, issuer: "https://synthetic.cloudflareaccess.com", audience: "synthetic-audience", users: [{ email: "operator@example.invalid", username: "operator" }] });
  const output = runtimeEnvironment({ ...environment(), MEIBO_ACCESS_CONFIG: access }, ORIGIN);
  assert.equal(output.MEIBO_ACCESS_CONFIG, access);
  assert.equal(Object.hasOwn(output, "ANOTHER_BINDING"), false);
});
test("a supplied invalid Access config refuses startup instead of choosing password login", () => {
  for (const access of ["", "not-json", "{}", "null", JSON.stringify({ version: 2 }), "x".repeat(32 * 1024 + 1)]) {
    assert.throws(() => runtimeEnvironment({ ...environment(), MEIBO_ACCESS_CONFIG: access }, ORIGIN));
  }
});

function environment(config: unknown = CONFIG): Record<string, string> {
  return {
    MEIBO_ORIGIN: ORIGIN.origin,
    MEIBO_REMOTE_CONFIG: JSON.stringify(config),
    MEIBO_DB_CONTROL_URL: "https://control.synthetic.example",
    MEIBO_DB_CONTROL_TOKEN: "synthetic-control-value",
    MEIBO_DB_ALPHA_URL: "https://alpha.synthetic.example",
    MEIBO_DB_ALPHA_TOKEN: "synthetic-alpha-value",
    MEIBO_DB_UNRELATED: "unrelated-synthetic-value",
    ANOTHER_BINDING: "must-not-be-forwarded",
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

async function flush(): Promise<void> {
  await new Promise<void>((resolve) => setImmediate(resolve));
}

class ManualClock implements CoordinatorClock {
  private current = 0;
  private serial = 0;
  private timers = new Map<number, { at: number; callback: () => void }>();
  now() { return this.current; }
  setTimeout(callback: () => void, delayMs: number) {
    const id = ++this.serial;
    this.timers.set(id, { at: this.current + delayMs, callback });
    return id;
  }
  clearTimeout(handle: number) { this.timers.delete(handle); }
  jumpWithoutTimers(delay: number) { this.current += delay; }
  async advance(delay: number): Promise<void> {
    const target = this.current + delay;
    while (true) {
      const due = [...this.timers].filter(([, value]) => value.at <= target).sort((a, b) => a[1].at - b[1].at)[0];
      if (!due) break;
      this.current = due[1].at;
      this.timers.delete(due[0]);
      due[1].callback();
      await flush();
    }
    this.current = target;
    await flush();
  }
  count() { return this.timers.size; }
}

class FakeRuntime {
  running = false;
  starts: { env: Record<string, string>; enableInternet: true }[] = [];
  ports: number[] = [];
  inactivity: (number | bigint)[] = [];
  health: Request[] = [];
  requests: Request[] = [];
  signals: AbortSignal[] = [];
  startFailure: unknown;
  inactivityFailure: unknown;
  healthHandler: (request: Request, signal: AbortSignal) => Promise<Response> = async () => new Response("ok");
  handler: (request: Request, signal: AbortSignal) => Promise<Response> = async () => new Response("done");
  start(options: { env: Record<string, string>; enableInternet: true }) {
    if (this.startFailure) throw this.startFailure;
    this.starts.push(options);
    this.running = true;
  }
  getTcpPort(port: number) {
    this.ports.push(port);
    return {
      fetch: async (input: Request, init?: RequestInit) => {
        const signal = init?.signal ?? input.signal;
        if (new URL(input.url).pathname === "/health") {
          this.health.push(input);
          return this.healthHandler(input, signal);
        }
        this.requests.push(input);
        this.signals.push(signal);
        return this.handler(input, signal);
      },
    };
  }
  async setInactivityTimeout(value: number | bigint) {
    this.inactivity.push(value);
    if (this.inactivityFailure) throw this.inactivityFailure;
  }
}

function request(path = "/api/update", body?: ReadableStream<Uint8Array>, signal?: AbortSignal): ValidatedRequest {
  const init: RequestInit & { duplex: "half" } = {
    method: "POST", headers: { Host: ORIGIN.host, Origin: ORIGIN.origin }, duplex: "half",
    ...(body ? { body } : {}), ...(signal ? { signal } : {}),
  };
  return { request: new Request(`${ORIGIN.origin}${path}`, init), origin: ORIGIN, method: "POST", bodyLimitBytes: MIB };
}

function lazyBody(size = 32 * MIB) {
  let pulls = 0;
  let cancelled = 0;
  let remaining = size;
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      pulls++;
      if (!remaining) { controller.close(); return; }
      const n = Math.min(MIB, remaining);
      remaining -= n;
      controller.enqueue(new Uint8Array(n));
    },
    cancel() { cancelled++; },
  }, { highWaterMark: 0 });
  return { body, pulls: () => pulls, cancelled: () => cancelled };
}

function prepare(value: ValidatedRequest): Request {
  return new Request(`http://${ORIGIN.host}${new URL(value.request.url).pathname}`, value.request);
}

function setup() {
  const runtime = new FakeRuntime();
  const clock = new ManualClock();
  const coordinator = new ContainerCoordinator(runtime, environment(), ORIGIN, clock);
  return { runtime, clock, coordinator };
}

async function failure(response: Response, code: string): Promise<void> {
  assert.equal(response.status, 503);
  assert.equal(response.headers.get("Cache-Control"), "no-store");
  const body = await response.text();
  assert.equal(JSON.parse(body).code, code);
  assert.doesNotMatch(body, /synthetic-control-value|synthetic-alpha-value|secret-query|SQL-SENSITIVE|must-not-be-forwarded/);
}

test("runtime environment transfers only referenced secrets and explicit configuration", () => {
  const env = environment();
  const actual = runtimeEnvironment(env, ORIGIN);
  assert.deepEqual(Object.keys(actual).sort(), ["MEIBO_ORIGIN", "MEIBO_REMOTE_CONFIG", "MEIBO_DB_CONTROL_URL", "MEIBO_DB_CONTROL_TOKEN", "MEIBO_DB_ALPHA_URL", "MEIBO_DB_ALPHA_TOKEN"].sort());
  assert.equal(actual.MEIBO_REMOTE_CONFIG, env.MEIBO_REMOTE_CONFIG);
  assert.equal(actual.MEIBO_ORIGIN, ORIGIN.origin);
});

test("zero-tenant initialization config is accepted", () => {
  const actual = runtimeEnvironment(environment({ ...CONFIG, tenants: {} }), ORIGIN);
  assert.equal(actual.MEIBO_DB_CONTROL_TOKEN, "synthetic-control-value");
  assert.equal(actual.MEIBO_DB_ALPHA_TOKEN, undefined);
});

test("malformed or oversized config and unsafe configured origins reject privately", () => {
  const values = ["{SQL-SENSITIVE", " ", "x".repeat(MIB + 1)];
  for (const value of values) {
    assert.throws(() => runtimeEnvironment({ ...environment(), MEIBO_REMOTE_CONFIG: value }, ORIGIN), /公開環境の設定を確認してください。/);
  }
  for (const value of ["http://meibo.example", "https://meibo.example/", "https://meibo.example/path", "https://user:pass@meibo.example", "https://elsewhere.example"]) {
    assert.throws(() => runtimeEnvironment({ ...environment(), MEIBO_ORIGIN: value }, ORIGIN), /公開環境の設定を確認してください。/);
  }
  assert.equal(runtimeEnvironment({ ...environment(), MEIBO_ORIGIN: "https://meibo.example:443" }, ORIGIN).MEIBO_ORIGIN, ORIGIN.origin);
});

test("invalid config shape, references, slugs and duplicate bindings fail with fixed errors", () => {
  const invalid = [null, [], {}, { ...CONFIG, version: 2 }, { ...CONFIG, extra: "SQL-SENSITIVE" },
    { ...CONFIG, control: { ...CONFIG.control, extra: 1 } },
    { ...CONFIG, control: { ...CONFIG.control, token_env: "UNSCOPED_TOKEN" } },
    { ...CONFIG, control: { ...CONFIG.control, token_env: CONFIG.control.url_env } },
    { ...CONFIG, tenants: [] }, { ...CONFIG, tenants: { "bad/slug": CONFIG.tenants.alpha } },
    { ...CONFIG, tenants: { "-alpha": CONFIG.tenants.alpha } }, { ...CONFIG, tenants: { "alpha-": CONFIG.tenants.alpha } },
    { ...CONFIG, tenants: { ["a".repeat(49)]: CONFIG.tenants.alpha } },
    { ...CONFIG, tenants: { alpha: { url_env: CONFIG.control.url_env, token_env: CONFIG.tenants.alpha.token_env } } },
    { ...CONFIG, tenants: { alpha: { ...CONFIG.tenants.alpha, unknown: 1 } } },
    { ...CONFIG, tenants: Object.fromEntries(Array.from({ length: 1001 }, (_, i) => [`tenant-${i}`, CONFIG.tenants.alpha])) }];
  for (const config of invalid) {
    assert.throws(() => runtimeEnvironment(environment(config), ORIGIN), (error: unknown) => {
      assert.equal((error as Error).message, "公開環境の設定を確認してください。");
      assert.doesNotMatch(String(error), /SQL-SENSITIVE|synthetic/);
      return true;
    });
  }
});

test("missing, empty and non-string required secrets reject without starting", async () => {
  for (const value of [undefined, "", 42]) {
    const runtime = new FakeRuntime();
    const env = environment();
    (env as Record<string, unknown>).MEIBO_DB_ALPHA_TOKEN = value;
    const coordinator = new ContainerCoordinator(runtime, env, ORIGIN, new ManualClock());
    await failure(await coordinator.forward(request(), () => prepare(request())), "unavailable");
    assert.equal(runtime.starts.length, 0);
    assert.equal(runtime.health.length, 0);
    assert.equal(runtime.requests.length, 0);
  }
});

test("inherited secrets cannot satisfy explicit environment references", () => {
  const env = environment();
  const secret = env.MEIBO_DB_ALPHA_TOKEN;
  delete env.MEIBO_DB_ALPHA_TOKEN;
  Object.setPrototypeOf(env, { MEIBO_DB_ALPHA_TOKEN: secret });
  assert.throws(() => runtimeEnvironment(env, ORIGIN), /公開環境の設定を確認してください。/);
});

test("concurrent cold requests share startup and preserve public authority", async () => {
  const { runtime, coordinator, clock } = setup();
  runtime.handler = async (req) => new Response(new URL(req.url).pathname);
  const first = request("/api/first");
  const second = request("/api/second");
  const results = await Promise.all([coordinator.forward(first, () => prepare(first)), coordinator.forward(second, () => prepare(second))]);
  assert.deepEqual(await Promise.all(results.map((value) => value.text())), ["/api/first", "/api/second"]);
  assert.equal(runtime.starts.length, 1);
  assert.equal(runtime.health.length, 1);
  assert.equal(runtime.starts[0].enableInternet, true);
  assert.equal(runtime.starts[0].env.ANOTHER_BINDING, undefined);
  assert.equal(runtime.starts[0].env.MEIBO_DB_UNRELATED, undefined);
  assert.ok(runtime.ports.every((value) => value === 4317));
  assert.equal(runtime.requests[0].headers.get("Host"), ORIGIN.host);
  assert.equal(runtime.requests[0].headers.get("Origin"), ORIGIN.origin);
  assert.equal(new URL(runtime.requests[0].url).protocol, "http:");
  assert.equal(clock.count(), 0);
});

test("readiness retries only GET health and never resends user writes", async () => {
  const { runtime, coordinator, clock } = setup();
  runtime.healthHandler = async () => new Response("not yet", { status: runtime.health.length === 1 ? 503 : 200 });
  const req = request();
  const result = coordinator.forward(req, () => prepare(req));
  await flush();
  assert.equal(runtime.requests.length, 0);
  await clock.advance(100);
  assert.equal((await result).status, 200);
  assert.equal(runtime.health.length, 2);
  assert.ok(runtime.health.every((value) => value.method === "GET" && value.headers.get("Host") === ORIGIN.host && value.redirect === "manual"));
  assert.equal(runtime.requests.length, 1);
});

test("already running runtime after DO restart reapplies busy and idle conditions", async () => {
  const { runtime, coordinator } = setup();
  runtime.running = true;
  const req = request();
  assert.equal((await coordinator.forward(req, () => prepare(req))).status, 200);
  assert.equal(runtime.starts.length, 0);
  assert.equal(runtime.health.length, 1);
  assert.equal(runtime.inactivity[0], 180_000);
  assert.equal(runtime.inactivity.at(-1), 60_000);
});

test("runtime inactivity API failure prevents forwarding", async () => {
  const { runtime, coordinator } = setup();
  runtime.running = true;
  runtime.inactivityFailure = new Error("SQL-SENSITIVE");
  const req = request();
  await failure(await coordinator.forward(req, () => prepare(req)), "unavailable");
  assert.equal(runtime.requests.length, 0);
});

test("start failure is a known unavailable result without retries", async () => {
  const { runtime, coordinator } = setup();
  runtime.startFailure = new Error("secret-query synthetic-control-value");
  const req = request();
  await failure(await coordinator.forward(req, () => prepare(req)), "unavailable");
  assert.equal(runtime.requests.length, 0);
  assert.equal(runtime.health.length, 0);
});

test("readiness has a 30 second deadline and does not stop or send user operations", async () => {
  const { runtime, coordinator, clock } = setup();
  runtime.healthHandler = async () => new Promise(() => {});
  const req = request();
  const result = coordinator.forward(req, () => prepare(req));
  await flush();
  await clock.advance(29_999);
  assert.equal(runtime.requests.length, 0);
  await clock.advance(1);
  await failure(await result, "unavailable");
  assert.equal(runtime.starts.length, 1);
  assert.equal(runtime.requests.length, 0);
  assert.equal(clock.count(), 0);
});

test("forward timeout is outcome_unknown and cannot prove the old operation was cancelled", async () => {
  const { runtime, coordinator, clock } = setup();
  const sent = deferred<Response>();
  runtime.handler = () => sent.promise;
  const req = request();
  const result = coordinator.forward(req, () => prepare(req));
  await flush();
  assert.equal(runtime.requests.length, 1);
  await clock.advance(180_000);
  await failure(await result, "outcome_unknown");
  assert.equal(runtime.requests.length, 1);
  assert.equal(runtime.starts.length, 1);
  assert.equal(runtime.signals[0].aborted, true);
  assert.equal(runtime.inactivity.at(-1), 180_000);
  sent.resolve(new Response("late committed result"));
  await flush();
  assert.equal(runtime.requests.length, 1);
});

test("an overdue timer cannot allow a late bodyless response to succeed", async () => {
  const { runtime, coordinator, clock } = setup();
  runtime.handler = async () => {
    clock.jumpWithoutTimers(180_001);
    return new Response(null, { status: 204 });
  };
  const req = request();
  await failure(await coordinator.forward(req, () => prepare(req)), "outcome_unknown");
  assert.equal(runtime.requests.length, 1);
  assert.equal(clock.count(), 0);
  assert.equal(runtime.inactivity.at(-1), 180_000);
});

test("forward body deadline aborts and cancels a stalled response without retry", async () => {
  const { runtime, coordinator, clock } = setup();
  let cancelled = 0;
  runtime.handler = async () => new Response(new ReadableStream({ cancel() { cancelled++; } }, { highWaterMark: 0 }));
  const req = request();
  const result = coordinator.forward(req, () => prepare(req));
  await flush();
  await clock.advance(180_000);
  await failure(await result, "outcome_unknown");
  assert.equal(cancelled, 1);
  assert.equal(runtime.requests.length, 1);
});

test("the full 33 MiB response cap is accepted and length is regenerated", async () => {
  const { runtime, coordinator } = setup();
  runtime.handler = async () => new Response(lazyBody(33 * MIB).body, { headers: { "Content-Length": "0" } });
  const req = request();
  const response = await coordinator.forward(req, () => prepare(req));
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("Content-Length"), null);
  assert.equal((await response.arrayBuffer()).byteLength, 33 * MIB);
});

test("many tiny upstream chunks preserve all bytes without changing the result", async () => {
  const { runtime, coordinator } = setup();
  runtime.handler = async () => {
    let count = 0;
    return new Response(new ReadableStream<Uint8Array>({
      pull(controller) {
        if (count === 20_000) controller.close();
        else { controller.enqueue(Uint8Array.of(count % 251)); count++; }
      },
    }, { highWaterMark: 0 }));
  };
  const req = request();
  const bytes = new Uint8Array(await (await coordinator.forward(req, () => prepare(req))).arrayBuffer());
  assert.equal(bytes.length, 20_000);
  assert.ok(bytes.every((value, i) => value === i % 251));
});

test("oversized success and error responses fail after one send and cancel the source", async () => {
  for (const status of [200, 500]) {
    const { runtime, coordinator } = setup();
    const source = lazyBody(33 * MIB + 1);
    runtime.handler = async () => new Response(source.body, { status, headers: { "Content-Length": "1" } });
    const req = request();
    await failure(await coordinator.forward(req, () => prepare(req)), "outcome_unknown");
    assert.equal(runtime.requests.length, 1);
    assert.equal(source.cancelled(), 1);
    assert.equal(runtime.inactivity.at(-1), 180_000);
  }
});

test("buffering preserves separate cookies, security and cache headers", async () => {
  const { runtime, coordinator } = setup();
  runtime.handler = async () => {
    const headers = new Headers({ "Content-Security-Policy": "default-src 'self'", "Cache-Control": "no-store", "Content-Type": "application/json" });
    headers.append("Set-Cookie", "session=synthetic; Secure; HttpOnly");
    headers.append("Set-Cookie", "old=; Max-Age=0; Secure; HttpOnly");
    return new Response('{"saved":true}', { status: 201, headers });
  };
  const req = request();
  const response = await coordinator.forward(req, () => prepare(req));
  assert.equal(response.status, 201);
  assert.deepEqual(response.headers.getSetCookie(), ["session=synthetic; Secure; HttpOnly", "old=; Max-Age=0; Secure; HttpOnly"]);
  assert.equal(response.headers.get("Content-Security-Policy"), "default-src 'self'");
  assert.equal(response.headers.get("Cache-Control"), "no-store");
  assert.equal(await response.text(), '{"saved":true}');
});

test("queued requests remain unread and unlocked until the prior response completes", async () => {
  const { runtime, coordinator } = setup();
  let finish!: () => void;
  runtime.handler = async (_req) => runtime.requests.length === 1 ? new Response(new ReadableStream({ start(controller) { finish = () => controller.close(); } })) : new Response("second");
  const firstReq = request("/api/first");
  const first = coordinator.forward(firstReq, () => prepare(firstReq));
  await flush();
  const source = lazyBody();
  const secondReq = request("/api/import", source.body);
  let prepared = 0;
  const second = coordinator.forward(secondReq, () => { prepared++; return prepare(secondReq); });
  await flush();
  assert.equal(prepared, 0);
  assert.equal(source.pulls(), 0);
  assert.equal(source.body.locked, false);
  assert.equal(runtime.requests.length, 1);
  finish();
  await first;
  await second;
  assert.equal(prepared, 1);
  assert.equal(runtime.requests.length, 2);
});

test("sixteen waiting 32 MiB streams and saturation do not pre-read their bodies", async () => {
  const { runtime, coordinator } = setup();
  const active = deferred<Response>();
  runtime.handler = () => runtime.requests.length === 1 ? active.promise : Promise.resolve(new Response("ok"));
  const firstReq = request("/api/active");
  const first = coordinator.forward(firstReq, () => prepare(firstReq));
  await flush();
  const sources = Array.from({ length: 17 }, () => lazyBody());
  let prepared = 0;
  const waiting = sources.map((source, i) => {
    const req = request(`/api/import?waiting=${i}`, source.body);
    return coordinator.forward(req, () => { prepared++; return prepare(req); });
  });
  await flush();
  await failure(await waiting[16], "unavailable");
  assert.equal(prepared, 0);
  assert.ok(sources.every((source) => source.pulls() === 0 && source.body.locked === false));
  active.resolve(new Response("first"));
  await first;
  await Promise.all(waiting.slice(0, 16));
  assert.equal(prepared, 16);
  assert.equal(runtime.requests.length, 17);
  assert.equal(sources[16].pulls(), 0);
  assert.equal(sources[16].body.locked, false);
});

test("a waiting request expires after 30 seconds without preparing, reading or later sending", async () => {
  const { runtime, coordinator, clock } = setup();
  const active = deferred<Response>();
  runtime.handler = () => active.promise;
  const firstReq = request();
  const first = coordinator.forward(firstReq, () => prepare(firstReq));
  await flush();
  const source = lazyBody();
  const req = request("/api/import", source.body);
  let prepared = 0;
  const waiting = coordinator.forward(req, () => { prepared++; return prepare(req); });
  await clock.advance(30_000);
  await failure(await waiting, "unavailable");
  assert.equal(prepared, 0);
  assert.equal(source.pulls(), 0);
  assert.equal(source.body.locked, false);
  active.resolve(new Response("known result"));
  await first;
  assert.equal(runtime.requests.length, 1);
  assert.equal(clock.count(), 0);
});

test("queued client cancellation removes the waiter without dispatch or body acquisition", async () => {
  const { runtime, coordinator } = setup();
  const active = deferred<Response>();
  runtime.handler = () => active.promise;
  const firstReq = request();
  const first = coordinator.forward(firstReq, () => prepare(firstReq));
  await flush();
  const source = lazyBody();
  const abort = new AbortController();
  const req = request("/api/import", source.body, abort.signal);
  let prepared = 0;
  const waiting = coordinator.forward(req, () => { prepared++; return prepare(req); });
  abort.abort(new Error("SQL-SENSITIVE"));
  await failure(await waiting, "unavailable");
  assert.equal(prepared, 0);
  assert.equal(source.pulls(), 0);
  assert.equal(source.body.locked, false);
  active.resolve(new Response("ok"));
  await first;
  assert.equal(runtime.requests.length, 1);
});

test("known injected boundary failures are preserved but arbitrary error-shaped objects are not trusted", async () => {
  class KnownBoundaryFailure extends Error {}
  const known = (error: unknown) => error instanceof KnownBoundaryFailure ? new Response('{"code":"payload_too_large"}', { status: 413, headers: { "Cache-Control": "no-store" } }) : undefined;
  for (const error of [new KnownBoundaryFailure(), { status: 413, code: "payload_too_large", message: "SQL-SENSITIVE" }]) {
    const { runtime, coordinator } = setup();
    runtime.handler = async () => { throw error; };
    const req = request();
    const result = await coordinator.forward(req, () => prepare(req), known);
    if (error instanceof KnownBoundaryFailure) assert.equal(result.status, 413);
    else await failure(result, "outcome_unknown");
    assert.equal(runtime.requests.length, 1);
  }
});

test("prepare failures occur before forwarding, remain private and allow later requests", async () => {
  const { runtime, coordinator } = setup();
  await failure(await coordinator.forward(request(), () => { throw new Error("SQL-SENSITIVE synthetic-control-value"); }), "unavailable");
  assert.equal(runtime.requests.length, 0);
  assert.equal(runtime.inactivity.at(-1), 60_000);
  const req = request();
  assert.equal((await coordinator.forward(req, () => prepare(req))).status, 200);
  assert.equal(runtime.requests.length, 1);
});

test("active client cancellation reports unknown without retry and cleans the deadline", async () => {
  const { runtime, coordinator, clock } = setup();
  runtime.handler = async () => new Promise(() => {});
  const abort = new AbortController();
  const req = request("/api/update", undefined, abort.signal);
  const result = coordinator.forward(req, () => prepare(req));
  await flush();
  assert.equal(runtime.requests.length, 1);
  abort.abort(new Error("SQL-SENSITIVE"));
  await failure(await result, "outcome_unknown");
  assert.equal(runtime.requests.length, 1);
  assert.equal(runtime.signals[0].aborted, true);
  assert.equal(clock.count(), 0);
});

test("idle inactivity update is awaited within the forward deadline", async () => {
  const { runtime, coordinator, clock } = setup();
  const idle = deferred<void>();
  runtime.setInactivityTimeout = async (duration: number | bigint) => {
    runtime.inactivity.push(duration);
    if (duration === 60_000) await idle.promise;
  };
  const req = request();
  const result = coordinator.forward(req, () => prepare(req));
  await flush();
  assert.equal(runtime.inactivity.at(-1), 60_000);
  await clock.advance(180_000);
  await failure(await result, "outcome_unknown");
  idle.resolve();
  await flush();
  assert.equal(runtime.requests.length, 1);
});

test("the default clock drives an actual successful runtime roundtrip", async () => {
  const runtime = new FakeRuntime();
  const coordinator = new ContainerCoordinator(runtime, environment(), ORIGIN);
  const req = request();
  const response = await coordinator.forward(req, () => prepare(req));
  assert.equal(await response.text(), "done");
  assert.equal(runtime.starts.length, 1);
  assert.equal(runtime.inactivity.at(-1), 60_000);
});

test("response loss is outcome_unknown and does not restart or retry the runtime", async () => {
  const { runtime, coordinator } = setup();
  runtime.handler = async () => { throw new Error("SQL-SENSITIVE https://provider.invalid/?secret-query=synthetic-control-value"); };
  const req = request();
  await failure(await coordinator.forward(req, () => prepare(req)), "outcome_unknown");
  assert.equal(runtime.requests.length, 1);
  assert.equal(runtime.starts.length, 1);
  assert.equal(runtime.inactivity.at(-1), 180_000);
});

test("an externally stopped runtime starts again with the same explicit configuration", async () => {
  const { runtime, coordinator } = setup();
  const req = request();
  await coordinator.forward(req, () => prepare(req));
  runtime.running = false;
  await coordinator.forward(req, () => prepare(req));
  assert.equal(runtime.starts.length, 2);
  assert.equal(runtime.health.length, 2);
  assert.deepEqual(runtime.starts[0], runtime.starts[1]);
});
