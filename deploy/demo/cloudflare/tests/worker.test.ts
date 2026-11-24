import assert from "node:assert/strict";
import test from "node:test";
import type {
  CacheLike,
  DemoClock,
  DemoExecutionContext,
  DemoWorkerEnv,
  RateLimitBinding,
  TimerHandle,
} from "../src/contracts.ts";
import { createDemoWorker } from "../src/worker.ts";

const DOMAIN = "demo.example.invalid";

class FakeClock implements DemoClock {
  private currentMs: number;
  private nextId = 1;
  private readonly timers = new Map<number, { runAt: number; callback: () => void }>();

  constructor(startMs = 1_700_000_000_000) {
    this.currentMs = startMs;
  }

  now(): number {
    return this.currentMs;
  }

  setTimeout(callback: () => void, delayMs: number): TimerHandle {
    const id = this.nextId++;
    this.timers.set(id, { runAt: this.currentMs + delayMs, callback });
    return id as unknown as TimerHandle;
  }

  clearTimeout(handle: TimerHandle): void {
    this.timers.delete(handle as unknown as number);
  }

  advance(ms: number): void {
    this.currentMs += ms;
    while (true) {
      let earliestId: number | undefined;
      let earliestAt = Number.POSITIVE_INFINITY;
      for (const [id, entry] of this.timers.entries()) {
        if (entry.runAt <= this.currentMs && entry.runAt < earliestAt) {
          earliestAt = entry.runAt;
          earliestId = id;
        }
      }
      if (earliestId === undefined) break;
      const entry = this.timers.get(earliestId);
      this.timers.delete(earliestId);
      entry?.callback();
    }
  }
}

class MemoryCache implements CacheLike {
  readonly entries = new Map<string, { status: number; statusText: string; headers: [string, string][]; body: Uint8Array }>();
  matchCalls: string[] = [];
  putCalls: { key: string; cacheControl: string | null }[] = [];

  private keyOf(input: RequestInfo | URL): string {
    if (typeof input === "string") return input;
    if (input instanceof URL) return input.toString();
    return `${input.method}:${input.url}`;
  }

  async match(request: RequestInfo | URL): Promise<Response | undefined> {
    const key = this.keyOf(request);
    this.matchCalls.push(key);
    const saved = this.entries.get(key);
    if (!saved) return undefined;
    return new Response(saved.body.slice(), {
      status: saved.status,
      statusText: saved.statusText,
      headers: saved.headers,
    });
  }

  async put(request: RequestInfo | URL, response: Response): Promise<void> {
    const key = this.keyOf(request);
    const headers: [string, string][] = [];
    response.headers.forEach((value, name) => headers.push([name, value]));
    const body = new Uint8Array(await response.arrayBuffer());
    this.entries.set(key, { status: response.status, statusText: response.statusText, headers, body });
    this.putCalls.push({ key, cacheControl: response.headers.get("Cache-Control") });
  }
}

function createCountingLimiter(maxAllowed: number): RateLimitBinding & { keys: string[] } {
  const counts = new Map<string, number>();
  const keys: string[] = [];
  return {
    keys,
    async limit({ key }) {
      keys.push(key);
      const next = (counts.get(key) ?? 0) + 1;
      counts.set(key, next);
      return { success: next <= maxAllowed };
    },
  };
}

function createWaitUntilCollector(): { ctx: DemoExecutionContext; flush(): Promise<void> } {
  const tasks: Promise<unknown>[] = [];
  return {
    ctx: {
      waitUntil(promise) {
        tasks.push(promise);
      },
    },
    async flush() {
      await Promise.all(tasks.splice(0));
    },
  };
}

function createHarness(options?: {
  demoEnabled?: string;
  domain?: string;
  limits?: { enter?: number; post?: number; get?: number };
  onContainerFetch?: (request: Request) => Promise<Response> | Response;
  clock?: FakeClock;
  bodyReadTimeoutMs?: number;
}) {
  const cache = new MemoryCache();
  const clock = options?.clock ?? new FakeClock();
  const enterLimiter = createCountingLimiter(options?.limits?.enter ?? 10);
  const postLimiter = createCountingLimiter(options?.limits?.post ?? 60);
  const getLimiter = createCountingLimiter(options?.limits?.get ?? 120);
  const doNames: string[] = [];
  const containerRequests: { url: string; method: string; headers: Headers; body: Uint8Array }[] = [];

  const env: DemoWorkerEnv = {
    MEIBO_DOMAIN: options?.domain ?? DOMAIN,
    DEMO_ENABLED: options?.demoEnabled ?? "true",
    MEIBO_DEMO_CONTACT: "",
    RATE_LIMIT_DEMO_ENTER: enterLimiter,
    RATE_LIMIT_POST: postLimiter,
    RATE_LIMIT_GET: getLimiter,
    DEMO_CONTAINER: {
      getByName(name) {
        doNames.push(name);
        return {
          async fetch(request) {
            const body = new Uint8Array(await request.clone().arrayBuffer());
            containerRequests.push({
              url: request.url,
              method: request.method,
              headers: new Headers(request.headers),
              body,
            });
            if (options?.onContainerFetch) {
              return await options.onContainerFetch(request);
            }
            return new Response("ok", {
              status: 200,
              headers: {
                "Content-Type": "text/plain; charset=utf-8",
                "Cache-Control": "no-store",
                "Content-Security-Policy": "default-src 'self'",
              },
            });
          },
        };
      },
    },
  };

  const worker = createDemoWorker({
    cache,
    clock,
    config: options?.bodyReadTimeoutMs !== undefined
      ? { bodyReadTimeoutMs: options.bodyReadTimeoutMs }
      : undefined,
  });

  return { worker, env, cache, clock, enterLimiter, postLimiter, getLimiter, doNames, containerRequests };
}

function makeRequest(
  path: string,
  init: RequestInit & { duplex?: "half" } = {},
  domain = DOMAIN,
): Request {
  const headers = new Headers(init.headers);
  if (!headers.has("Host")) {
    headers.set("Host", domain);
  }
  return new Request(`https://${domain}${path}`, { ...init, headers });
}

test("DEMO_ENABLED が true 以外ならコンテナ・キャッシュ・レート制限に触れず 503 を返す", async () => {
  const harness = createHarness({ demoEnabled: "false" });
  const { ctx, flush } = createWaitUntilCollector();

  // 事前にキャッシュが入っていても停止切替を最優先する
  await harness.cache.put(
    new Request(`https://${DOMAIN}/login`),
    new Response("cached", { headers: { "Cache-Control": "public, max-age=60" } }),
  );

  const htmlRes = await harness.worker.fetch(
    makeRequest("/login", { headers: { Accept: "text/html" } }),
    harness.env,
    ctx,
  );
  assert.equal(htmlRes.status, 503);
  assert.equal(htmlRes.headers.get("Retry-After"), "300");
  assert.equal(htmlRes.headers.get("Content-Type"), "text/html; charset=utf-8");
  assert.match(await htmlRes.text(), /メンテナンス中です。しばらくしてからお試しください。/);

  const jsonRes = await harness.worker.fetch(
    makeRequest("/api/companies", { headers: { Accept: "application/json" } }),
    harness.env,
    ctx,
  );
  assert.equal(jsonRes.status, 503);
  assert.equal(jsonRes.headers.get("Retry-After"), "300");
  assert.equal(jsonRes.headers.get("Content-Type"), "application/json; charset=utf-8");
  const jsonBody = await jsonRes.json() as { error: string; code: string };
  assert.equal(jsonBody.error, "メンテナンス中です。しばらくしてからお試しください。");
  assert.equal(jsonBody.code, "maintenance");

  await flush();
  assert.equal(harness.doNames.length, 0);
  assert.equal(harness.cache.matchCalls.length, 0);
  assert.equal(harness.getLimiter.keys.length, 0);
});

test("GET と POST 以外のメソッドは 405 を返しコンテナに触れない", async () => {
  const harness = createHarness();
  for (const method of ["PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"]) {
    const res = await harness.worker.fetch(makeRequest("/api/companies", { method }), harness.env);
    assert.equal(res.status, 405, `${method} は 405`);
    assert.equal(res.headers.get("Allow"), "GET, POST");
  }
  assert.equal(harness.doNames.length, 0);
});

test("公開ドメインと一致しない Host は 403 で拒否しコンテナに触れない", async () => {
  const harness = createHarness();
  const res = await harness.worker.fetch(
    new Request("https://evil.invalid/login", { headers: { Host: "evil.invalid" } }),
    harness.env,
  );
  assert.equal(res.status, 403);
  assert.equal(harness.doNames.length, 0);
});

test("POST の Content-Length 欠如・Transfer-Encoding・64KiB 超過を 411/413 で拒否しコンテナへ送らない", async () => {
  const harness = createHarness();

  // Content-Length なし -> 411
  const noLen = await harness.worker.fetch(
    makeRequest("/demo/enter", { method: "POST", body: "{}" }),
    harness.env,
  );
  assert.equal(noLen.status, 411);

  // Transfer-Encoding あり（Content-Length 併用含む）-> 411
  const chunked = await harness.worker.fetch(
    makeRequest("/demo/enter", {
      method: "POST",
      headers: { "Transfer-Encoding": "chunked", "Content-Length": "2" },
      body: "{}",
    }),
    harness.env,
  );
  assert.equal(chunked.status, 411);

  // 64KiB + 1 -> 413
  const oversizedHeader = await harness.worker.fetch(
    makeRequest("/api/companies", {
      method: "POST",
      headers: { "Content-Length": String(65536 + 1) },
      body: "a".repeat(65537),
    }),
    harness.env,
  );
  assert.equal(oversizedHeader.status, 413);

  // Content-Length を小さく偽装しつつ 64KiB 超を流す -> 413
  const lyingStream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new Uint8Array(65537));
      controller.close();
    },
  });
  const oversizedStream = await harness.worker.fetch(
    makeRequest("/api/companies", {
      method: "POST",
      headers: { "Content-Length": "100" },
      body: lyingStream,
      duplex: "half",
    }),
    harness.env,
  );
  assert.equal(oversizedStream.status, 413);

  assert.equal(harness.doNames.length, 0);
  assert.equal(harness.containerRequests.length, 0);
});

test("POST の本文を Worker が最後まで読み切ってから初めてコンテナへ渡す", async () => {
  const harness = createHarness();
  let streamController: ReadableStreamDefaultController<Uint8Array> | undefined;
  const bodyStream = new ReadableStream<Uint8Array>({
    start(controller) {
      streamController = controller;
    },
  });

  const payload = "{\"hello\":\"demo\"}";
  const bytes = new TextEncoder().encode(payload);

  const pending = harness.worker.fetch(
    makeRequest("/demo/enter", {
      method: "POST",
      headers: {
        "Content-Length": String(bytes.byteLength),
        "Content-Type": "application/json",
        Origin: `https://${DOMAIN}`,
        "X-Meibo-Local": "1",
      },
      body: bodyStream,
      duplex: "half",
    }),
    harness.env,
  );

  // 最初の断片だけ流した時点では、まだ DO / コンテナへ一切届いていない
  streamController!.enqueue(bytes.subarray(0, 6));
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(harness.doNames.length, 0);
  assert.equal(harness.containerRequests.length, 0);

  // 残りを流して閉じると、完全な本文と Content-Length 付きで転送される
  streamController!.enqueue(bytes.subarray(6));
  streamController!.close();

  const res = await pending;
  assert.equal(res.status, 200);
  assert.equal(harness.doNames.length, 1);
  assert.equal(harness.containerRequests.length, 1);
  assert.equal(new TextDecoder().decode(harness.containerRequests[0]!.body), payload);
  assert.equal(harness.containerRequests[0]!.headers.get("Content-Length"), String(bytes.byteLength));
});

test("POST の本文がちょうど 64KiB および 0 バイトのときは正常に転送する", async () => {
  const harness = createHarness();

  const exactLimit = new Uint8Array(65536).fill(0x61);
  const resMax = await harness.worker.fetch(
    makeRequest("/api/companies", {
      method: "POST",
      headers: { "Content-Length": "65536", "Content-Type": "application/json" },
      body: exactLimit,
    }),
    harness.env,
  );
  assert.equal(resMax.status, 200);
  assert.equal(harness.containerRequests[0]!.body.byteLength, 65536);

  const resEmpty = await harness.worker.fetch(
    makeRequest("/demo/enter", {
      method: "POST",
      headers: { "Content-Length": "0" },
    }),
    harness.env,
  );
  assert.equal(resEmpty.status, 200);
  assert.equal(harness.containerRequests[1]!.body.byteLength, 0);
  assert.equal(harness.containerRequests[1]!.headers.get("Content-Length"), "0");
});

test("POST の本文が途中で切れた・エラーになった・遅すぎるときは 400/408 で終えコンテナへ送らない", async () => {
  const clock = new FakeClock();
  const harness = createHarness({ clock, bodyReadTimeoutMs: 5_000 });

  // 宣言長より短く閉じた -> 400
  const truncatedStream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("abc"));
      controller.close();
    },
  });
  const resTruncated = await harness.worker.fetch(
    makeRequest("/demo/enter", {
      method: "POST",
      headers: { "Content-Length": "10" },
      body: truncatedStream,
      duplex: "half",
    }),
    harness.env,
  );
  assert.equal(resTruncated.status, 400);

  // ストリームが途中で異常終了 -> 400
  const brokenStream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("ab"));
      controller.error(new Error("connection reset"));
    },
  });
  const resBroken = await harness.worker.fetch(
    makeRequest("/demo/enter", {
      method: "POST",
      headers: { "Content-Length": "10" },
      body: brokenStream,
      duplex: "half",
    }),
    harness.env,
  );
  assert.equal(resBroken.status, 400);

  // 遅い送信（タイムアウト超過） -> 408
  const slowStream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("a"));
      // 以降ずっと送らない
    },
  });
  const slowPromise = harness.worker.fetch(
    makeRequest("/demo/enter", {
      method: "POST",
      headers: { "Content-Length": "10" },
      body: slowStream,
      duplex: "half",
    }),
    harness.env,
  );
  await Promise.resolve();
  clock.advance(5_000);
  const resSlow = await slowPromise;
  assert.equal(resSlow.status, 408);

  assert.equal(harness.doNames.length, 0);
  assert.equal(harness.containerRequests.length, 0);
});

test("レート制限超過で 429 と Retry-After を返し、制限された要求はコンテナへ届かない", async () => {
  const harness = createHarness({ limits: { enter: 2, post: 1, get: 2 } });

  // POST /demo/enter: 2 回まで許可、3 回目は 429
  for (let i = 0; i < 2; i++) {
    const ok = await harness.worker.fetch(
      makeRequest("/demo/enter", {
        method: "POST",
        headers: { "Content-Length": "2", "CF-Connecting-IP": "198.51.100.10" },
        body: "{}",
      }),
      harness.env,
    );
    assert.equal(ok.status, 200);
  }
  const enterBlocked = await harness.worker.fetch(
    makeRequest("/demo/enter", {
      method: "POST",
      headers: { "Content-Length": "2", "CF-Connecting-IP": "198.51.100.10" },
      body: "{}",
    }),
    harness.env,
  );
  assert.equal(enterBlocked.status, 429);
  assert.equal(enterBlocked.headers.get("Retry-After"), "60");

  // 別 IP は独立して通る
  const otherIpEnter = await harness.worker.fetch(
    makeRequest("/demo/enter", {
      method: "POST",
      headers: { "Content-Length": "2", "CF-Connecting-IP": "198.51.100.11" },
      body: "{}",
    }),
    harness.env,
  );
  assert.equal(otherIpEnter.status, 200);

  // そのほかの POST: 1 回まで許可、2 回目は 429
  const postOk = await harness.worker.fetch(
    makeRequest("/api/companies", {
      method: "POST",
      headers: { "Content-Length": "2", "CF-Connecting-IP": "198.51.100.10" },
      body: "{}",
    }),
    harness.env,
  );
  assert.equal(postOk.status, 200);
  const postBlocked = await harness.worker.fetch(
    makeRequest("/api/companies", {
      method: "POST",
      headers: { "Content-Length": "2", "CF-Connecting-IP": "198.51.100.10" },
      body: "{}",
    }),
    harness.env,
  );
  assert.equal(postBlocked.status, 429);

  // コンテナへ届く GET: 2 回まで許可、3 回目は 429
  for (let i = 0; i < 2; i++) {
    const getOk = await harness.worker.fetch(
      makeRequest("/api/companies", { headers: { "CF-Connecting-IP": "198.51.100.10" } }),
      harness.env,
    );
    assert.equal(getOk.status, 200);
  }
  const getBlocked = await harness.worker.fetch(
    makeRequest("/api/companies", { headers: { "CF-Connecting-IP": "198.51.100.10" } }),
    harness.env,
  );
  assert.equal(getBlocked.status, 429);

  // コンテナに届いたのは許可された 6 件だけ（2 + 1 + 1 + 2）
  assert.equal(harness.containerRequests.length, 6);
  assert.deepEqual(harness.enterLimiter.keys, [
    "enter:198.51.100.10",
    "enter:198.51.100.10",
    "enter:198.51.100.10",
    "enter:198.51.100.11",
  ]);
});

test("静的経路は Cache-Control を付け直して保持し、2 回目はコンテナを呼ばずクエリも無視する", async () => {
  const harness = createHarness({
    onContainerFetch: (request) => {
      const { pathname } = new URL(request.url);
      return new Response(`asset:${pathname}`, {
        status: 200,
        headers: {
          "Content-Type": pathname.endsWith(".png") ? "image/png" : "text/plain; charset=utf-8",
          "Cache-Control": "no-store",
          "Content-Security-Policy": "default-src 'self'",
        },
      });
    },
  });
  const { ctx, flush } = createWaitUntilCollector();

  const expectedTtls: [string, number][] = [
    ["/login", 300],
    ["/login.css", 3600],
    ["/login.js", 3600],
    ["/public-entry.js", 3600],
    ["/public-entry.png", 3600],
  ];

  for (const [path, ttl] of expectedTtls) {
    const first = await harness.worker.fetch(makeRequest(`${path}?reset=1&v=1`), harness.env, ctx);
    await flush();
    assert.equal(first.status, 200);
    assert.equal(first.headers.get("Cache-Control"), `public, max-age=${ttl}`);
    assert.equal(first.headers.get("Content-Security-Policy"), "default-src 'self'");
    assert.equal(await first.text(), `asset:${path}`);

    // キャッシュキーに公開ドメインを含み、クエリを含まない
    assert.equal(harness.cache.entries.has(`GET:https://${DOMAIN}${path}`), true);
  }

  assert.equal(harness.containerRequests.length, expectedTtls.length);

  // 別クエリでの 2 回目はすべてキャッシュから返り、コンテナも GET レート制限も呼ばれない
  const getCallsBefore = harness.getLimiter.keys.length;
  for (const [path, ttl] of expectedTtls) {
    const second = await harness.worker.fetch(makeRequest(`${path}?utm_source=video`), harness.env, ctx);
    assert.equal(second.status, 200);
    assert.equal(second.headers.get("Cache-Control"), `public, max-age=${ttl}`);
    assert.equal(await second.text(), `asset:${path}`);
  }
  assert.equal(harness.containerRequests.length, expectedTtls.length);
  assert.equal(harness.getLimiter.keys.length, getCallsBefore);
});

test("Set-Cookie を含む応答・非 200 応答・保持対象外の経路はキャッシュしない", async () => {
  const harness = createHarness({
    onContainerFetch: (request) => {
      const { pathname } = new URL(request.url);
      if (pathname === "/login") {
        return new Response("login-with-cookie", {
          status: 200,
          headers: [
            ["Cache-Control", "no-store"],
            ["Set-Cookie", "__Host-meibo-session=synthetic; Secure; HttpOnly; SameSite=Lax"],
          ],
        });
      }
      if (pathname === "/login.css") {
        return new Response("starting", { status: 503, headers: { "Cache-Control": "no-store" } });
      }
      return new Response("dynamic", {
        status: 200,
        headers: { "Cache-Control": "no-store" },
      });
    },
  });
  const { ctx, flush } = createWaitUntilCollector();

  const withCookie = await harness.worker.fetch(makeRequest("/login"), harness.env, ctx);
  await flush();
  assert.equal(withCookie.status, 200);
  assert.equal(withCookie.headers.get("Cache-Control"), "no-store");
  assert.equal(withCookie.headers.getSetCookie().length, 1);

  const errorAsset = await harness.worker.fetch(makeRequest("/login.css"), harness.env, ctx);
  await flush();
  assert.equal(errorAsset.status, 503);

  const dynamicRoot = await harness.worker.fetch(makeRequest("/"), harness.env, ctx);
  const dynamicApi = await harness.worker.fetch(makeRequest("/api/companies"), harness.env, ctx);
  await flush();
  assert.equal(dynamicRoot.headers.get("Cache-Control"), "no-store");
  assert.equal(dynamicApi.headers.get("Cache-Control"), "no-store");

  assert.equal(harness.cache.putCalls.length, 0);
  assert.equal(harness.cache.entries.size, 0);
});

test("転送時に必要なヘッダーと応答ヘッダーを保ち、CF-* と X-Forwarded-* を取り除く", async () => {
  const harness = createHarness({
    onContainerFetch: () => new Response("created", {
      status: 201,
      headers: [
        ["Content-Type", "application/json; charset=utf-8"],
        ["Content-Security-Policy", "default-src 'self'; frame-ancestors 'none'"],
        ["X-Content-Type-Options", "nosniff"],
        ["Cache-Control", "no-store"],
        ["Set-Cookie", "__Host-meibo-session=synthetic-a; Path=/; Secure; HttpOnly"],
        ["Set-Cookie", "demo-hint=1; Path=/; Secure"],
      ],
    }),
  });

  const response = await harness.worker.fetch(
    makeRequest("/api/companies?q=demo", {
      method: "POST",
      body: "{\"name\":\"合成・株式会社テスト\"}",
      headers: {
        "Content-Length": String(new TextEncoder().encode("{\"name\":\"合成・株式会社テスト\"}").byteLength),
        "Content-Type": "application/json; charset=utf-8",
        Origin: `https://${DOMAIN}`,
        Cookie: "__Host-meibo-session=synthetic-session",
        "X-Meibo-Local": "1",
        "X-Meibo-Context": "demo:demo",
        "CF-Connecting-IP": "198.51.100.20",
        "CF-Ray": "synthetic-ray",
        "CF-IPCountry": "JP",
        "X-Forwarded-For": "198.51.100.20",
        "X-Forwarded-Host": "spoofed.invalid",
        "X-Forwarded-Proto": "http",
        Forwarded: "for=198.51.100.20",
        Expect: "100-continue",
      },
    }),
    harness.env,
  );

  assert.equal(response.status, 201);
  assert.equal(await response.text(), "created");
  assert.equal(response.headers.get("Content-Security-Policy"), "default-src 'self'; frame-ancestors 'none'");
  assert.equal(response.headers.get("X-Content-Type-Options"), "nosniff");
  assert.equal(response.headers.getSetCookie().length, 2);

  assert.deepEqual(harness.doNames, ["meibo-demo-v1"]);
  const forwarded = harness.containerRequests[0]!;
  assert.equal(new URL(forwarded.url).host, DOMAIN);
  assert.equal(forwarded.headers.get("Host"), DOMAIN);
  assert.equal(forwarded.headers.get("Origin"), `https://${DOMAIN}`);
  assert.equal(forwarded.headers.get("Cookie"), "__Host-meibo-session=synthetic-session");
  assert.equal(forwarded.headers.get("Content-Type"), "application/json; charset=utf-8");
  assert.equal(forwarded.headers.get("X-Meibo-Local"), "1");
  assert.equal(forwarded.headers.get("X-Meibo-Context"), "demo:demo");

  for (const forbidden of [
    "CF-Connecting-IP",
    "CF-Ray",
    "CF-IPCountry",
    "X-Forwarded-For",
    "X-Forwarded-Host",
    "X-Forwarded-Proto",
    "Forwarded",
    "Expect",
  ]) {
    assert.equal(forwarded.headers.get(forbidden), null, `${forbidden} は転送しない`);
  }
});

test("MEIBO_DOMAIN が未設定または不正なときと DO 例外時は詳細を漏らさず 503 を返す", async () => {
  for (const invalidDomain of ["", "https://demo.example.invalid", "demo.example.invalid/path", "DEMO.EXAMPLE.INVALID"]) {
    const harness = createHarness({ domain: invalidDomain });
    const res = await harness.worker.fetch(makeRequest("/login"), harness.env);
    assert.equal(res.status, 503, `不正なドメイン "${invalidDomain}" は 503`);
    assert.equal(harness.doNames.length, 0);
  }

  const failingHarness = createHarness({
    onContainerFetch: () => {
      throw new Error("internal-secret-stack-detail");
    },
  });
  const errRes = await failingHarness.worker.fetch(
    makeRequest("/api/companies", { headers: { Accept: "application/json" } }),
    failingHarness.env,
  );
  assert.equal(errRes.status, 503);
  const text = await errRes.text();
  assert.equal(text.includes("internal-secret-stack-detail"), false);
});

test("不正な Content-Length 形式や宣言長超過の POST は 400 で拒否しコンテナへ送らない", async () => {
  const harness = createHarness();

  for (const badLength of ["abc", "-1", "10, 20"]) {
    const res = await harness.worker.fetch(
      makeRequest("/demo/enter", {
        method: "POST",
        headers: { "Content-Length": badLength },
        body: "{}",
      }),
      harness.env,
    );
    assert.equal(res.status, 400, `Content-Length: ${badLength} は 400`);
  }

  // 宣言長（5 バイト）より実本文（12 バイト、64KiB 以内）が多い -> 400
  const excessStream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("123456789012"));
      controller.close();
    },
  });
  const resExcess = await harness.worker.fetch(
    makeRequest("/demo/enter", {
      method: "POST",
      headers: { "Content-Length": "5" },
      body: excessStream,
      duplex: "half",
    }),
    harness.env,
  );
  assert.equal(resExcess.status, 400);
  assert.equal(harness.doNames.length, 0);
});
