import assert from "node:assert/strict";
import test from "node:test";
import type {
  AlarmStorage,
  ContainerRuntime,
  DemoClock,
  DemoContainerEnv,
  TimerHandle,
} from "../src/contracts.ts";
import { ALARM_REFRESH_SLACK_MS, DemoContainerController } from "../src/controller.ts";

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

interface FakeContainerOptions {
  healthResponses?: (() => Promise<Response> | Response)[];
  onAppFetch?: (request: Request) => Promise<Response> | Response;
}

function createFakeContainer(options: FakeContainerOptions = {}) {
  let running = false;
  let resolveMonitor: (() => void) | undefined;
  let rejectMonitor: ((reason?: unknown) => void) | undefined;
  let healthAttempt = 0;

  const starts: { env: Record<string, string>; enableInternet: false }[] = [];
  const inactivityTimeouts: (number | bigint)[] = [];
  const healthRequests: { url: string; host: string | null }[] = [];
  const appRequests: { url: string; method: string; headers: Headers; body: string }[] = [];
  let destroyCount = 0;
  let monitorCount = 0;

  const runtime: ContainerRuntime = {
    get running() {
      return running;
    },
    start(startOptions) {
      starts.push(startOptions);
      running = true;
    },
    monitor() {
      monitorCount++;
      return new Promise<void>((resolve, reject) => {
        resolveMonitor = resolve;
        rejectMonitor = reject;
      });
    },
    async destroy() {
      destroyCount++;
      running = false;
      const done = resolveMonitor;
      resolveMonitor = undefined;
      rejectMonitor = undefined;
      done?.();
    },
    async setInactivityTimeout(durationMs) {
      inactivityTimeouts.push(durationMs);
    },
    getTcpPort(port) {
      assert.equal(port, 4317);
      return {
        async fetch(input) {
          const req = input instanceof Request ? input : new Request(input);
          const url = new URL(req.url);
          if (url.pathname === "/health") {
            healthRequests.push({ url: req.url, host: req.headers.get("Host") });
            const handler = options.healthResponses?.[healthAttempt];
            healthAttempt++;
            if (handler) return await handler();
            return new Response(JSON.stringify({ ok: true }), { status: 200 });
          }
          appRequests.push({
            url: req.url,
            method: req.method,
            headers: new Headers(req.headers),
            body: await req.text(),
          });
          if (options.onAppFetch) {
            return await options.onAppFetch(req);
          }
          return new Response("demo-ok", {
            status: 200,
            headers: {
              "Content-Security-Policy": "default-src 'self'",
              "Cache-Control": "no-store",
            },
          });
        },
      };
    },
  };

  return {
    runtime,
    starts,
    inactivityTimeouts,
    healthRequests,
    appRequests,
    get destroyCount() {
      return destroyCount;
    },
    get monitorCount() {
      return monitorCount;
    },
    simulateExit(error?: Error) {
      running = false;
      const res = resolveMonitor;
      const rej = rejectMonitor;
      resolveMonitor = undefined;
      rejectMonitor = undefined;
      if (error) {
        rej?.(error);
      } else {
        res?.();
      }
    },
  };
}

function createFakeAlarmStorage(): AlarmStorage & { alarms: number[] } {
  const alarms: number[] = [];
  return {
    alarms,
    async setAlarm(scheduledTime) {
      alarms.push(typeof scheduledTime === "number" ? scheduledTime : scheduledTime.getTime());
    },
  };
}

function makeRequest(path: string, init: RequestInit = {}): Request {
  const headers = new Headers(init.headers);
  if (!headers.has("Host")) headers.set("Host", DOMAIN);
  return new Request(`https://${DOMAIN}${path}`, { ...init, headers });
}

test("同時要求でもコンテナ起動は 1 回だけ（単一飛行）で、許可した環境変数と enableInternet: false だけを渡す", async () => {
  const clock = new FakeClock();
  const fake = createFakeContainer({
    healthResponses: [
      () => Promise.reject(new Error("ECONNREFUSED")),
      () => new Response("not ready", { status: 503 }),
      () => new Response(JSON.stringify({ ok: true }), { status: 200 }),
    ],
  });
  const alarmStorage = createFakeAlarmStorage();
  const env: DemoContainerEnv & { UNRELATED_SECRET: string } = {
    MEIBO_DOMAIN: DOMAIN,
    MEIBO_DEMO_CONTACT: "https://example.invalid/contact",
    UNRELATED_SECRET: "must-not-pass",
  };
  const controller = new DemoContainerController(fake.runtime, alarmStorage, env, undefined, clock);

  const req1 = controller.handleFetch(makeRequest("/login"));
  const req2 = controller.handleFetch(makeRequest("/api/summary"));
  const req3 = controller.handleFetch(makeRequest("/login.css"));

  // /health が 3 回目で 200 になるまで時間を進める
  for (let i = 0; i < 4; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    clock.advance(200);
  }

  const responses = await Promise.all([req1, req2, req3]);
  assert.deepEqual(responses.map((r) => r.status), [200, 200, 200]);

  assert.equal(fake.starts.length, 1, "同時要求でも start() は 1 回だけ");
  assert.equal(fake.starts[0]!.enableInternet, false);
  assert.deepEqual(fake.starts[0]!.env, {
    MEIBO_DOMAIN: DOMAIN,
    PORT: "4317",
    MEIBO_DEMO_CONTACT: "https://example.invalid/contact",
  });
  assert.equal(fake.healthRequests[0]!.url, `http://${DOMAIN}/health`);
  assert.equal(fake.healthRequests[0]!.host, DOMAIN);
  assert.equal(fake.appRequests.length, 3);
  for (const forwarded of fake.appRequests) {
    assert.equal(new URL(forwarded.url).protocol, "http:");
    assert.equal(new URL(forwarded.url).host, DOMAIN);
    assert.equal(forwarded.headers.get("Host"), DOMAIN);
  }
});

test("MEIBO_DEMO_CONTACT が空のときはコンテナの環境変数へ渡さない", async () => {
  const clock = new FakeClock();
  const fake = createFakeContainer();
  const alarmStorage = createFakeAlarmStorage();
  const controller = new DemoContainerController(
    fake.runtime,
    alarmStorage,
    { MEIBO_DOMAIN: DOMAIN, MEIBO_DEMO_CONTACT: "   " },
    undefined,
    clock,
  );

  const res = await controller.handleFetch(makeRequest("/login"));
  assert.equal(res.status, 200);
  assert.deepEqual(fake.starts[0]!.env, {
    MEIBO_DOMAIN: DOMAIN,
    PORT: "4317",
  });
});

test("MEIBO_DEMO_CONTACT が不正な URL のときはコンテナを起動せず 503 を返す", async () => {
  const clock = new FakeClock();
  const fake = createFakeContainer();
  const alarmStorage = createFakeAlarmStorage();
  const controller = new DemoContainerController(
    fake.runtime,
    alarmStorage,
    { MEIBO_DOMAIN: DOMAIN, MEIBO_DEMO_CONTACT: "javascript:alert(1)" },
    undefined,
    clock,
  );

  const res = await controller.handleFetch(makeRequest("/login"));
  assert.equal(res.status, 503);
  assert.equal(fake.starts.length, 0);
});

test("/health が約 25 秒以内に 200 にならなければ 503 と Retry-After: 5 を返す", async () => {
  const clock = new FakeClock();
  const fake = createFakeContainer({
    healthResponses: Array.from({ length: 200 }, () => () => new Response("starting", { status: 503 })),
  });
  const alarmStorage = createFakeAlarmStorage();
  const controller = new DemoContainerController(
    fake.runtime,
    alarmStorage,
    { MEIBO_DOMAIN: DOMAIN },
    { startupTimeoutMs: 25_000, startupPollIntervalMs: 5_000 },
    clock,
  );

  const pending = controller.handleFetch(makeRequest("/login", { headers: { Accept: "text/html" } }));
  for (let i = 0; i < 10; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    clock.advance(5_000);
  }

  const res = await pending;
  assert.equal(res.status, 503);
  assert.equal(res.headers.get("Retry-After"), "5");
  assert.match(await res.text(), /起動中です。少し待ってからもう一度お試しください。/);
  assert.equal(fake.appRequests.length, 0);
});

test("コンテナ自己終了を monitor() で検知し、次の要求で新しく起動する", async () => {
  const clock = new FakeClock();
  const fake = createFakeContainer();
  const alarmStorage = createFakeAlarmStorage();
  const controller = new DemoContainerController(
    fake.runtime,
    alarmStorage,
    { MEIBO_DOMAIN: DOMAIN },
    undefined,
    clock,
  );

  const first = await controller.handleFetch(makeRequest("/login"));
  assert.equal(first.status, 200);
  assert.equal(fake.starts.length, 1);
  assert.equal(fake.monitorCount, 1);

  // アプリが自己終了（正常終了 / 異常終了の両パターンを確認）
  fake.simulateExit();
  await Promise.resolve();

  const second = await controller.handleFetch(makeRequest("/login"));
  assert.equal(second.status, 200);
  assert.equal(fake.starts.length, 2, "正常終了後の次回要求で再起動する");
  assert.equal(fake.monitorCount, 2);

  fake.simulateExit(new Error("exit code 70"));
  await Promise.resolve();

  const third = await controller.handleFetch(makeRequest("/login"));
  assert.equal(third.status, 200);
  assert.equal(fake.starts.length, 3, "異常終了後の次回要求でも再起動する");
});

test("アイドル停止アラームは、期限の延びが2分に満たない間は書き込まず、発火時に期限がまだ先なら設定し直し、期限到達で停止して次回要求で再起動する", async () => {
  const startAt = 1_700_000_000_000;
  const idleMs = 15 * 60 * 1000;
  const minute = 60 * 1000;
  const clock = new FakeClock(startAt);
  const fake = createFakeContainer();
  const alarmStorage = createFakeAlarmStorage();
  const controller = new DemoContainerController(
    fake.runtime,
    alarmStorage,
    { MEIBO_DOMAIN: DOMAIN },
    { idleTimeoutMs: idleMs },
    clock,
  );

  // 1 件目: アラーム未設定なので t0 + 15 分に 1 回だけ設定する
  await controller.handleFetch(makeRequest("/login"));
  assert.deepEqual(alarmStorage.alarms, [startAt + idleMs]);
  assert.equal(fake.inactivityTimeouts[0], idleMs);

  // 1 分後（期限の延びが ALARM_REFRESH_SLACK_MS 未満）: 連続した要求でも書き込まない
  clock.advance(minute);
  await controller.handleFetch(makeRequest("/api/summary"));
  await Promise.all([
    controller.handleFetch(makeRequest("/login")),
    controller.handleFetch(makeRequest("/login.css")),
  ]);
  assert.deepEqual(alarmStorage.alarms, [startAt + idleMs], "延びが2分に満たなければ setAlarm しない");

  // 5 分後: 新しい期限（t0 + 20 分）が設定済み（t0 + 15 分）より 2 分以上先なので、1 回だけ設定し直す
  clock.advance(4 * minute);
  await controller.handleFetch(makeRequest("/api/companies"));
  await controller.handleFetch(makeRequest("/api/summary"));
  assert.deepEqual(alarmStorage.alarms, [startAt + idleMs, startAt + 5 * minute + idleMs]);

  // 6 分後: 期限は t0 + 21 分、設定済みは t0 + 20 分。延びは 1 分なので書き込まない
  clock.advance(minute);
  await controller.handleFetch(makeRequest("/login"));
  assert.equal(alarmStorage.alarms.length, 2);
  const extendedDeadline = clock.now() + idleMs;

  // 設定済みのアラーム（t0 + 20 分）が発火: 最後の要求の期限（t0 + 21 分）がまだ先なので、止めずにその期限で設定し直す
  clock.advance(startAt + 5 * minute + idleMs - clock.now());
  await controller.handleAlarm();
  assert.equal(fake.destroyCount, 0);
  assert.equal(fake.runtime.running, true);
  assert.equal(alarmStorage.alarms.length, 3);
  assert.deepEqual(alarmStorage.alarms.at(-1), extendedDeadline);

  // 期限到達でアラーム発火 -> destroy() で停止
  clock.advance(extendedDeadline - clock.now());
  await controller.handleAlarm();
  assert.equal(fake.destroyCount, 1);
  assert.equal(fake.runtime.running, false);

  // 停止済みの状態で再度アラームが走っても何もしない
  await controller.handleAlarm();
  assert.equal(fake.destroyCount, 1);

  // 停止後に新しい要求が来たら再起動し、アラームも設定し直す
  const alarmsBefore = alarmStorage.alarms.length;
  const afterRestart = await controller.handleFetch(makeRequest("/login"));
  assert.equal(afterRestart.status, 200);
  assert.equal(fake.starts.length, 2);
  assert.equal(alarmStorage.alarms.length, alarmsBefore + 1);
  assert.equal(alarmStorage.alarms.at(-1), clock.now() + idleMs);
});

test("DO が退避してメモリ上の印が消えたら、次の要求でアラームを設定し直す", async () => {
  const startAt = 1_700_000_000_000;
  const idleMs = 15 * 60 * 1000;
  const clock = new FakeClock(startAt);
  const fake = createFakeContainer();
  const alarmStorage = createFakeAlarmStorage();
  const first = new DemoContainerController(
    fake.runtime,
    alarmStorage,
    { MEIBO_DOMAIN: DOMAIN },
    { idleTimeoutMs: idleMs },
    clock,
  );
  await first.handleFetch(makeRequest("/login"));
  assert.equal(alarmStorage.alarms.length, 1);

  // 退避後の新しいインスタンス（同じ保存先・動き続けるコンテナ）
  clock.advance(60 * 1000);
  const revived = new DemoContainerController(
    fake.runtime,
    alarmStorage,
    { MEIBO_DOMAIN: DOMAIN },
    { idleTimeoutMs: idleMs },
    clock,
  );
  await revived.handleFetch(makeRequest("/api/summary"));
  await revived.handleFetch(makeRequest("/api/summary"));
  assert.deepEqual(alarmStorage.alarms, [startAt + idleMs, startAt + 60 * 1000 + idleMs]);
});

test("DO が退避したあと、要求がないまま設定済みのアラームが発火したら、保険として止める（最後の要求から、アイドル時間 - 2分以上たっている）", async () => {
  const startAt = 1_700_000_000_000;
  const idleMs = 15 * 60 * 1000;
  const minute = 60 * 1000;
  const clock = new FakeClock(startAt);
  const fake = createFakeContainer();
  const alarmStorage = createFakeAlarmStorage();
  const options = { idleTimeoutMs: idleMs };
  const first = new DemoContainerController(fake.runtime, alarmStorage, { MEIBO_DOMAIN: DOMAIN }, options, clock);
  await first.handleFetch(makeRequest("/login")); // アラーム: t0 + 15 分
  clock.advance(5 * minute);
  await first.handleFetch(makeRequest("/api/summary")); // 設定し直す: t0 + 20 分
  clock.advance(minute);
  await first.handleFetch(makeRequest("/api/summary")); // 延びは 1 分: 書き込まない（期限 t0 + 21 分）
  const lastRequestAt = clock.now();
  assert.deepEqual(alarmStorage.alarms, [startAt + idleMs, startAt + 5 * minute + idleMs]);

  // DO が退避（メモリ上の期限と印が消える）。要求はないまま、設定済みのアラーム（t0 + 20 分）が発火する。
  const revived = new DemoContainerController(fake.runtime, alarmStorage, { MEIBO_DOMAIN: DOMAIN }, options, clock);
  clock.advance(startAt + 5 * minute + idleMs - clock.now());
  await revived.handleAlarm();
  assert.equal(fake.destroyCount, 1, "メモリ上の期限が無ければ止める（アプリが固まったときの保険）");
  assert.ok(
    clock.now() - lastRequestAt >= idleMs - ALARM_REFRESH_SLACK_MS,
    "早く止めても、最後の要求から（アイドル時間 - ALARM_REFRESH_SLACK_MS）以上あとになる",
  );
});

test("setAlarm が失敗したら印を戻し、次の要求で設定し直す", async () => {
  const clock = new FakeClock();
  const fake = createFakeContainer();
  let failures = 1;
  const alarms: number[] = [];
  const alarmStorage: AlarmStorage = {
    async setAlarm(time) {
      if (failures > 0) {
        failures -= 1;
        throw new Error("storage unavailable");
      }
      alarms.push(typeof time === "number" ? time : time.getTime());
    },
  };
  const controller = new DemoContainerController(
    fake.runtime,
    alarmStorage,
    { MEIBO_DOMAIN: DOMAIN },
    undefined,
    clock,
  );
  const failed = await controller.handleFetch(makeRequest("/login"));
  assert.equal(failed.status, 503);
  const ok = await controller.handleFetch(makeRequest("/login"));
  assert.equal(ok.status, 200);
  assert.equal(alarms.length, 1);
});

test("DO 転送時も URL のホスト・Host・許可ヘッダーだけを渡し、CF-* と X-Forwarded-* を落とす", async () => {
  const clock = new FakeClock();
  const fake = createFakeContainer();
  const alarmStorage = createFakeAlarmStorage();
  const controller = new DemoContainerController(
    fake.runtime,
    alarmStorage,
    { MEIBO_DOMAIN: DOMAIN },
    undefined,
    clock,
  );

  const payload = JSON.stringify({ step: "enter" });
  const res = await controller.handleFetch(makeRequest("/demo/enter", {
    method: "POST",
    body: payload,
    headers: {
      "Content-Length": String(new TextEncoder().encode(payload).byteLength),
      "Content-Type": "application/json",
      Origin: `https://${DOMAIN}`,
      Cookie: "__Host-meibo-session=synthetic",
      "X-Meibo-Local": "1",
      "X-Meibo-Context": "demo:demo",
      "CF-Connecting-IP": "198.51.100.30",
      "X-Forwarded-For": "198.51.100.30",
    },
  }));

  assert.equal(res.status, 200);
  const sent = fake.appRequests[0]!;
  assert.equal(sent.url, `http://${DOMAIN}/demo/enter`);
  assert.equal(sent.headers.get("Host"), DOMAIN);
  assert.equal(sent.headers.get("Origin"), `https://${DOMAIN}`);
  assert.equal(sent.headers.get("Cookie"), "__Host-meibo-session=synthetic");
  assert.equal(sent.headers.get("X-Meibo-Local"), "1");
  assert.equal(sent.headers.get("X-Meibo-Context"), "demo:demo");
  assert.equal(sent.headers.get("Content-Length"), String(new TextEncoder().encode(payload).byteLength));
  assert.equal(sent.headers.get("CF-Connecting-IP"), null);
  assert.equal(sent.headers.get("X-Forwarded-For"), null);
  assert.equal(sent.body, payload);
});
