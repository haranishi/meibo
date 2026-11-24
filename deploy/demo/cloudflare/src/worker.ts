import {
  boundaryErrorResponse,
  buildForwardedRequest,
  buildStaticCacheKey,
  forwardResponse,
  hasSetCookie,
  maintenanceResponse,
  normalizeRateLimitClientIp,
  rateLimitedResponse,
  readBoundedPostBody,
  staticCacheTtl,
  validateDemoDomain,
  validateMethod,
  validateRequestUrlAndHost,
  withStaticCacheControl,
} from "./boundary.ts";
import { resolveDemoConfig, SYSTEM_CLOCK } from "./config.ts";
import type {
  CacheLike,
  DemoExecutionContext,
  DemoRateLimiters,
  DemoWorkerDependencies,
  DemoWorkerEnv,
  RateLimitBinding,
} from "./contracts.ts";

function defaultCache(): CacheLike | undefined {
  return typeof caches !== "undefined" ? caches.default : undefined;
}

function resolveRateLimiters(
  env: DemoWorkerEnv,
  overrides?: Partial<DemoRateLimiters>,
): DemoRateLimiters {
  return {
    demoEnter: overrides?.demoEnter ?? env.RATE_LIMIT_DEMO_ENTER,
    post: overrides?.post ?? env.RATE_LIMIT_POST,
    get: overrides?.get ?? env.RATE_LIMIT_GET,
  };
}

function selectRateLimiter(
  method: "GET" | "POST",
  pathname: string,
  limiters: DemoRateLimiters,
): { bucket: string; limiter: RateLimitBinding } {
  if (method === "POST" && pathname === "/demo/enter") {
    return { bucket: "enter", limiter: limiters.demoEnter };
  }
  if (method === "POST") {
    return { bucket: "post", limiter: limiters.post };
  }
  return { bucket: "get", limiter: limiters.get };
}

/** IPv6 の接続元は /64 に丸める（normalizeRateLimitClientIp）。IPv4 と不明は従来どおり。 */
export function rateLimitKey(request: Request, bucket: string): string {
  return `${bucket}:${normalizeRateLimitClientIp(request.headers.get("CF-Connecting-IP"))}`;
}

/**
 * 公開デモ用 Worker の入口ハンドラを生成する。
 * キャッシュ・時計・設定・レート制限を差し込めるため、Node 単体試験でもそのまま検証できる。
 */
export function createDemoWorker(dependencies: DemoWorkerDependencies = {}) {
  const config = resolveDemoConfig(dependencies.config);
  const clock = dependencies.clock ?? SYSTEM_CLOCK;

  return {
    async fetch(
      request: Request,
      env: DemoWorkerEnv,
      ctx?: DemoExecutionContext,
    ): Promise<Response> {
      try {
        // 1. 停止の切替（コンテナ・キャッシュ・レート制限に一切触れない）
        if (env.DEMO_ENABLED !== "true") {
          return maintenanceResponse(request, config.maintenanceRetryAfterSeconds);
        }

        // 2. method（GET / POST のみ）
        const method = validateMethod(request.method);

        // 公開ドメイン設定と要求 URL / Host の検証
        const domain = validateDemoDomain(env.MEIBO_DOMAIN);
        const url = validateRequestUrlAndHost(request, domain);

        // 3. POST の本文を上限付きで最後まで読み切る（遅い送信をコンテナへ渡さない）
        const postBody = method === "POST"
          ? await readBoundedPostBody(
            request,
            config.maxPostBodyBytes,
            config.bodyReadTimeoutMs,
            clock,
          )
          : null;

        // 5（先読み）. 静的な GET 応答のキャッシュ参照。
        // 保持済みの応答はコンテナを起こさず返し、コンテナ到達 GET の制限も消費しない。
        const cache = dependencies.cache ?? defaultCache();
        const cacheTtl = method === "GET" ? staticCacheTtl(url.pathname, config) : undefined;
        const cacheKey = cacheTtl !== undefined ? buildStaticCacheKey(domain, url.pathname) : undefined;

        if (cache && cacheKey) {
          try {
            const cached = await cache.match(cacheKey);
            if (cached) {
              return forwardResponse(cached);
            }
          } catch {
            // Cache API が使えない環境では通常転送へ進める。
          }
        }

        // 4. 接続元ごとの頻度制限（POST /demo/enter・その他 POST・コンテナへ届く GET）
        const limiters = resolveRateLimiters(env, dependencies.rateLimiters);
        const { bucket, limiter } = selectRateLimiter(method, url.pathname, limiters);
        const outcome = await limiter.limit({ key: rateLimitKey(request, bucket) });
        if (!outcome.success) {
          return rateLimitedResponse(request, config.rateLimitRetryAfterSeconds);
        }

        // 6. 固定名の Durable Object 経由でコンテナへ転送
        const stub = env.DEMO_CONTAINER.getByName(config.instanceName);
        const forwardedRequest = buildForwardedRequest(request, url, domain, "https:", postBody);
        const upstream = await stub.fetch(forwardedRequest);

        // 7. 応答（静的経路かつ 200 かつ Set-Cookie なしのみ Cache-Control を付け直して保持）
        if (
          cache
          && cacheKey
          && cacheTtl !== undefined
          && upstream.status === 200
          && !hasSetCookie(upstream.headers)
        ) {
          const cacheable = withStaticCacheControl(upstream, cacheTtl);
          const putPromise = cache.put(cacheKey, cacheable.clone()).catch(() => {});
          if (ctx) {
            ctx.waitUntil(putPromise);
          } else {
            await putPromise;
          }
          return cacheable;
        }

        return forwardResponse(upstream);
      } catch (error) {
        return boundaryErrorResponse(error, request);
      }
    },
  };
}
