// 公開ゲート：HTTPレベルの境界の検査。実バイナリ・コンテナ・`wrangler dev`・配備後の公開URLのどれにも向けられる
// （公開URLへは `MEIBO_DEMO_URL=https://… npm run test:one`）。
// 状態を書き換えない検査だけ。ただし「遅い本文の接続」は、前段のWorkerが受け止める本番の構成を確かめるためのもので、
// 公開URL（`MEIBO_DEMO_URL`）のときだけ行う（実バイナリに直接向けると、見張りが働いてデモが終了するため）。
import tls from "node:tls";
import { expect, test } from "@playwright/test";

const remote = Boolean(process.env.MEIBO_DEMO_URL);

test.describe("公開ゲート（HTTPの境界）", () => {
  let origin;
  test.beforeAll(async ({ baseURL }) => { origin = new URL(baseURL).origin; });

  test("到達：/health が200。入口とOGP画像が返る", async ({ request }) => {
    const health = await request.get("/health");
    expect(health.status()).toBe(200);
    expect(await health.json()).toEqual({ ok: true });
    const login = await request.get("/login");
    expect(login.status()).toBe(200);
    expect(login.headers()["content-type"]).toContain("text/html");
    const image = await request.get("/public-entry.png");
    expect(image.status()).toBe(200);
    expect(image.headers()["content-type"]).toBe("image/png");
  });

  test("未認証：業務のAPIとsessionの確認は401、取り込みも入れない", async ({ request }) => {
    for (const path of ["/api/summary", "/api/companies", "/api/opportunities", "/auth/session", "/api/followups"]) {
      const response = await request.get(path);
      expect(response.status(), path).toBe(401);
    }
    const headers = { Origin: origin, "X-Meibo-Local": "1", "Content-Type": "text/csv" };
    const imported = await request.post("/api/import", { headers, data: "origin,name,prefecture\nsynthetic,x,13\n" });
    expect([401, 403, 404]).toContain(imported.status());
  });

  test("公開の問い合わせ受付・連携の戻り先は、デモでは存在しない", async ({ request }) => {
    for (const path of ["/forms/intake.js", "/forms/anything", "/oauth/activity-return"]) {
      const response = await request.get(path);
      expect(response.status(), path).toBe(404);
    }
  });

  test("method：GETとPOST以外は405。POSTは、Originとヘッダーが揃わなければ403", async ({ request }) => {
    expect((await request.fetch("/login", { method: "PUT" })).status()).toBe(405);
    expect((await request.fetch("/login", { method: "DELETE" })).status()).toBe(405);
    const noHeaders = await request.post("/demo/enter", { data: "{}" });
    expect([403, 415]).toContain(noHeaders.status());
    const wrongOrigin = await request.post("/demo/enter", {
      headers: { Origin: "https://evil.example", "X-Meibo-Local": "1", "Content-Type": "application/json" }, data: "{}",
    });
    expect(wrongOrigin.status()).toBe(403);
  });

  test("本文：分割転送・過大な本文・形が違う本文は、入室に届かず断られる", async ({ request }) => {
    const headers = { Origin: origin, "X-Meibo-Local": "1", "Content-Type": "application/json" };
    const huge = await request.post("/demo/enter", { headers, data: JSON.stringify({ pad: "x".repeat(70_000) }) });
    expect(huge.status()).toBe(413);
    const wrongShape = await request.post("/demo/enter", { headers, data: JSON.stringify({ username: "demo" }) });
    expect(wrongShape.status()).toBe(400);
    const notJson = await request.post("/demo/enter", { headers, data: "not json" });
    expect(notJson.status()).toBe(400);
  });

  test("ヘッダー：入口にCSP・X-Frame-Options・nosniff・Referrer-Policyがある", async ({ request }) => {
    const headers = (await request.get("/login")).headers();
    expect(headers["content-security-policy"]).toContain("default-src 'self'");
    expect(headers["content-security-policy"]).toContain("frame-ancestors 'none'");
    expect(headers["x-frame-options"]).toBe("DENY");
    expect(headers["x-content-type-options"]).toBe("nosniff");
    expect(headers["referrer-policy"]).toBe("no-referrer");
  });

  test("Host：公開ドメイン以外のHostは、アプリに届かず断られる", async ({ request }) => {
    const response = await request.get("/login", { headers: { Host: "not-the-demo.example" } });
    expect([400, 403, 404, 421]).toContain(response.status());
  });

  test("遅い本文の接続が、他の訪問者を止めない（公開URLのみ）", async ({ request }) => {
    test.skip(!remote, "実バイナリに直接向けると見張りが働いてデモが終了するため、公開URLだけで行う");
    const url = new URL(origin);
    // 手元の `wrangler dev`（自己署名の証明書）に向けるときだけ、MEIBO_DEMO_INSECURE_TLS=1 で証明書の検証を外す。公開URLでは外さない。
    const insecure = process.env.MEIBO_DEMO_INSECURE_TLS === "1";
    const socket = tls.connect({ host: url.hostname, port: Number(url.port || 443), servername: url.hostname, rejectUnauthorized: !insecure });
    await new Promise((resolve, reject) => { socket.once("secureConnect", resolve); socket.once("error", reject); });
    let received = "";
    socket.on("data", (chunk) => { received += chunk.toString("latin1"); });
    // 本文を2048バイトと言いながら、1バイトも送らない。tiny_http は、1,024バイト以下の本文は接続スレッドが先に読み切るが、
    // それを超える本文は処理のループが読むため、アプリに直接届くと全員の応答が止まる（上限64KiB以下の経路を選ぶ）。
    socket.write([
      "POST /api/companies HTTP/1.1", `Host: ${url.host}`, `Origin: ${origin}`, "X-Meibo-Local: 1",
      "Content-Type: application/json", "Content-Length: 2048", "", "",
    ].join("\r\n"));
    try {
      await new Promise((resolve) => setTimeout(resolve, 1500));
      // 止まっている接続があっても、ほかの訪問者の要求は数秒以内に返る。
      const started = Date.now();
      const health = await request.get("/health", { timeout: 5000 });
      expect(health.status()).toBe(200);
      expect(Date.now() - started).toBeLessThan(3000);
      const login = await request.get("/login", { timeout: 5000 });
      expect(login.status()).toBe(200);
      // 受け止める側が、いつまでも待たずに断る（目安: 本文の読み取り上限5秒＋余裕）か、接続を閉じる。
      await new Promise((resolve) => setTimeout(resolve, 8000));
      const closed = socket.destroyed || /HTTP\/1\.1 (400|408|413)/.test(received);
      expect(closed, `遅い接続が放置されている: ${received.slice(0, 80)}`).toBe(true);
    } finally {
      socket.destroy();
    }
    const after = await request.get("/health");
    expect(after.status()).toBe(200);
  });
});
