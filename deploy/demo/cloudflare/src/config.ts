import type { DemoClock, DemoConfig } from "./contracts.ts";

/** 静的応答を Cache API で保持する経路と保持秒数（クエリは無視する）。 */
export const DEFAULT_STATIC_CACHE_TTL_SECONDS: Readonly<Record<string, number>> = Object.freeze({
  "/login": 300,
  "/login.css": 3600,
  "/login.js": 3600,
  "/public-entry.js": 3600,
  "/public-entry.png": 3600,
});

/** 公開デモの既定設定。試験では個別の項目だけ上書きできる。 */
export const DEFAULT_DEMO_CONFIG: DemoConfig = Object.freeze({
  instanceName: "meibo-demo-v1",
  containerPort: 4317,
  maxPostBodyBytes: 64 * 1024,
  bodyReadTimeoutMs: 5_000,
  maintenanceRetryAfterSeconds: 300,
  rateLimitRetryAfterSeconds: 60,
  startupRetryAfterSeconds: 5,
  startupTimeoutMs: 25_000,
  startupPollIntervalMs: 200,
  idleTimeoutMs: 15 * 60 * 1000,
  staticCacheTtlSeconds: DEFAULT_STATIC_CACHE_TTL_SECONDS,
});

/** 本番実行時の既定クロック。 */
export const SYSTEM_CLOCK: DemoClock = {
  now: () => Date.now(),
  setTimeout: (callback, delayMs) => setTimeout(callback, delayMs),
  clearTimeout: (handle) => clearTimeout(handle),
};

/** 既定設定へ任意の上書きを合成する。 */
export function resolveDemoConfig(overrides?: Partial<DemoConfig>): DemoConfig {
  if (!overrides) return DEFAULT_DEMO_CONFIG;
  return Object.freeze({
    ...DEFAULT_DEMO_CONFIG,
    ...overrides,
    staticCacheTtlSeconds: overrides.staticCacheTtlSeconds
      ? Object.freeze({ ...overrides.staticCacheTtlSeconds })
      : DEFAULT_STATIC_CACHE_TTL_SECONDS,
  });
}
