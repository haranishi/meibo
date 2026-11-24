import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import test from "node:test";

// cloudflare:workers の基底クラスだけを差し替え、実際の src/index.ts と DemoContainer を結合試験する。
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier !== "cloudflare:workers") return nextResolve(specifier, context);
    return {
      url: `data:text/javascript,${encodeURIComponent("export class DurableObject { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }")}`,
      shortCircuit: true,
    };
  },
});
const { default: worker, DemoContainer } = await import("../src/index.ts");
hooks.deregister();

const DOMAIN = "demo.example.invalid";

function createFakeRuntime() {
  let running = false;
  let resolveMonitor: (() => void) | undefined;
  const starts: unknown[] = [];
  const forwarded: { url: string; method: string; headers: Headers; body: string }[] = [];
  let destroyed = 0;

  const container = {
    get running() {
      return running;
    },
    start(options: unknown) {
      starts.push(options);
      running = true;
    },
    monitor() {
      return new Promise<void>((resolve) => {
        resolveMonitor = resolve;
      });
    },
    async destroy() {
      destroyed++;
      running = false;
      resolveMonitor?.();
      resolveMonitor = undefined;
    },
    async setInactivityTimeout() {},
    getTcpPort(port: number) {
      assert.equal(port, 4317);
      return {
        async fetch(request: Request) {
          const url = new URL(request.url);
          if (url.pathname === "/health") {
            return new Response(JSON.stringify({ ok: true }), { status: 200 });
          }
          forwarded.push({
            url: request.url,
            method: request.method,
            headers: new Headers(request.headers),
            body: await request.text(),
          });
          return new Response(JSON.stringify({ entered: true }), {
            status: 200,
            headers: [
              ["Content-Type", "application/json; charset=utf-8"],
              ["Cache-Control", "no-store"],
              ["Content-Security-Policy", "default-src 'self'"],
              ["Set-Cookie", "__Host-meibo-session=synthetic; Path=/; Secure; HttpOnly; SameSite=Lax"],
            ],
          });
        },
      };
    },
  };

  return {
    container,
    starts,
    forwarded,
    get destroyed() {
      return destroyed;
    },
  };
}

test("本番エントリ（src/index.ts と DemoContainer）の結合動作を確認する", async () => {
  const fake = createFakeRuntime();
  const alarms: number[] = [];
  const storage = {
    async setAlarm(time: number | Date) {
      alarms.push(typeof time === "number" ? time : time.getTime());
    },
    put() {
      throw new Error("DO 保存領域へ書き込んではならない");
    },
  };

  const allowAll = {
    async limit() {
      return { success: true };
    },
  };

  const doNames: string[] = [];
  const env = {
    MEIBO_DOMAIN: DOMAIN,
    DEMO_ENABLED: "true",
    MEIBO_DEMO_CONTACT: "",
    RATE_LIMIT_DEMO_ENTER: allowAll,
    RATE_LIMIT_POST: allowAll,
    RATE_LIMIT_GET: allowAll,
    DEMO_CONTAINER: {
      getByName(name: string) {
        doNames.push(name);
        return { fetch: (req: Request) => demoObject.fetch(req) };
      },
    },
  };

  const demoObject = new DemoContainer(
    { container: fake.container, storage } as never,
    env as never,
  );

  const waitUntilTasks: Promise<unknown>[] = [];
  const ctx = {
    waitUntil(promise: Promise<unknown>) {
      waitUntilTasks.push(promise);
    },
  };

  const body = "{}";
  const response = await worker.fetch(
    new Request(`https://${DOMAIN}/demo/enter`, {
      method: "POST",
      body,
      headers: {
        Host: DOMAIN,
        Origin: `https://${DOMAIN}`,
        "Content-Type": "application/json",
        "Content-Length": String(body.length),
        "X-Meibo-Local": "1",
        "CF-Connecting-IP": "198.51.100.50",
        "X-Forwarded-For": "198.51.100.50",
      },
    }),
    env as never,
    ctx as never,
  );
  await Promise.all(waitUntilTasks);

  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { entered: true });
  assert.equal(response.headers.get("Content-Security-Policy"), "default-src 'self'");
  assert.equal(response.headers.getSetCookie().length, 1);

  assert.deepEqual(doNames, ["meibo-demo-v1"]);
  assert.equal(fake.starts.length, 1);
  assert.equal(alarms.length, 1);

  const sent = fake.forwarded[0]!;
  assert.equal(sent.url, `http://${DOMAIN}/demo/enter`);
  assert.equal(sent.headers.get("Host"), DOMAIN);
  assert.equal(sent.headers.get("Origin"), `https://${DOMAIN}`);
  assert.equal(sent.headers.get("X-Meibo-Local"), "1");
  assert.equal(sent.headers.get("CF-Connecting-IP"), null);
  assert.equal(sent.headers.get("X-Forwarded-For"), null);
  assert.equal(sent.body, "{}");

  // container が無い DO 状態では 503 を返す
  const noContainerDo = new DemoContainer({ storage } as never, env as never);
  const unavailable = await noContainerDo.fetch(new Request(`https://${DOMAIN}/login`, { headers: { Host: DOMAIN } }));
  assert.equal(unavailable.status, 503);
});

test("キャッシュ済みの静的応答はコンテナ休止中でもコンテナを起こさず返す", async () => {
  const { createDemoWorker } = await import("../src/worker.ts");
  const fake = createFakeRuntime();
  const entries = new Map<string, { status: number; headers: [string, string][]; body: Uint8Array }>();
  const cache = {
    async match(req: RequestInfo | URL) {
      const key = req instanceof Request ? req.url : String(req);
      const saved = entries.get(key);
      if (!saved) return undefined;
      return new Response(saved.body.slice(), { status: saved.status, headers: saved.headers });
    },
    async put(req: RequestInfo | URL, res: Response) {
      const key = req instanceof Request ? req.url : String(req);
      const headers: [string, string][] = [];
      res.headers.forEach((v, k) => headers.push([k, v]));
      entries.set(key, { status: res.status, headers, body: new Uint8Array(await res.arrayBuffer()) });
    },
  };

  const allowAll = { async limit() { return { success: true }; } };
  const storage = { async setAlarm() {} };
  const env = {
    MEIBO_DOMAIN: DOMAIN,
    DEMO_ENABLED: "true",
    MEIBO_DEMO_CONTACT: "",
    RATE_LIMIT_DEMO_ENTER: allowAll,
    RATE_LIMIT_POST: allowAll,
    RATE_LIMIT_GET: allowAll,
    DEMO_CONTAINER: {
      getByName() {
        return { fetch: (req: Request) => demoObject.fetch(req) };
      },
    },
  };

  // Set-Cookie を返さない静的応答で検証するため /login 応答を差し替え
  const origGetTcpPort = fake.container.getTcpPort.bind(fake.container);
  fake.container.getTcpPort = (port: number) => {
    const inner = origGetTcpPort(port);
    return {
      async fetch(req: Request) {
        if (new URL(req.url).pathname === "/login") {
          return new Response("<html>demo login</html>", {
            status: 200,
            headers: {
              "Content-Type": "text/html; charset=utf-8",
              "Cache-Control": "no-store",
            },
          });
        }
        return inner.fetch(req);
      },
    };
  };

  const demoObject = new DemoContainer({ container: fake.container, storage } as never, env as never);
  const demoWorker = createDemoWorker({ cache });

  // 1 回目: コンテナを起動して /login を取得・キャッシュ
  const first = await demoWorker.fetch(new Request(`https://${DOMAIN}/login`, { headers: { Host: DOMAIN } }), env);
  assert.equal(first.status, 200);
  assert.equal(first.headers.get("Cache-Control"), "public, max-age=300");
  assert.equal(fake.starts.length, 1);
  assert.equal(fake.container.running, true);

  // コンテナを休止させる
  await fake.container.destroy();
  assert.equal(fake.container.running, false);

  // 2 回目: コンテナが休止中でもキャッシュから返し、コンテナを起こさない
  const second = await demoWorker.fetch(new Request(`https://${DOMAIN}/login?reset=1`, { headers: { Host: DOMAIN } }), env);
  assert.equal(second.status, 200);
  assert.equal(await second.text(), "<html>demo login</html>");
  assert.equal(fake.starts.length, 1);
  assert.equal(fake.container.running, false);
});
