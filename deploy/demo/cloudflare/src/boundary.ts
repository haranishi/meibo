import type { DemoClock, DemoConfig } from "./contracts.ts";

const ERROR_DETAILS = {
  invalid_configuration: [503, "公開デモの設定を確認してください。"],
  invalid_request: [400, "要求を確認してください。"],
  unexpected_host: [403, "公開URLからアクセスしてください。"],
  method_not_allowed: [405, "この操作には対応していません。"],
  request_timeout: [408, "送信が時間内に完了しませんでした。"],
  length_required: [411, "Content-Length を指定して送信してください。"],
  payload_too_large: [413, "送信内容が上限（64KiB）を超えています。"],
  unavailable: [503, "一時的に利用できません。少し待ってからもう一度お試しください。"],
} as const;

export type BoundaryErrorCode = keyof typeof ERROR_DETAILS;

/** 内部例外の詳細を外へ漏らさない固定コード付きの境界エラー。 */
export class BoundaryError extends Error {
  readonly code: BoundaryErrorCode;
  readonly status: number;

  constructor(code: BoundaryErrorCode) {
    super(ERROR_DETAILS[code][1]);
    this.name = "BoundaryError";
    this.code = code;
    this.status = ERROR_DETAILS[code][0];
  }
}

const HOP_BY_HOP_HEADERS = [
  "Connection",
  "Keep-Alive",
  "Proxy-Authenticate",
  "Proxy-Authorization",
  "Proxy-Connection",
  "TE",
  "Trailer",
  "Transfer-Encoding",
  "Upgrade",
] as const;

/** コンテナへそのまま渡すことを許可する要求ヘッダー（小文字名）。 */
const ALLOWED_FORWARD_HEADERS = new Set([
  "cookie",
  "origin",
  "content-type",
  "accept",
  "accept-language",
  "x-csrf-token",
]);

const MAX_CONTACT_URL_LEN = 2048;

function parseIpv4Octets(text: string): [number, number, number, number] | null {
  const parts = text.split(".");
  if (parts.length !== 4) return null;
  const octets: number[] = [];
  for (const part of parts) {
    if (!/^(?:0|[1-9][0-9]{0,2})$/.test(part)) return null;
    const value = Number(part);
    if (value > 255) return null;
    octets.push(value);
  }
  return [octets[0]!, octets[1]!, octets[2]!, octets[3]!];
}

function parseIpv6SegmentList(text: string, allowTrailingIpv4: boolean): number[] | null {
  if (text.length === 0) return [];
  if (text.startsWith(":") || text.endsWith(":")) return null;
  const rawParts = text.split(":");
  const values: number[] = [];
  for (let i = 0; i < rawParts.length; i++) {
    const part = rawParts[i]!;
    if (part.includes(".")) {
      if (!allowTrailingIpv4 || i !== rawParts.length - 1) return null;
      const octets = parseIpv4Octets(part);
      if (!octets) return null;
      values.push((octets[0] << 8) | octets[1], (octets[2] << 8) | octets[3]);
    } else {
      if (!/^[0-9a-fA-F]{1,4}$/.test(part)) return null;
      values.push(Number.parseInt(part, 16));
    }
  }
  return values;
}

/**
 * Rust の std::net::Ipv6Addr::from_str と同じ規則で IPv6 アドレスを 8 個の 16bit 値へ分解する。
 * 不正な形式・ゾーン ID・省略 0 個の :: は null を返す。
 */
export function parseIpv6Groups(
  text: string,
): [number, number, number, number, number, number, number, number] | null {
  if (text.length === 0 || !/^[0-9a-fA-F:.]+$/.test(text)) return null;
  const halves = text.split("::");
  if (halves.length > 2) return null;
  const hasCompression = halves.length === 2;
  const left = parseIpv6SegmentList(halves[0]!, !hasCompression);
  if (!left) return null;
  const right = hasCompression ? parseIpv6SegmentList(halves[1]!, true) : [];
  if (!right) return null;
  const total = left.length + right.length;
  if (hasCompression ? total >= 8 : total !== 8) return null;
  const groups = [...left, ...new Array<number>(8 - total).fill(0), ...right];
  return [
    groups[0]!,
    groups[1]!,
    groups[2]!,
    groups[3]!,
    groups[4]!,
    groups[5]!,
    groups[6]!,
    groups[7]!,
  ];
}

function validPort(port: string): boolean {
  if (!/^[0-9]+$/.test(port)) return false;
  const value = Number(port);
  return Number.isSafeInteger(value) && value >= 1 && value <= 65535;
}

/**
 * 連絡先 URL をアプリ（Rust の validate_contact_url）と同じ規則で検証する。
 * 小文字の http:// / https:// のみ、ASCII 図形文字（禁止記号を除く）、authority とホスト・ポートを検査する。
 */
export function isValidContactUrl(raw: string): boolean {
  if (raw.length === 0 || raw.length > MAX_CONTACT_URL_LEN) {
    return false;
  }
  let rest: string;
  if (raw.startsWith("https://")) {
    rest = raw.slice("https://".length);
  } else if (raw.startsWith("http://")) {
    rest = raw.slice("http://".length);
  } else {
    return false;
  }
  for (let i = 0; i < raw.length; i++) {
    const code = raw.charCodeAt(i);
    if (code < 0x21 || code > 0x7e) return false;
    if (
      code === 0x3c // <
      || code === 0x3e // >
      || code === 0x22 // "
      || code === 0x27 // '
      || code === 0x60 // `
      || code === 0x5c // \
    ) {
      return false;
    }
  }
  const authority = rest.split(/[/?#]/, 1)[0] ?? "";
  if (authority.length === 0 || authority.includes("@")) {
    return false;
  }
  if (authority.startsWith("[")) {
    const closeIndex = authority.indexOf("]");
    if (closeIndex === -1) return false;
    const addr = authority.slice(1, closeIndex);
    const tail = authority.slice(closeIndex + 1);
    if (!parseIpv6Groups(addr)) return false;
    if (tail.length > 0) {
      if (!tail.startsWith(":")) return false;
      if (!validPort(tail.slice(1))) return false;
    }
  } else {
    const parts = authority.split(":");
    if (parts.length > 2) return false;
    const host = parts[0] ?? "";
    if (parts.length === 2 && !validPort(parts[1]!)) {
      return false;
    }
    if (
      host.length === 0
      || host.length > 253
      || !/^[A-Za-z0-9.-]+$/.test(host)
      || host.split(".").some((label) => label.length === 0 || label.startsWith("-") || label.endsWith("-"))
    ) {
      return false;
    }
  }
  return true;
}

/**
 * 接続元 IP をレート制限のキー用に正規化する。
 * IPv6 は先頭 64 ビット（xxxx:xxxx:xxxx:xxxx::/64・小文字・各群を4桁のゼロ埋めにそろえる）へ丸め、
 * IPv4 射影（::ffff:a.b.c.d）は IPv4 として扱う。IPv4 と不明は従来どおり。
 */
export function normalizeRateLimitClientIp(raw: string | null | undefined): string {
  const clientIp = raw?.trim();
  if (!clientIp || clientIp.length === 0) {
    return "unknown";
  }
  const groups = parseIpv6Groups(clientIp);
  if (!groups) {
    return clientIp;
  }
  if (
    groups[0] === 0
    && groups[1] === 0
    && groups[2] === 0
    && groups[3] === 0
    && groups[4] === 0
    && groups[5] === 0xffff
  ) {
    return `${groups[6] >> 8}.${groups[6] & 0xff}.${groups[7] >> 8}.${groups[7] & 0xff}`;
  }
  const prefix = groups
    .slice(0, 4)
    .map((group) => group.toString(16).padStart(4, "0"))
    .join(":");
  return `${prefix}::/64`;
}

/**
 * 公開ドメイン設定（MEIBO_DOMAIN）を検証し、正規化した host を返す。
 * スキーム・パス・空白・大文字は受け付けない。
 */
export function validateDemoDomain(value: string | undefined): string {
  if (typeof value !== "string" || value.length === 0 || value.length > 253) {
    throw new BoundaryError("invalid_configuration");
  }
  if (!/^[a-z0-9.-]+(?::[1-9][0-9]{0,4})?$/.test(value)) {
    throw new BoundaryError("invalid_configuration");
  }
  let url: URL;
  try {
    url = new URL(`https://${value}`);
  } catch {
    throw new BoundaryError("invalid_configuration");
  }
  if (url.protocol !== "https:" || (value !== url.host && value !== `${url.hostname}:443`)) {
    throw new BoundaryError("invalid_configuration");
  }
  const labels = url.hostname.split(".");
  if (labels.some((label) => !label || label.length > 63 || label.startsWith("-") || label.endsWith("-"))) {
    throw new BoundaryError("invalid_configuration");
  }
  return url.host;
}

/** 要求メソッドを GET / POST のみに制限する。 */
export function validateMethod(method: string): "GET" | "POST" {
  if (method === "GET" || method === "POST") return method;
  throw new BoundaryError("method_not_allowed");
}

/** 要求 URL と Host ヘッダーが公開ドメインと一致するか検証する。 */
export function validateRequestUrlAndHost(request: Request, domain: string): URL {
  if (/[\u0000-\u0020\u007f\\]/.test(request.url)) {
    throw new BoundaryError("invalid_request");
  }
  let url: URL;
  try {
    url = new URL(request.url);
  } catch {
    throw new BoundaryError("invalid_request");
  }
  if (
    url.username !== ""
    || url.password !== ""
    || url.pathname.startsWith("//")
    || url.pathname.length + url.search.length > 8192
  ) {
    throw new BoundaryError("invalid_request");
  }
  const incomingHost = request.headers.get("Host");
  if (incomingHost !== null && incomingHost.trim().toLowerCase() !== domain) {
    throw new BoundaryError("unexpected_host");
  }
  if (url.host.toLowerCase() !== domain) {
    throw new BoundaryError("unexpected_host");
  }
  return url;
}

/**
 * POST の本文を検査し、上限サイズと時間上限のもとで Worker 側が最後まで読み切る。
 * 遅い送信や途中切断をコンテナへ一切見せないための中心的な防御。
 */
export async function readBoundedPostBody(
  request: Request,
  maxBytes: number,
  timeoutMs: number,
  clock: DemoClock,
): Promise<Uint8Array<ArrayBuffer>> {
  if (request.headers.has("Transfer-Encoding")) {
    throw new BoundaryError("length_required");
  }
  const rawLength = request.headers.get("Content-Length");
  if (rawLength === null || rawLength.trim() === "") {
    throw new BoundaryError("length_required");
  }
  const trimmed = rawLength.trim();
  if (!/^[0-9]+$/.test(trimmed)) {
    throw new BoundaryError("invalid_request");
  }
  const declaredLength = Number(trimmed);
  if (!Number.isSafeInteger(declaredLength) || declaredLength > maxBytes) {
    throw new BoundaryError("payload_too_large");
  }

  if (request.body === null) {
    if (declaredLength === 0) return new Uint8Array(new ArrayBuffer(0));
    throw new BoundaryError("invalid_request");
  }

  if (request.signal.aborted) {
    throw new BoundaryError("invalid_request");
  }

  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let totalBytes = 0;
  let failure: BoundaryError | undefined;
  let rejectDeadline: ((error: BoundaryError) => void) | undefined;

  const deadlinePromise = new Promise<never>((_, reject) => {
    rejectDeadline = reject;
  });
  // 期限到達前に正常完了したときに未処理拒否が起きないよう先に吸収する。
  void deadlinePromise.catch(() => {});

  const triggerFailure = (code: "request_timeout" | "invalid_request"): void => {
    if (failure) return;
    failure = new BoundaryError(code);
    void reader.cancel().catch(() => {});
    rejectDeadline?.(failure);
  };

  const onAbort = (): void => triggerFailure("invalid_request");
  const timer = clock.setTimeout(() => triggerFailure("request_timeout"), timeoutMs);
  request.signal.addEventListener("abort", onAbort, { once: true });

  try {
    while (true) {
      if (failure) throw failure;
      const readPromise = reader.read();
      // タイムアウト後に遅れて失敗した read() の未処理拒否を防ぐ。
      void readPromise.catch(() => {});
      let next: ReadableStreamReadResult<Uint8Array>;
      try {
        next = await Promise.race([readPromise, deadlinePromise]);
      } catch (error) {
        if (error instanceof BoundaryError) throw error;
        throw failure ?? new BoundaryError("invalid_request");
      }
      if (failure) throw failure;
      if (next.done) break;

      const chunk = next.value;
      if (chunk.byteLength > maxBytes - totalBytes) {
        void reader.cancel().catch(() => {});
        throw new BoundaryError("payload_too_large");
      }
      if (chunk.byteLength > declaredLength - totalBytes) {
        void reader.cancel().catch(() => {});
        throw new BoundaryError("invalid_request");
      }
      chunks.push(chunk);
      totalBytes += chunk.byteLength;
    }

    if (totalBytes !== declaredLength) {
      throw new BoundaryError("invalid_request");
    }

    const buffer = new ArrayBuffer(totalBytes);
    const merged = new Uint8Array(buffer);
    let offset = 0;
    for (const chunk of chunks) {
      merged.set(chunk, offset);
      offset += chunk.byteLength;
    }
    return merged;
  } finally {
    clock.clearTimeout(timer);
    request.signal.removeEventListener("abort", onAbort);
    try {
      reader.releaseLock();
    } catch {
      // cancel 済み等でロックが外れていても無視する。
    }
  }
}

/**
 * コンテナ（または DO）へ渡す要求を組み立てる。
 * URL のホストと Host ヘッダーを公開ドメインに揃え、許可したヘッダーだけを通す。
 * CF-*・X-Forwarded-*・Forwarded・Expect・ホップ単位ヘッダーは一切渡さない。
 */
export function buildForwardedRequest(
  request: Request,
  url: URL,
  domain: string,
  protocol: "http:" | "https:",
  body: Uint8Array<ArrayBuffer> | null,
): Request {
  const headers = new Headers();
  request.headers.forEach((value, name) => {
    const lower = name.toLowerCase();
    if (ALLOWED_FORWARD_HEADERS.has(lower) || lower.startsWith("x-meibo-")) {
      headers.append(name, value);
    }
  });
  headers.set("Host", domain);

  const method = validateMethod(request.method);
  if (method === "POST") {
    const payload = body ?? new Uint8Array(new ArrayBuffer(0));
    headers.set("Content-Length", String(payload.byteLength));
    return new Request(`${protocol}//${domain}${url.pathname}${url.search}`, {
      method,
      headers,
      body: payload,
      redirect: "manual",
    });
  }

  return new Request(`${protocol}//${domain}${url.pathname}${url.search}`, {
    method,
    headers,
    redirect: "manual",
  });
}

/** 静的保持の対象経路なら保持秒数を返す（対象外は undefined）。 */
export function staticCacheTtl(pathname: string, config: DemoConfig): number | undefined {
  if (!Object.hasOwn(config.staticCacheTtlSeconds, pathname)) return undefined;
  return config.staticCacheTtlSeconds[pathname];
}

/** 公開ドメインを含め、クエリを無視した Cache API 用のキーを作る。 */
export function buildStaticCacheKey(domain: string, pathname: string): Request {
  return new Request(`https://${domain}${pathname}`, { method: "GET" });
}

/** 応答に Set-Cookie が含まれるか判定する。 */
export function hasSetCookie(headers: Headers): boolean {
  return headers.has("Set-Cookie") || headers.getSetCookie().length > 0;
}

/** ホップ単位ヘッダーだけを除き、Set-Cookie やセキュリティヘッダーをそのまま保つ。 */
export function stripHopByHopHeaders(original: Headers): Headers {
  const headers = new Headers(original);
  for (const nominated of (original.get("Connection") ?? "").split(",")) {
    const name = nominated.trim();
    if (/^[!#$%&'*+.^_`|~0-9a-z-]+$/i.test(name)) {
      headers.delete(name);
    }
  }
  for (const name of HOP_BY_HOP_HEADERS) {
    headers.delete(name);
  }
  return headers;
}

/** コンテナの応答をそのまま訪問者へ返す形へ整える。 */
export function forwardResponse(response: Response): Response {
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers: stripHopByHopHeaders(response.headers),
  });
}

/** 静的保持用の応答として Cache-Control を付け直した新しい応答を作る。 */
export function withStaticCacheControl(response: Response, ttlSeconds: number): Response {
  const headers = stripHopByHopHeaders(response.headers);
  headers.delete("Pragma");
  headers.delete("Expires");
  headers.set("Cache-Control", `public, max-age=${ttlSeconds}`);
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

function prefersJson(request?: Request): boolean {
  if (!request) return true;
  if (request.method === "POST") return true;
  const accept = request.headers.get("Accept") ?? "";
  if (accept.includes("text/html")) return false;
  if (accept.includes("application/json")) return true;
  return false;
}

function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll("\"", "&quot;");
}

function renderStatusHtml(title: string, message: string): string {
  return `<!doctype html>
<html lang="ja">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(title)}</title>
<style>body{font-family:system-ui,-apple-system,sans-serif;max-width:36rem;margin:4rem auto;padding:0 1.25rem;line-height:1.7;color:#1f2937}h1{font-size:1.25rem;margin:0 0 .75rem}</style>
</head>
<body>
<h1>${escapeHtml(title)}</h1>
<p>${escapeHtml(message)}</p>
</body>
</html>`;
}

function statusResponse(
  status: number,
  code: string,
  title: string,
  message: string,
  request?: Request,
  extraHeaders?: Record<string, string>,
): Response {
  const headers = new Headers({
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
    ...extraHeaders,
  });
  if (prefersJson(request)) {
    headers.set("Content-Type", "application/json; charset=utf-8");
    return new Response(JSON.stringify({ error: message, code }), { status, headers });
  }
  headers.set("Content-Type", "text/html; charset=utf-8");
  return new Response(renderStatusHtml(title, message), { status, headers });
}

/** 停止切替（DEMO_ENABLED !== "true"）時の 503 応答。 */
export function maintenanceResponse(request: Request, retryAfterSeconds: number): Response {
  return statusResponse(
    503,
    "maintenance",
    "メンテナンス中",
    "メンテナンス中です。しばらくしてからお試しください。",
    request,
    { "Retry-After": String(retryAfterSeconds) },
  );
}

/** コンテナ起動待ち超過時の 503 応答。 */
export function startupPendingResponse(request: Request, retryAfterSeconds: number): Response {
  return statusResponse(
    503,
    "starting",
    "デモを起動中です",
    "起動中です。少し待ってからもう一度お試しください。",
    request,
    { "Retry-After": String(retryAfterSeconds) },
  );
}

/** レート制限超過時の 429 応答。 */
export function rateLimitedResponse(request: Request, retryAfterSeconds: number): Response {
  return statusResponse(
    429,
    "rate_limited",
    "アクセスが集中しています",
    "アクセスが集中しています。少し待ってからもう一度お試しください。",
    request,
    { "Retry-After": String(retryAfterSeconds) },
  );
}

/** 境界エラーを固定形式の HTTP 応答へ変換する。 */
export function boundaryErrorResponse(error: unknown, request?: Request): Response {
  const code: BoundaryErrorCode = error instanceof BoundaryError ? error.code : "unavailable";
  const [status, message] = ERROR_DETAILS[code];
  const headers = new Headers({
    "Content-Type": "application/json; charset=utf-8",
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
  });
  if (status === 405) {
    headers.set("Allow", "GET, POST");
  }
  if (request && !prefersJson(request) && status === 503) {
    headers.set("Content-Type", "text/html; charset=utf-8");
    return new Response(renderStatusHtml("一時的に利用できません", message), { status, headers });
  }
  return new Response(JSON.stringify({ error: message, code }), { status, headers });
}
