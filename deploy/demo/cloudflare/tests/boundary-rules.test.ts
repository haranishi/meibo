import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { isValidContactUrl, normalizeRateLimitClientIp } from "../src/boundary.ts";
import { rateLimitKey } from "../src/worker.ts";

interface ContactUrlCase {
  url: string;
  valid: boolean;
  note: string;
}

// Rust 側（crates/app/src/hosted/demo.rs の単体試験）も同じファイルを読む。
const cases = JSON.parse(
  readFileSync(new URL("./contact-url-cases.json", import.meta.url), "utf8"),
) as ContactUrlCase[];

test("連絡先 URL の共有事例をアプリ（Rust）と同じ規則で判定する", () => {
  assert.ok(cases.filter((c) => c.valid).length >= 15, "受理の事例は 15 件以上");
  assert.ok(cases.filter((c) => !c.valid).length >= 15, "拒否の事例は 15 件以上");
  for (const c of cases) {
    if (c.note === "全体長ちょうど2048文字") assert.equal(c.url.length, 2048);
    if (c.note === "全体長2049文字超過") assert.equal(c.url.length, 2049);
    if (c.note === "ホスト長ちょうど253文字") assert.equal(c.url.length - "https://".length, 253);
    if (c.note === "ホスト長254文字超過") assert.equal(c.url.length - "https://".length, 254);
    assert.equal(isValidContactUrl(c.url), c.valid, `${c.note}: ${JSON.stringify(c.url)}`);
  }
});

test("IPv6 の接続元は先頭 64 ビットに丸めたキーになる", () => {
  const a = normalizeRateLimitClientIp("2001:db8:1:2:aaaa:bbbb:cccc:dddd");
  const b = normalizeRateLimitClientIp("2001:0DB8:0001:0002:1:2:3:4");
  assert.equal(a, "2001:0db8:0001:0002::/64");
  assert.equal(a, b, "同じ /64 内の別アドレスは同じキー");
  assert.notEqual(a, normalizeRateLimitClientIp("2001:db8:1:3::1"), "別の /64 は別のキー");
  // :: の省略形を展開してから丸める
  assert.equal(normalizeRateLimitClientIp("2001:db8::1"), "2001:0db8:0000:0000::/64");
  assert.equal(normalizeRateLimitClientIp("2001:db8:0:0:ffff::"), "2001:0db8:0000:0000::/64");
  assert.equal(normalizeRateLimitClientIp("::1"), "0000:0000:0000:0000::/64");
});

test("IPv4 射影は IPv4 として、IPv4 と不明は従来どおりのキーになる", () => {
  assert.equal(normalizeRateLimitClientIp("::ffff:192.0.2.1"), "192.0.2.1");
  assert.equal(normalizeRateLimitClientIp("::FFFF:c000:0201"), "192.0.2.1");
  assert.equal(normalizeRateLimitClientIp("192.0.2.1"), "192.0.2.1");
  assert.equal(normalizeRateLimitClientIp(" 198.51.100.7 "), "198.51.100.7");
  assert.equal(normalizeRateLimitClientIp(null), "unknown");
  assert.equal(normalizeRateLimitClientIp(""), "unknown");
  assert.equal(normalizeRateLimitClientIp("not-an-ip"), "not-an-ip");
});

test("rateLimitKey は経路の区分と正規化した接続元を組み合わせる", () => {
  const request = (ip?: string) =>
    new Request("https://demo.example.invalid/login", {
      headers: ip === undefined ? {} : { "CF-Connecting-IP": ip },
    });
  assert.equal(rateLimitKey(request("2001:db8:1:2::5"), "enter"), "enter:2001:0db8:0001:0002::/64");
  assert.equal(
    rateLimitKey(request("2001:db8:1:2::5"), "get"),
    rateLimitKey(request("2001:db8:1:2:ffff::9"), "get"),
  );
  assert.equal(rateLimitKey(request("::ffff:203.0.113.9"), "post"), "post:203.0.113.9");
  assert.equal(rateLimitKey(request("203.0.113.9"), "post"), "post:203.0.113.9");
  assert.equal(rateLimitKey(request(), "post"), "post:unknown");
});
