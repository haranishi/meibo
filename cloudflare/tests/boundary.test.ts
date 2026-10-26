import assert from "node:assert/strict";
import test from "node:test";
import {
  BoundaryError,
  boundaryErrorResponse,
  forwardResponse,
  prepareContainerRequest,
  validatePublicOrigin,
  validateRequest,
} from "../src/boundary.ts";

const HOST = "crm.example.test";
const PUBLIC = `https://${HOST}`;
const ORIGIN = { origin: PUBLIC, host: HOST };
const MIB = 1024 * 1024;

function incoming(path = "/", init = {}) {
  const headers = new Headers(init.headers);
  if (!headers.has("Host")) headers.set("Host", HOST);
  return new Request(new URL(path, PUBLIC), { ...init, headers, duplex: "half" });
}

// Exposes malformed metadata that Fetch constructors normally canonicalize first.
function rawUrl(url, init = {}) {
  const request = incoming("/", init);
  Object.defineProperty(request, "url", { value: url });
  return request;
}

function source(byteLength, chunkSize = 64 * 1024) {
  const counts = { pulls: 0, cancels: 0 };
  const chunk = new Uint8Array(chunkSize).fill(97);
  let remaining = byteLength;
  const stream = new ReadableStream({
    pull(controller) {
      counts.pulls++;
      if (remaining === 0) {
        controller.close();
        return;
      }
      const length = Math.min(remaining, chunk.byteLength);
      remaining -= length;
      controller.enqueue(chunk.subarray(0, length));
    },
    cancel() { counts.cancels++; },
  }, { highWaterMark: 0 });
  return { stream, counts };
}

async function drain(stream) {
  assert.ok(stream);
  const reader = stream.getReader();
  let bytes = 0;
  try {
    while (true) {
      const next = await reader.read();
      if (next.done) return bytes;
      bytes += next.value.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
}

function isBoundary(status, code) {
  return (error) => {
    assert.ok(error instanceof BoundaryError);
    assert.equal(error.status, status);
    if (code) assert.equal(error.code, code);
    return true;
  };
}

test("configured HTTPS origins agree with Rust's canonical public authority", () => {
  assert.deepEqual(validatePublicOrigin(PUBLIC), ORIGIN);
  assert.deepEqual(validatePublicOrigin(`${PUBLIC}:443`), ORIGIN);
  assert.deepEqual(validatePublicOrigin(`${PUBLIC}:8443`), {
    origin: `${PUBLIC}:8443`, host: `${HOST}:8443`,
  });
});

test("ambiguous or unsafe configured origins fail closed without echoing configuration", () => {
  for (const value of [
    "", `http://${HOST}`, ` ${PUBLIC}`, `${PUBLIC} `, `${PUBLIC}/`,
    `${PUBLIC}/login`, `${PUBLIC}/..`, `${PUBLIC}?private=value`, `${PUBLIC}#value`,
    `https://user:password@${HOST}`, `https://${HOST}\\private`,
    `HTTPS://${HOST}`, "https://CRM.example.test", "https://crm.example.test.",
    "https://.example.test", "https://crm..example.test", "https://-crm.example.test",
    "https://crm-.example.test", "https://crm_example.test", "https://[::1]",
    `${PUBLIC}:0`, `${PUBLIC}:65536`, `${PUBLIC}:0443`, `${PUBLIC}:`,
    "https://127.000.000.001", "https://éxample.test", `${PUBLIC}\n`,
  ]) {
    assert.throws(() => validatePublicOrigin(value), isBoundary(503));
  }
});

test("GET metadata validation keeps the exact original request and reads no body", () => {
  const request = incoming("/login?next=%2F");
  const result = validateRequest(request, ORIGIN);
  assert.equal(result.request, request);
  assert.equal(result.origin, ORIGIN);
  assert.equal(result.method, "GET");
  assert.equal(result.bodyLimitBytes, MIB);
  assert.equal(request.bodyUsed, false);
});

test("only Rust's exact CSV import path gets the 32 MiB limit", () => {
  const csv = validateRequest(incoming("/api/import?encoding=utf8", { method: "POST" }), ORIGIN);
  assert.equal(csv.bodyLimitBytes, 32 * MIB);
  for (const path of ["/auth/login", "/api/import/", "/api/imported", "/api/%69mport"]) {
    assert.equal(validateRequest(incoming(path, { method: "POST" }), ORIGIN).bodyLimitBytes, MIB);
  }
});

test("URL origin and exact Host must both match, independently of forwarded claims", () => {
  const spoofed = { Forwarded: `proto=https;host=${HOST}`, "X-Forwarded-Host": HOST };
  for (const request of [
    incoming(`http://${HOST}/`, { headers: spoofed }),
    incoming("https://preview.crm.example.test/", { headers: spoofed }),
    incoming("https://meibo-beta-preview.account.workers.dev/", { headers: spoofed }),
    incoming("/", { headers: { Host: "attacker.example", ...spoofed } }),
    incoming("/", { headers: { Host: "CRM.example.test" } }),
    incoming("/", { headers: { Host: `${HOST}:443` } }),
    incoming("/", { headers: { Host: `${HOST}, attacker.example` } }),
    rawUrl(`${PUBLIC}/#private`),
    rawUrl(`https://user:password@${HOST}/`),
    rawUrl(`https://${HOST}\\@attacker.example/`),
  ]) {
    assert.throws(() => validateRequest(request, ORIGIN), isBoundary(403));
  }
  const missingHost = incoming();
  missingHost.headers.delete("Host");
  assert.throws(() => validateRequest(missingHost, ORIGIN), isBoundary(403));
});

test("malformed request targets cannot be forwarded as another authority", () => {
  for (const url of ["not a URL", `${PUBLIC}//attacker.example/path`, `${PUBLIC}/\nprivate`, `${PUBLIC}/${"a".repeat(8192)}`]) {
    assert.throws(() => validateRequest(rawUrl(url), ORIGIN), isBoundary(400));
  }
});

test("only GET and POST are accepted, with a fixed 405 response", async () => {
  for (const method of ["HEAD", "PUT", "PATCH", "DELETE", "OPTIONS"]) {
    let failure;
    try { validateRequest(incoming("/", { method }), ORIGIN); } catch (error) { failure = error; }
    assert.ok(isBoundary(405)(failure));
    const response = boundaryErrorResponse(failure);
    assert.equal(response.status, 405);
    assert.equal(response.headers.get("Allow"), "GET, POST");
    assert.equal(response.headers.get("Cache-Control"), "no-store");
    assert.ok((await response.json()).error);
  }
});

test("queued 32 MiB bodies remain untouched through metadata validation", async () => {
  const input = source(32 * MIB);
  const request = incoming("/api/import", { method: "POST", body: input.stream });
  const checked = validateRequest(request, ORIGIN);
  await Promise.resolve();
  assert.equal(input.counts.pulls, 0);
  assert.equal(request.bodyUsed, false);
  assert.equal(request.body.locked, false);
  assert.equal(checked.bodyLimitBytes, 32 * MIB);
});

test("forward-time request preparation is itself lazy until its body is consumed", async () => {
  const input = source(32 * MIB);
  const request = incoming("/api/import", { method: "POST", body: input.stream });
  const forwarded = prepareContainerRequest(validateRequest(request, ORIGIN));
  await Promise.resolve();
  assert.equal(input.counts.pulls, 0);
  assert.equal(request.bodyUsed, false);
  assert.equal(request.body.locked, false);
  await forwarded.body.cancel();
  assert.equal(input.counts.cancels, 1);
  assert.equal(input.counts.pulls, 0);
});

test("oversized declared bodies are rejected before acquiring a body reader", () => {
  for (const [path, limit] of [["/api/update", MIB], ["/api/import", 32 * MIB]]) {
    const input = source(limit + 1);
    const request = incoming(path, { method: "POST", body: input.stream, headers: { "Content-Length": String(limit + 1) } });
    assert.throws(() => validateRequest(request, ORIGIN), isBoundary(413, "payload_too_large"));
    assert.equal(input.counts.pulls, 0);
    assert.equal(request.body.locked, false);
  }
});

test("invalid and duplicate Content-Length values fail before body consumption", () => {
  for (const length of ["-1", "1.5", "1, 2", "+1", "one", "1e6"]) {
    const input = source(1);
    const request = incoming("/api/update", { method: "POST", body: input.stream, headers: { "Content-Length": length } });
    assert.throws(() => validateRequest(request, ORIGIN), isBoundary(400));
    assert.equal(input.counts.pulls, 0);
  }
});

for (const [path, limit, kind] of [["/api/update", MIB, "JSON"], ["/api/import?encoding=utf8", 32 * MIB, "CSV"]]) {
  test(`${kind} body exactly at its byte limit is forwarded completely`, async () => {
    const input = source(limit);
    const request = incoming(path, { method: "POST", body: input.stream });
    assert.equal(await drain(prepareContainerRequest(validateRequest(request, ORIGIN)).body), limit);
    assert.equal(input.counts.cancels, 0);
  });

  for (const declared of [undefined, "0", "1"]) {
    test(`${kind} streamed overflow is rejected with Content-Length ${declared ?? "absent"}`, async () => {
      const input = source(limit + 1);
      const headers = declared === undefined ? {} : { "Content-Length": declared };
      const request = incoming(path, { method: "POST", body: input.stream, headers });
      const forwarded = prepareContainerRequest(validateRequest(request, ORIGIN));
      assert.equal(forwarded.headers.get("Content-Length"), null);
      await assert.rejects(drain(forwarded.body), isBoundary(413, "payload_too_large"));
      assert.equal(input.counts.cancels, 1);
      assert.ok(input.counts.pulls <= Math.ceil(limit / (64 * 1024)) + 1);
    });
  }
}

test("body limits count UTF-8 bytes rather than string length", async () => {
  const bytes = new TextEncoder().encode("あ".repeat(Math.floor(MIB / 3) + 1));
  assert.ok(bytes.byteLength > MIB);
  const request = incoming("/api/update", { method: "POST", body: bytes });
  await assert.rejects(drain(prepareContainerRequest(validateRequest(request, ORIGIN)).body), isBoundary(413));
});

test("source failures become fixed errors without their private exception details", async () => {
  const stream = new ReadableStream({ pull() { throw new Error("private request body and database token"); } }, { highWaterMark: 0 });
  const request = incoming("/api/update", { method: "POST", body: stream });
  const forwarded = prepareContainerRequest(validateRequest(request, ORIGIN));
  await assert.rejects(drain(forwarded.body), (error) => {
    isBoundary(400)(error);
    assert.doesNotMatch(error.message, /private|database|token/);
    return true;
  });
});

test("forwarded HTTP preserves public authority and Rust authentication inputs", async () => {
  const headers = {
    Host: HOST, Origin: PUBLIC, Cookie: "__Host-meibo-session=synthetic",
    "X-Meibo-Local": "1", "X-Meibo-Context": "synthetic-tenant-session",
    "X-CSRF-Token": "synthetic-csrf", "Content-Type": "application/json",
    "Content-Length": "1", Forwarded: "for=attacker;proto=http;host=attacker.example",
    "X-Forwarded-For": "attacker", "X-Forwarded-Host": "attacker.example",
    "X-Forwarded-Proto": "http", "X-Forwarded-Port": "80", "X-Forwarded-Prefix": "/private",
    Connection: "keep-alive, X-Private-Hop", "X-Private-Hop": "private",
    "Keep-Alive": "timeout=5", "Proxy-Connection": "keep-alive",
    "Proxy-Authorization": "synthetic", "Proxy-Authenticate": "synthetic",
    TE: "trailers", Trailer: "X-Private-Trailer", "Transfer-Encoding": "chunked", Upgrade: "websocket",
  };
  const body = "{\"synthetic\":true}";
  const request = incoming("/api/update?filter=%2F&order=1", { method: "POST", headers, body });
  const forwarded = prepareContainerRequest(validateRequest(request, ORIGIN));
  assert.equal(forwarded.url, `http://${HOST}/api/update?filter=%2F&order=1`);
  assert.equal(forwarded.method, "POST");
  assert.equal(forwarded.redirect, "manual");
  for (const name of ["Host", "Origin", "Cookie", "X-Meibo-Local", "X-Meibo-Context", "X-CSRF-Token", "Content-Type"]) {
    assert.equal(forwarded.headers.get(name), headers[name]);
  }
  for (const name of ["Forwarded", "X-Forwarded-For", "X-Forwarded-Host", "X-Forwarded-Proto", "X-Forwarded-Port", "X-Forwarded-Prefix", "Connection", "X-Private-Hop", "Keep-Alive", "Proxy-Connection", "Proxy-Authorization", "Proxy-Authenticate", "TE", "Trailer", "Transfer-Encoding", "Upgrade", "Content-Length"]) {
    assert.equal(forwarded.headers.get(name), null, name);
  }
  assert.equal(request.headers.get("X-Forwarded-Host"), "attacker.example");
  assert.equal(await forwarded.text(), body);
});

test("nondefault public ports are checked and retained for internal HTTP", () => {
  const origin = validatePublicOrigin(`${PUBLIC}:8443`);
  const request = new Request(`${PUBLIC}:8443/login`, { headers: { Host: `${HOST}:8443` } });
  const forwarded = prepareContainerRequest(validateRequest(request, origin));
  assert.equal(forwarded.url, `http://${HOST}:8443/login`);
  assert.equal(forwarded.headers.get("Host"), `${HOST}:8443`);
  assert.equal(forwarded.body, null);
});

test("Origin, CSRF, content type and tenant validation remain authoritative in Rust", () => {
  const request = incoming("/api/update", { method: "POST", headers: { Origin: "https://attacker.example", "Content-Type": "text/plain", "X-Meibo-Context": "old-context" } });
  const forwarded = prepareContainerRequest(validateRequest(request, ORIGIN));
  assert.equal(forwarded.headers.get("Origin"), "https://attacker.example");
  assert.equal(forwarded.headers.get("Content-Type"), "text/plain");
  assert.equal(forwarded.headers.get("X-Meibo-Context"), "old-context");
  assert.equal(forwarded.headers.get("X-Meibo-Local"), null);
});

test("responses retain distinct cookies, CSP, no-store and logout headers without prereading", async () => {
  const input = source(12);
  const headers = new Headers({
    "Content-Security-Policy": "default-src 'self'; frame-ancestors 'none'",
    "Cache-Control": "no-store", "Clear-Site-Data": '"cache", "storage"',
    "Content-Type": "application/json", Connection: "close, X-Private-Hop", "X-Private-Hop": "private",
  });
  const cookies = [
    "__Host-meibo-session=synthetic; Path=/; HttpOnly; SameSite=Strict; Secure",
    "other=synthetic; Expires=Wed, 21 Oct 2037 07:28:00 GMT; Secure",
  ];
  for (const cookie of cookies) headers.append("Set-Cookie", cookie);
  const response = forwardResponse(new Response(input.stream, { status: 200, headers }));
  assert.equal(input.counts.pulls, 0);
  assert.deepEqual(response.headers.getSetCookie(), cookies);
  for (const name of ["Content-Security-Policy", "Cache-Control", "Clear-Site-Data", "Content-Type"]) {
    assert.equal(response.headers.get(name), headers.get(name));
  }
  assert.equal(response.headers.get("Connection"), null);
  assert.equal(response.headers.get("X-Private-Hop"), null);
  assert.equal(await drain(response.body), 12);
});

test("login redirects remain redirects and do not follow another endpoint", () => {
  const response = forwardResponse(new Response(null, { status: 302, headers: { Location: "/login", "Cache-Control": "no-store" } }));
  assert.equal(response.status, 302);
  assert.equal(response.headers.get("Location"), "/login");
  assert.equal(response.headers.get("Cache-Control"), "no-store");
});

test("all edge errors are no-store and never serialize private causes or configuration", async () => {
  for (const error of [
    new BoundaryError("payload_too_large"),
    new BoundaryError("unexpected_origin"),
    new BoundaryError("invalid_configuration"),
    new Error("private URL https://private.example/?token=synthetic and body synthetic-password"),
    { MEIBO_REMOTE_CONFIG: "private database credential", Cookie: "synthetic-session" },
  ]) {
    const response = boundaryErrorResponse(error);
    assert.equal(response.headers.get("Cache-Control"), "no-store");
    assert.equal(response.headers.get("Content-Type"), "application/json; charset=utf-8");
    assert.equal(response.headers.get("X-Content-Type-Options"), "nosniff");
    const body = await response.text();
    assert.doesNotMatch(body, /private|password|credential|token=|synthetic|MEIBO_REMOTE_CONFIG|Cookie/);
    assert.ok(JSON.parse(body).error);
  }
});
