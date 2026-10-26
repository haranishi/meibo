import type { ValidatedPublicOrigin, ValidatedRequest } from "./contracts.ts";

const JSON_LIMIT_BYTES = 1024 * 1024;
const CSV_LIMIT_BYTES = 32 * JSON_LIMIT_BYTES;
const ERROR_DETAILS = {
  invalid_configuration: [503, "処理を完了できませんでした。"],
  invalid_request: [400, "要求を確認してください。"],
  unexpected_origin: [403, "公開URLからアクセスしてください。"],
  method_not_allowed: [405, "この操作には対応していません。"],
  payload_too_large: [413, "送信内容が上限を超えています。"],
  unavailable: [503, "処理を完了できませんでした。"],
} as const;

export class BoundaryError extends Error {
  readonly code: keyof typeof ERROR_DETAILS;
  readonly status: number;
  constructor(code: keyof typeof ERROR_DETAILS) {
    super(ERROR_DETAILS[code][1]);
    this.name = "BoundaryError";
    this.code = code;
    this.status = ERROR_DETAILS[code][0];
  }
}

/** Accept the lowercase HTTPS authority understood by the hosted Rust service. */
export function validatePublicOrigin(value: string): ValidatedPublicOrigin {
  try {
    const authority = value.slice("https://".length);
    if (!value.startsWith("https://") || authority.length > 253
      || !/^[a-z0-9.-]+(?::[1-9][0-9]{0,4})?$/.test(authority)) {
      throw new BoundaryError("invalid_configuration");
    }
    const url = new URL(value);
    if ((value !== url.origin && value !== `${url.origin}:443`)
      || url.hostname.split(".").some((part) => !part || part.length > 63 || part.startsWith("-") || part.endsWith("-"))) {
      throw new BoundaryError("invalid_configuration");
    }
    return Object.freeze({ origin: url.origin, host: url.host });
  } catch {
    throw new BoundaryError("invalid_configuration");
  }
}

/** Metadata only: no reader, wrapper stream, clone or body consumption while queued. */
export function validateRequest(request: Request, origin: ValidatedPublicOrigin): ValidatedRequest {
  let url: URL;
  try {
    url = new URL(request.url);
  } catch {
    throw new BoundaryError("invalid_request");
  }
  if (/[\u0000-\u0020\u007f]/.test(request.url)) throw new BoundaryError("invalid_request");
  if (url.protocol !== "https:" || url.origin !== origin.origin || url.host !== origin.host
    || url.username || url.password || url.hash || request.url.includes("\\")
    || request.headers.get("Host") !== origin.host) {
    throw new BoundaryError("unexpected_origin");
  }
  if (url.pathname.startsWith("//") || url.pathname.length + url.search.length > 8192) {
    throw new BoundaryError("invalid_request");
  }
  const method = request.method;
  if (method !== "GET" && method !== "POST") throw new BoundaryError("method_not_allowed");
  const bodyLimitBytes = url.pathname === "/api/import" ? CSV_LIMIT_BYTES : JSON_LIMIT_BYTES;
  const declared = request.headers.get("Content-Length");
  if (declared !== null) {
    if (!/^[0-9]+$/.test(declared)) throw new BoundaryError("invalid_request");
    if (Number(declared) > bodyLimitBytes) throw new BoundaryError("payload_too_large");
  }
  return { request, origin, method, bodyLimitBytes };
}

const HOP_BY_HOP = [
  "Connection", "Keep-Alive", "Proxy-Authenticate", "Proxy-Authorization",
  "Proxy-Connection", "TE", "Trailer", "Transfer-Encoding", "Upgrade",
];

function endToEndHeaders(original: Headers): Headers {
  const headers = new Headers(original);
  for (const nominated of (original.get("Connection") ?? "").split(",")) {
    const name = nominated.trim();
    if (/^[!#$%&'*+.^_`|~0-9a-z-]+$/i.test(name)) headers.delete(name);
  }
  for (const name of HOP_BY_HOP) headers.delete(name);
  return headers;
}

/** The source reader is acquired only when the forwarded body is actually read. */
function boundedBody(input: ReadableStream<Uint8Array>, limit: number): ReadableStream<Uint8Array> {
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
  let bytes = 0;
  function release(): void {
    reader?.releaseLock();
    reader = undefined;
  }
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        reader ??= input.getReader();
        const next = await reader.read();
        if (next.done) {
          release();
          controller.close();
        } else if (next.value.byteLength > limit - bytes) {
          const failure = new BoundaryError("payload_too_large");
          controller.error(failure);
          void reader.cancel().catch(() => {});
          release();
        } else {
          bytes += next.value.byteLength;
          controller.enqueue(next.value);
        }
      } catch {
        release();
        controller.error(new BoundaryError("invalid_request"));
      }
    },
    cancel() {
      if (!reader) return input.cancel();
      const cancelled = reader.cancel();
      release();
      return cancelled;
    },
  }, { highWaterMark: 0 });
}

/** Call after queue admission. Rust keeps Origin/CSRF/context/content-type authority. */
export function prepareContainerRequest(validated: ValidatedRequest): Request {
  const { request, origin, method, bodyLimitBytes } = validated;
  const url = new URL(request.url);
  const headers = endToEndHeaders(request.headers);
  for (const name of Array.from(headers.keys())) {
    const lower = name.toLowerCase();
    if (lower === "forwarded" || lower === "x-forwarded" || lower.startsWith("x-forwarded-")) headers.delete(name);
  }
  headers.set("Host", origin.host);
  // Never allow a false length to dispatch a valid prefix of an oversized body.
  headers.delete("Content-Length");
  // Node's built-in Fetch requires duplex for streams; Workers ignores this member.
  const init: RequestInit & { duplex: "half" } = {
    method, headers, redirect: "manual", signal: request.signal,
    body: request.body === null ? null : boundedBody(request.body, bodyLimitBytes),
    duplex: "half",
  };
  return new Request(`http://${origin.host}${url.pathname}${url.search}`, init);
}

/** Retains separate Set-Cookie values and application security/cache headers. */
export function forwardResponse(response: Response): Response {
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers: endToEndHeaders(response.headers),
  });
}

/** Only fixed public errors are serialized; exception causes and bindings stay private. */
export function boundaryErrorResponse(error: unknown): Response {
  const code = error instanceof BoundaryError ? error.code : "unavailable";
  const [status, message] = ERROR_DETAILS[code];
  const headers = new Headers({
    "Content-Type": "application/json; charset=utf-8",
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
  });
  if (status === 405) headers.set("Allow", "GET, POST");
  return new Response(JSON.stringify({ error: message, code }), { status, headers });
}
