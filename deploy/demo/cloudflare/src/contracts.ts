/** Worker が公開デモ処理で利用する環境バインディング。 */
export interface Env extends CloudflareBindings {}

/** レート制限バインディングの最小契約。試験では差し替え可能。 */
export interface RateLimitBinding {
  limit(options: { key: string }): Promise<{ success: boolean }>;
}

/** 静的応答保持に使う Cache API の最小契約。 */
export interface CacheLike {
  match(request: RequestInfo | URL): Promise<Response | undefined>;
  put(request: RequestInfo | URL, response: Response): Promise<void>;
}

/** バックグラウンド保存用の最小 ExecutionContext 契約。 */
export interface DemoExecutionContext {
  waitUntil(promise: Promise<unknown>): void;
}

/** Durable Object スタブの最小契約。 */
export interface DemoContainerStub {
  fetch(request: Request): Promise<Response>;
}

/** Durable Object 名前空間の最小契約。 */
export interface DemoContainerNamespace {
  getByName(name: string): DemoContainerStub;
}

/** Worker エントリが必要とする環境値とバインディング。 */
export interface DemoWorkerEnv {
  readonly MEIBO_DOMAIN: string;
  readonly DEMO_ENABLED: string;
  readonly MEIBO_DEMO_CONTACT?: string | undefined;
  readonly DEMO_CONTAINER: DemoContainerNamespace;
  readonly RATE_LIMIT_DEMO_ENTER: RateLimitBinding;
  readonly RATE_LIMIT_POST: RateLimitBinding;
  readonly RATE_LIMIT_GET: RateLimitBinding;
}

/** Durable Object 側がコンテナ起動に使う環境値。 */
export interface DemoContainerEnv {
  readonly MEIBO_DOMAIN: string;
  readonly MEIBO_DEMO_CONTACT?: string | undefined;
}

/** 組み込みタイマーのハンドル型。 */
export type TimerHandle = ReturnType<typeof setTimeout>;

/** 時計とタイマーの抽象。試験で時刻進行を差し込めるようにする。 */
export interface DemoClock {
  now(): number;
  setTimeout(callback: () => void, delayMs: number): TimerHandle;
  clearTimeout(handle: TimerHandle): void;
}

type WorkersContainer = NonNullable<DurableObjectState["container"]>;

/** コンテナの TCP ポートへ HTTP 転送する最小契約。 */
export type ContainerPort = Pick<ReturnType<WorkersContainer["getTcpPort"]>, "fetch">;

/** Durable Object Container API のうち公開デモが使う最小契約。 */
export interface ContainerRuntime {
  readonly running: boolean;
  start(options: { env: Record<string, string>; enableInternet: false }): void;
  monitor(): Promise<void>;
  destroy(error?: unknown): Promise<void>;
  getTcpPort(port: number): ContainerPort;
  setInactivityTimeout?(durationMs: number | bigint): Promise<void>;
}

/**
 * アイドル停止用のアラーム操作だけに絞った契約。
 * 営業データや Cookie を Durable Object の保存領域へ書かないことを型でも保証する。
 */
export interface AlarmStorage {
  setAlarm(scheduledTime: number | Date): Promise<void>;
  deleteAlarm?(): Promise<void>;
}

/** レート制限バインディング一式（試験での部分差し替えにも対応）。 */
export interface DemoRateLimiters {
  readonly demoEnter: RateLimitBinding;
  readonly post: RateLimitBinding;
  readonly get: RateLimitBinding;
}

/** 公開デモ Worker / Durable Object の動作設定。 */
export interface DemoConfig {
  readonly instanceName: string;
  readonly containerPort: number;
  readonly maxPostBodyBytes: number;
  readonly bodyReadTimeoutMs: number;
  readonly maintenanceRetryAfterSeconds: number;
  readonly rateLimitRetryAfterSeconds: number;
  readonly startupRetryAfterSeconds: number;
  readonly startupTimeoutMs: number;
  readonly startupPollIntervalMs: number;
  readonly idleTimeoutMs: number;
  readonly staticCacheTtlSeconds: Readonly<Record<string, number>>;
}

/** Worker 試験向けに実行環境を差し込む依存セット。 */
export interface DemoWorkerDependencies {
  readonly cache?: CacheLike | undefined;
  readonly clock?: DemoClock | undefined;
  readonly config?: Partial<DemoConfig> | undefined;
  readonly rateLimiters?: Partial<DemoRateLimiters> | undefined;
}
