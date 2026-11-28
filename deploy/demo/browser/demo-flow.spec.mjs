// 公開デモの実ブラウザ試験。実バイナリ（--public-demo）と自己署名のHTTPS前段に対して、
// 入口 → 入室 → 紹介動画の流れ（再架電・商談ボード・月次集計）→ 共有session → 入室の全体上限、を確かめる。
// 全員が1組織を共有する状態を持つデモなので、順番に実行する（読むだけの確認 → 書き換える操作の順）。
import { expect, test as base } from "@playwright/test";

function recordBrowserErrors(context, problems) {
  context.on("requestfailed", (request) => problems.push(`requestfailed: ${request.url()} ${request.failure()?.errorText}`));
  context.on("weberror", (event) => problems.push(`pageerror: ${event.error().message}`));
}

const test = base.extend({
  newVisitorContext: async ({ browser }, use, info) => {
    const { baseURL, viewport } = info.project.use;
    const problems = [];
    await use(async () => {
      const context = await browser.newContext({ baseURL, viewport, ignoreHTTPSErrors: true, serviceWorkers: "block" });
      recordBrowserErrors(context, problems);
      return context;
    });
    if (info.status !== info.expectedStatus && problems.length > 0) {
      await info.attach("browser-errors", { body: problems.join("\n"), contentType: "text/plain" });
    }
  },
});

// アプリと同じ書式（日本時間・24時間表記）。
const tokyo = new Intl.DateTimeFormat("ja-JP", {
  timeZone: "Asia/Tokyo", year: "numeric", month: "numeric", day: "numeric",
  hour: "2-digit", minute: "2-digit", hourCycle: "h23",
});
const tokyoWeekday = new Intl.DateTimeFormat("en-US", { timeZone: "Asia/Tokyo", weekday: "short" });
const DAY_MS = 86_400_000;
const JST_OFFSET_MS = 9 * 3_600_000;

/** 日本時間の「今日」の日数番号（日本時間の0時を境にする）。 */
function jstDayIndex(nowMs) { return Math.floor((nowMs + JST_OFFSET_MS) / DAY_MS); }
/** 日本時間の日数番号 → その日の朝9時（日本時間）のUNIXミリ秒。 */
function jstMorning9(dayIndex) { return dayIndex * DAY_MS; /* 日本時間9:00 = UTC 0:00 */ }
/** 「土日を除く翌日9時」（祝日は平日として数える）。 */
function nextWeekdayMorning(nowMs) {
  let day = jstDayIndex(nowMs) + 1;
  for (;;) {
    const weekday = tokyoWeekday.format(new Date(jstMorning9(day)));
    if (weekday !== "Sat" && weekday !== "Sun") return jstMorning9(day);
    day += 1;
  }
}

const yen = (digits) => new RegExp(`[¥￥]${digits}`); // 全角の円記号にも対応

test.describe.serial("公開デモ（実バイナリ・HTTPS前段）", () => {
  let context;
  let page;
  let origin;
  const problems = [];

  test.beforeAll(async ({ browser }, info) => {
    const { baseURL, viewport } = info.project.use;
    origin = new URL(baseURL).origin;
    // 画面は日時をブラウザの時間帯で表示する。期待値は日本時間で作るので、実行環境の時間帯（CIはUTC）によらず日本時間にそろえる。
    context = await browser.newContext({ baseURL, viewport, ignoreHTTPSErrors: true, serviceWorkers: "block", timezoneId: "Asia/Tokyo", locale: "ja-JP" });
    recordBrowserErrors(context, problems);
    page = await context.newPage();
    page.on("response", (response) => { if (response.status() >= 500) problems.push(`HTTP ${response.status()} ${response.url()}`); });
  });
  test.afterAll(async () => { await context?.close(); });

  const nav = (label) => page.locator("nav.workspace-navigation button").filter({ hasText: label }).first();
  const overflow = () => page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);

  test("入口：未認証の / はデモ入口へ。説明・OGP・共有が出る", async () => {
    await page.goto("/");
    await expect(page).toHaveURL(`${origin}/login`);
    await expect(page).toHaveTitle("meibo · 公開デモ");
    await expect(page.getByRole("heading", { name: "営業ワークスペースを試す" })).toBeVisible();
    await expect(page.getByRole("button", { name: "デモを開く" })).toBeEnabled();
    await expect(page.locator("input[type=password]")).toHaveCount(0);
    for (const text of [
      "データはすべて合成です", "全員が同じデモを共有します", "15分操作がないと初期化されます",
      "個人情報を入力しないでください", "実行先に通信の記録が残ります",
    ]) await expect(page.locator("main")).toContainText(text);

    await expect(page.locator('link[rel="canonical"]')).toHaveAttribute("href", `${origin}/login`);
    await expect(page.locator('meta[property="og:title"]')).toHaveAttribute("content", "meibo · 公開デモ");
    await expect(page.locator('meta[property="og:image"]')).toHaveAttribute("content", `${origin}/public-entry.png`);
    await expect(page.locator('meta[name="twitter:card"]')).toHaveAttribute("content", "summary_large_image");
    const encoded = encodeURIComponent(`${origin}/login`);
    await expect(page.locator("#public-share-x")).toHaveAttribute("href", new RegExp(`^https://twitter\\.com/intent/tweet\\?url=${encoded}`));
    await expect(page.locator("#public-share-line")).toHaveAttribute("href", new RegExp(`^https://social-plugins\\.line\\.me/lineit/share\\?url=${encoded}`));
    await expect(page.locator("#public-share-copy")).toBeEnabled();
    const image = await page.request.get("/public-entry.png");
    expect(image.status()).toBe(200);
    expect(image.headers()["content-type"]).toBe("image/png");
    expect(await overflow()).toBeLessThanOrEqual(0);
  });

  test("入口：パスワードログインは使えず、Hostが違う要求は拒否される", async () => {
    const headers = { Origin: origin, "X-Meibo-Local": "1", "Content-Type": "application/json" };
    const login = await page.request.post("/auth/login", { headers, data: JSON.stringify({ tenant: "demo", username: "demo", password: "x".repeat(12) }) });
    expect(login.status()).toBe(404);
    const wrongHost = await page.request.get("/login", { headers: { Host: "evil.example" } });
    expect(wrongHost.status()).toBe(403);
    const page_ = await page.request.get("/login");
    expect(page_.headers()["x-frame-options"]).toBe("DENY");
    expect(page_.headers()["content-security-policy"]).toContain("default-src 'self'");
  });

  test("入口：キーボードだけで入室でき、フォーカスが見える", async ({ newVisitorContext }) => {
    const own = await newVisitorContext();
    try {
      const keyboard = await own.newPage();
      await keyboard.goto("/login");
      for (let step = 0; step < 8; step += 1) {
        if (await keyboard.evaluate(() => document.activeElement?.id) === "demo-enter") break;
        await keyboard.keyboard.press("Tab");
      }
      await expect(keyboard.locator("#demo-enter")).toBeFocused();
      const outline = await keyboard.locator("#demo-enter").evaluate((element) => parseFloat(getComputedStyle(element).outlineWidth));
      expect(outline).toBeGreaterThanOrEqual(2);
      await keyboard.keyboard.press("Enter");
      await expect(keyboard).toHaveURL(`${origin}/`);
      await expect(keyboard.locator(".demo-banner")).toBeVisible();
    } finally {
      await own.close();
    }
  });

  test("入口：初期化後の古いsessionは、初期化の案内つきで入口へ戻る", async ({ newVisitorContext }) => {
    const own = await newVisitorContext();
    try {
      await own.addCookies([{ name: "__Host-meibo-session", value: "a".repeat(64), url: origin, secure: true, httpOnly: true, sameSite: "Strict" }]);
      const stale = await own.newPage();
      await stale.goto("/");
      await expect(stale).toHaveURL(`${origin}/login?reset=1`);
      await expect(stale.locator("#demo-reset-notice")).toBeVisible();
      await expect(stale.locator("#demo-reset-notice")).toContainText("デモが初期化されたか、しばらく操作がなかったため、入り直しが必要です。もう一度『デモを開く』を押してください。");
      await expect(stale.getByRole("button", { name: "デモを開く" })).toBeEnabled();
    } finally {
      await own.close();
    }
  });

  // 実行先のコンテナは、最初の入室のときに起動に数秒かかり、準備ができるまで503を返すことがある。
  // 応答は、前段が返す形（503・Retry-After・JSONのcode）を、ブラウザの通信の差し替えで再現する。
  const startingReply = (retryAfter = "1") => ({
    status: 503,
    headers: { "Retry-After": retryAfter, "Content-Type": "application/json; charset=utf-8" },
    body: JSON.stringify({ error: "起動中です。少し待ってからもう一度お試しください。", code: "starting" }),
  });

  test("入口：起動待ちの503は自動で再試行し、待つ間は案内を出して、入室できる", async ({ newVisitorContext }) => {
    const own = await newVisitorContext();
    try {
      const entry = await own.newPage();
      let calls = 0;
      await entry.route("**/demo/enter", async (route) => {
        calls += 1;
        if (calls === 1) await new Promise((resolve) => setTimeout(resolve, 1800)); // 1.5秒の案内が出るまで待たせる
        if (calls <= 2) return route.fulfill(startingReply());
        return route.continue();
      });
      await entry.goto("/login");
      await entry.getByRole("button", { name: "デモを開く" }).click();
      await expect(entry.locator("#demo-enter")).toHaveText("起動しています。数秒かかることがあります…");
      await expect(entry.locator("#demo-enter")).toBeDisabled();
      await expect(entry).toHaveURL(`${origin}/`, { timeout: 20_000 });
      await expect(entry.locator(".demo-banner")).toBeVisible();
      expect(calls).toBe(3);
    } finally {
      await own.close();
    }
  });

  test("入口：メンテナンス中の503は、待たずにすぐ案内し、ボタンが元に戻る", async ({ newVisitorContext }) => {
    const own = await newVisitorContext();
    try {
      const entry = await own.newPage();
      let calls = 0;
      await entry.route("**/demo/enter", (route) => {
        calls += 1;
        return route.fulfill({
          status: 503,
          headers: { "Retry-After": "300", "Content-Type": "application/json; charset=utf-8" },
          body: JSON.stringify({ error: "メンテナンス中です。しばらくしてからお試しください。", code: "maintenance" }),
        });
      });
      await entry.goto("/login");
      await entry.getByRole("button", { name: "デモを開く" }).click();
      await expect(entry.locator("#login-error")).toHaveText("メンテナンス中です。しばらくしてからお試しください。", { timeout: 3000 });
      await expect(entry.getByRole("button", { name: "デモを開く" })).toBeEnabled();
      expect(calls).toBe(1);
    } finally {
      await own.close();
    }
  });

  test("入口：503が続いたら、4回の再試行のあとに諦めて案内する", async ({ newVisitorContext }) => {
    const own = await newVisitorContext();
    try {
      const entry = await own.newPage();
      let calls = 0;
      await entry.route("**/demo/enter", (route) => { calls += 1; return route.fulfill(startingReply()); });
      await entry.goto("/login");
      await entry.getByRole("button", { name: "デモを開く" }).click();
      await expect(entry.locator("#login-error")).toHaveText("入室できませんでした。時間を置いてもう一度お試しください。", { timeout: 15_000 });
      await expect(entry.getByRole("button", { name: "デモを開く" })).toBeEnabled();
      expect(calls).toBe(5);
    } finally {
      await own.close();
    }
  });

  test("入室：ボタン1つで入れ、常時の注意書きと、使えない導線の非表示が効く", async () => {
    // CIで一度、押しても画面が移らず原因が残らなかったので、失敗したときは入室の応答と画面の案内を残す。
    const replies = [];
    const note = (response) => { if (response.url() === `${origin}/demo/enter`) replies.push(response.status()); };
    page.on("response", note);
    await page.getByRole("button", { name: "デモを開く" }).click();
    try {
      await expect(page).toHaveURL(`${origin}/`);
    } catch (error) {
      const text = (selector) => page.locator(selector).textContent({ timeout: 1000 }).catch(() => "なし");
      error.message += `\n入室の応答: [${replies.join(", ")}]\nボタン: ${await text("#demo-enter")}\n案内: ${await text("#login-error")}\nこれまでの問題: ${problems.join(" / ") || "なし"}`;
      throw error;
    } finally {
      page.off("response", note);
    }
    const banner = page.locator(".demo-banner");
    await expect(banner).toBeVisible();
    for (const text of ["公開デモ", "データはすべて合成です", "15分操作がないと初期化されます", "個人情報を入力しないでください"]) {
      await expect(banner).toContainText(text);
    }
    await expect(page.locator(".demo-badge")).toBeVisible();
    await expect(page.locator(".hosted-session")).toContainText("demo · 編集");
    expect(await overflow()).toBeLessThanOrEqual(0);

    // 画面を下へ動かしても、帯は上に残る。
    await page.evaluate(() => window.scrollTo(0, 600));
    const top = await banner.evaluate((element) => element.getBoundingClientRect().top);
    expect(Math.abs(top)).toBeLessThanOrEqual(1);
    await page.evaluate(() => window.scrollTo(0, 0));

    // 使えない操作の導線は、見えず、キーボードでも辿れない。
    for (const selector of ["#import-csv", "#csv-import-dialog", "#sales-connections", "#sales-tools", ".lead-inbox-open", "#lead-inbox-dialog", "#activity-sync-root"]) {
      await expect(page.locator(selector), selector).toBeHidden();
    }

    const cookies = await context.cookies();
    expect(cookies).toHaveLength(1);
    expect(cookies[0]).toMatchObject({ name: "__Host-meibo-session", secure: true, httpOnly: true, sameSite: "Strict" });
  });

  test("止める操作：入室したあとでも、取り込み・問い合わせ・連携は403で断られ、公開の受付は404", async () => {
    const session = await (await page.request.get("/auth/session")).json();
    expect(session).toMatchObject({ username: "demo", tenant: "demo", role: "editor" });
    const headers = { Origin: origin, "X-Meibo-Local": "1", "X-Meibo-Context": session.storage_scope, "X-Requested-With": "XMLHttpRequest" };
    const disabled = async (response, label) => {
      expect(response.status(), label).toBe(403);
      expect((await response.json()).code, label).toBe("demo_disabled");
    };
    await disabled(await page.request.post("/api/import?origin=synthetic", {
      headers: { ...headers, "Content-Type": "text/csv" }, data: "origin,name,prefecture\nsynthetic,x,13\n",
    }), "POST /api/import");
    for (const path of ["/api/leads", "/api/lead-form", "/api/activity-sync", "/api/activity-sync/status"]) {
      await disabled(await page.request.get(path, { headers }), `GET ${path}`);
    }
    await disabled(await page.request.post("/api/lead-form", { headers: { ...headers, "Content-Type": "application/json" }, data: "{}" }), "POST /api/lead-form");
    for (const path of ["/forms/intake.js", "/forms/anything", "/oauth/activity-return"]) {
      expect((await page.request.get(path)).status(), path).toBe(404);
    }
    // 業務の読み取りは、これまでどおり使える。
    expect((await page.request.get("/api/summary", { headers })).status()).toBe(200);
  });

  test("ホーム：期限が過ぎた再架電が1件（昨日の朝9時・日本時間）", async () => {
    await nav("ホーム").click();
    const items = page.locator("#followup-items .home-list-item");
    await expect(items).toHaveCount(1);
    await expect(items.first()).toContainText("合成・");
    const expectedDue = tokyo.format(new Date(jstMorning9(jstDayIndex(Date.now()) - 1)));
    await expect(items.first()).toContainText(`${expectedDue}（日本時間）`);
    await expect(page.locator("#followup-count")).toHaveText("1");
  });

  test("商談：月次集計の初期値が紹介動画と同じ（32万円・50%・65万円・97万円）", async () => {
    await nav("商談").click();
    await page.getByRole("button", { name: "月次集計" }).click();
    const totals = page.locator("dl.sales-report-totals");
    await expect(totals).toBeVisible();
    const value = (name) => totals.locator("> div").filter({ has: page.locator("dt", { hasText: name }) }).locator("dd").first();
    await expect(value("月次受注額")).toHaveText(yen("320,000"));
    await expect(value("受注率")).toHaveText("50%");
    await expect(value("確度加重見込み")).toHaveText(yen("650,000"));
    await expect(value("着地予測")).toHaveText(yen("970,000"));

    // 担当別：受注額・確度加重見込み・着地予測・月間目標・目標との差・確度未設定（全角の円記号は半角にそろえる）。
    const rows = page.locator("table.sales-projection-table tbody tr");
    await expect(rows).toHaveCount(2);
    const cells = await rows.evaluateAll((trs) => trs.map((tr) => [...tr.cells].map((cell) => cell.textContent.trim().replace("￥", "¥"))));
    expect(cells).toEqual([
      ["demo", "¥320,000", "¥480,000", "¥800,000", "¥400,000", "¥400,000", "0 件"],
      ["demo-b", "¥0", "¥170,000", "¥170,000", "¥200,000", "-¥30,000", "0 件"],
    ]);
    expect(await overflow()).toBeLessThanOrEqual(0);
  });

  test("商談：ボードに6つの段階と9件のカード（合成・の名前だけ）", async () => {
    await nav("商談").click();
    const toggle = page.getByRole("button", { name: "ボードで表示" });
    if (await toggle.count()) await toggle.click(); // 狭い幅は最初が一覧
    const lanes = page.locator(".crm-lane");
    await expect(lanes).toHaveCount(6);
    const counts = await lanes.evaluateAll((elements) => elements.map((lane) => ({
      stage: lane.querySelector("h2")?.textContent?.trim().split(/\s|·/)[0],
      cards: lane.querySelectorAll(".crm-deal-card").length,
    })));
    expect(counts).toEqual([
      { stage: "見込み", cards: 2 }, { stage: "評価", cards: 2 }, { stage: "提案", cards: 2 },
      { stage: "交渉", cards: 1 }, { stage: "受注", cards: 1 }, { stage: "失注", cards: 1 },
    ]);
    const names = await page.locator(".crm-deal-card .crm-record-name").allInnerTexts();
    expect(names).toHaveLength(9);
    for (const name of names) expect(name.startsWith("合成・"), name).toBe(true);
  });

  test("架電モード：不在のため再架電を予約すると、土日を除く翌日の朝9時が入る", async () => {
    await nav("取引先").click();
    await page.getByRole("button", { name: "架電モード" }).first().click();
    await expect(page.getByRole("heading", { name: "活動を記録する" })).toBeVisible();
    const company = (await page.locator("#selected-company-title").first().innerText()).trim();
    expect(company.startsWith("合成・")).toBe(true);
    await page.getByLabel("活動メモ").fill("合成の試験メモ");
    await page.getByLabel("不在のため再架電を予約").check();
    await expect(page.locator("#activity-callback-timing")).toContainText("土日を除く翌日9時（日本時間）");
    await page.getByRole("button", { name: "保存して次へ" }).click();
    await expect(page.locator("#notice")).toContainText("架電を記録し、再架電を予約しました。");
    await page.getByRole("button", { name: "架電モードを終了" }).click();

    const heads = await page.locator("#table-container table thead tr:last-child > th").allInnerTexts();
    const column = heads.findIndex((text) => text.includes("次回対応"));
    expect(column).toBeGreaterThanOrEqual(0);
    const row = page.locator("#table-container table tbody tr").filter({ hasText: company }).first();
    const expected = tokyo.format(new Date(nextWeekdayMorning(Date.now())));
    await expect(row.locator(":scope > th, :scope > td").nth(column)).toHaveText(expected);
  });

  test("商談：段階を受注へ変えると、月次集計の着地予測が97万円から109万円へ動く", async () => {
    await nav("商談").click();
    const toggle = page.getByRole("button", { name: "ボードで表示" });
    if (await toggle.count()) await toggle.click();
    const name = "合成・受注管理画面の改修";
    await page.getByLabel(`${name}の商談段階`).selectOption({ label: "受注" });
    await expect(page.locator("#notice")).toContainText("商談段階を更新しました。");
    const won = page.locator(".crm-lane").filter({ has: page.locator("h2", { hasText: /^受注/ }) });
    await expect(won.locator(".crm-deal-card")).toHaveCount(2);

    await page.getByRole("button", { name: "月次集計" }).click();
    const totals = page.locator("dl.sales-report-totals");
    const value = (label) => totals.locator("> div").filter({ has: page.locator("dt", { hasText: label }) }).locator("dd").first();
    await expect(value("着地予測")).toHaveText(yen("1,090,000"));
    await expect(value("受注率")).toHaveText("66.7%");
    await expect(value("月次受注額")).toHaveText(yen("620,000"));
  });

  // 入室は起動ごとに1つの共有sessionを渡すだけ。別々に入室した訪問者は同じsessionを使い、片方のログアウトで他方は止まらない。
  test("共有session：別々に入室した2人が両方操作でき、片方がログアウトしても、もう片方は続けられる", async ({ newVisitorContext }) => {
    const base = { Origin: origin, "X-Meibo-Local": "1", "X-Requested-With": "XMLHttpRequest" };
    const contexts = [];
    try {
      const visitors = [];
      for (let i = 0; i < 2; i += 1) {
        const own = await newVisitorContext();
        contexts.push(own);
        const entered = await own.request.post("/demo/enter", { headers: { ...base, "Content-Type": "application/json" }, data: "{}" });
        expect(entered.status(), `訪問者${i + 1}の入室`).toBe(200);
        const scope = (await entered.json()).storage_scope;
        visitors.push({ own, headers: { ...base, "X-Meibo-Context": scope } });
      }
      for (const [i, { own, headers }] of visitors.entries()) {
        expect((await own.request.get("/api/summary", { headers })).status(), `訪問者${i + 1}の操作`).toBe(200);
      }
      const [first, second] = visitors;
      const logout = await first.own.request.post("/auth/logout", { headers: { ...first.headers, "Content-Type": "application/json" }, data: "{}" });
      expect(logout.status()).toBe(200);
      expect(await first.own.cookies()).toHaveLength(0);
      expect((await second.own.request.get("/api/summary", { headers: second.headers })).status(), "ログアウトしていない側").toBe(200);
      // 先に入室していたメインの画面も止まらない。
      const session = await (await page.request.get("/auth/session")).json();
      expect((await page.request.get("/api/summary", { headers: { ...base, "X-Meibo-Context": session.storage_scope } })).status()).toBe(200);
    } finally {
      for (const own of contexts) await own.close();
    }
  });

  // アプリの全体の上限（1分ごとの固定窓・300回）を確かめる。公開URLでは前段のWorkerの接続元ごとの上限が先に効き、
  // 自分のIPを制限してしまうので省略する。429を受けてから押すまでに分がまたいで入れてしまったら、連打からやり直す（最大3回）。
  test("入室の全体上限：429が返った直後に、新しい訪問者には「混み合っています」が出て、入室済みの画面は動き続ける", async ({ newVisitorContext }) => {
    test.skip(Boolean(process.env.MEIBO_DEMO_URL), "公開URLでは前段の接続元ごとの上限が先に効くため省略");
    const headers = { Origin: origin, "X-Meibo-Local": "1", "Content-Type": "application/json" };
    let shown = false;
    for (let round = 0; round < 3 && !shown; round += 1) {
      const own = await newVisitorContext();
      try {
        const visitor = await own.newPage();
        await visitor.goto("/login");
        await expect(visitor.getByRole("button", { name: "デモを開く" })).toBeEnabled();
        let busy;
        for (let attempt = 0; attempt < 400 && !busy; attempt += 1) {
          const response = await page.request.post("/demo/enter", { headers, data: "{}" });
          if (response.status() !== 200) busy = response;
        }
        expect(busy, "400回までに429が返る").toBeTruthy();
        expect(busy.status()).toBe(429);
        expect((await busy.json()).code).toBe("demo_busy");
        await visitor.getByRole("button", { name: "デモを開く" }).click();
        const error = visitor.locator("#login-error");
        await expect(error.or(visitor.locator(".demo-banner"))).toBeVisible();
        if (!(await error.isVisible())) continue; // 分がまたいで入室できた。連打からやり直す。
        await expect(error).toHaveText("混み合っています。少し待ってからもう一度お試しください。");
        await expect(visitor.getByRole("button", { name: "デモを開く" })).toBeEnabled();
        await expect(visitor).toHaveURL(`${origin}/login`);
        shown = true;
      } finally {
        await own.close();
      }
    }
    expect(shown, "3回のうちに「混み合っています」を確かめる").toBe(true);
    // 連打のあとも、先に入室していた画面の操作は止まらない（共有sessionは追い出されない）。
    await page.goto("/");
    await expect(page.locator(".demo-banner")).toBeVisible();
    await expect(page.locator(".hosted-session")).toContainText("demo · 編集");
    const session = await (await page.request.get("/auth/session")).json();
    const summary = await page.request.get("/api/summary", { headers: { ...headers, "X-Meibo-Context": session.storage_scope } });
    expect(summary.status()).toBe(200);
  });

  test("最後に：ページのエラーも、通信の失敗も、サーバーの5xxも無い", async () => {
    expect(problems).toEqual([]);
  });
});
