/** Worker bindings. Database values are registered as secrets, never tracked vars. */
export interface Env extends CloudflareBindings {
  readonly MEIBO_REMOTE_CONFIG: string;
  readonly MEIBO_ACCESS_CONFIG?: string;
  readonly [name: `MEIBO_DB_${string}`]: string | undefined;
}

/** Canonical HTTPS origin and authority, produced by the public boundary. */
export interface ValidatedPublicOrigin {
  readonly origin: string;
  readonly host: string;
}

/** Boundary output. The request body remains unread while waiting to forward. */
export interface ValidatedRequest {
  readonly request: Request;
  readonly origin: ValidatedPublicOrigin;
  readonly method: "GET" | "POST";
  readonly bodyLimitBytes: number;
}

type WorkersContainer = NonNullable<DurableObjectState["container"]>;

/** HTTP-only access to a container port; usable by an injected test runtime. */
export type ContainerPort = Pick<ReturnType<WorkersContainer["getTcpPort"]>, "fetch">;

/** The coordinator's required built-in Container API, without stop or storage access. */
export interface ContainerRuntime {
  readonly running: boolean;
  start(options: { env: Record<string, string>; enableInternet: true }): void;
  getTcpPort(port: number): ContainerPort;
  setInactivityTimeout(durationMs: number | bigint): Promise<void>;
}
