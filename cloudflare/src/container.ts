import type { ContainerRuntime, Env, ValidatedPublicOrigin, ValidatedRequest } from "./contracts.ts";

const PORT = 4317;
const WAITING_LIMIT = 16;
const QUEUE_DEADLINE_MS = 30_000;
const STARTUP_DEADLINE_MS = 30_000;
const FORWARD_DEADLINE_MS = 180_000;
const RESPONSE_LIMIT_BYTES = 33 * 1024 * 1024;
const BUSY_INACTIVITY_MS = 180_000;
const IDLE_INACTIVITY_MS = 60_000;
const CONFIG_LIMIT_BYTES = 1024 * 1024;

const MESSAGES = {
  unavailable: "処理を完了できませんでした。",
  invalid_configuration: "公開環境の設定を確認してください。",
  outcome_unknown: "処理結果を確認できません。再送せず管理者へ連絡してください。",
} as const;

/** Contains only fixed public information, never the underlying exception. */
class CoordinatorError extends Error {
  readonly code: keyof typeof MESSAGES;
  constructor(code: keyof typeof MESSAGES) {
    super(MESSAGES[code]);
    this.name = "CoordinatorError";
    this.code = code;
  }
}

export interface CoordinatorClock {
  now(): number;
  setTimeout(callback: () => void, delayMs: number): number;
  clearTimeout(handle: number): void;
}

type RuntimeBindings = Omit<Env, "MEIBO_CONTAINER">;
export type KnownErrorResponse = (error: unknown) => Response | undefined;

const SYSTEM_CLOCK: CoordinatorClock = {
  now: () => Date.now(),
  setTimeout: (callback, delayMs) => setTimeout(callback, delayMs),
  clearTimeout: (handle) => clearTimeout(handle),
};

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function exactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value);
  return actual.length === keys.length && keys.every((key) => Object.hasOwn(value, key));
}

/** The Worker validates references, while Rust validates database identity/readiness. */
export function runtimeEnvironment(env: RuntimeBindings, origin: ValidatedPublicOrigin): Record<string, string> {
  try {
    const config = env.MEIBO_REMOTE_CONFIG;
    if (typeof config !== "string" || config.length === 0 || config.length > CONFIG_LIMIT_BYTES
      || new TextEncoder().encode(config).byteLength > CONFIG_LIMIT_BYTES) {
      throw new CoordinatorError("invalid_configuration");
    }
    if (typeof env.MEIBO_ORIGIN !== "string") throw new CoordinatorError("invalid_configuration");
    const configuredOrigin = new URL(env.MEIBO_ORIGIN);
    if (configuredOrigin.protocol !== "https:" || configuredOrigin.origin !== origin.origin
      || (env.MEIBO_ORIGIN !== configuredOrigin.origin && env.MEIBO_ORIGIN !== `${configuredOrigin.origin}:443`)) {
      throw new CoordinatorError("invalid_configuration");
    }
    const parsed: unknown = JSON.parse(config);
    if (!record(parsed) || !exactKeys(parsed, ["version", "control", "tenants"])
      || parsed.version !== 1 || !record(parsed.tenants) || Object.keys(parsed.tenants).length > 1000) {
      throw new CoordinatorError("invalid_configuration");
    }
    const output: Record<string, string> = { MEIBO_ORIGIN: origin.origin, MEIBO_REMOTE_CONFIG: config };
    if (Object.hasOwn(env, "MEIBO_ACCESS_CONFIG")) {
      const access = env.MEIBO_ACCESS_CONFIG;
      if (typeof access !== "string" || access.length === 0 || new TextEncoder().encode(access).byteLength > 32 * 1024) {
        throw new CoordinatorError("invalid_configuration");
      }
      const parsedAccess: unknown = JSON.parse(access);
      if (!record(parsedAccess) || parsedAccess.version !== 1) throw new CoordinatorError("invalid_configuration");
      // Rust owns the complete identity/issuer validation before it opens storage.
      output.MEIBO_ACCESS_CONFIG = access;
    }
    const references = new Set<string>();
    function databaseReference(value: unknown): void {
      if (!record(value) || !exactKeys(value, ["url_env", "token_env"])) {
        throw new CoordinatorError("invalid_configuration");
      }
      for (const kind of ["url_env", "token_env"]) {
        const name = value[kind];
        if (typeof name !== "string" || !/^MEIBO_DB_[A-Z0-9_]+$/.test(name) || references.has(name)) {
          throw new CoordinatorError("invalid_configuration");
        }
        references.add(name);
        const secret: unknown = env[name as `MEIBO_DB_${string}`];
        if (!Object.hasOwn(env, name) || typeof secret !== "string" || secret.length === 0) {
          throw new CoordinatorError("invalid_configuration");
        }
        output[name] = secret;
      }
    }
    databaseReference(parsed.control);
    for (const [slug, database] of Object.entries(parsed.tenants)) {
      if (slug.length < 1 || slug.length > 48 || !/^[a-z0-9]+(?:[a-z0-9-]*[a-z0-9])?$/.test(slug)) {
        throw new CoordinatorError("invalid_configuration");
      }
      databaseReference(database);
    }
    return output;
  } catch {
    throw new CoordinatorError("invalid_configuration");
  }
}

interface Waiter {
  readonly resolve: (release: () => void) => void;
  readonly signal: AbortSignal;
  readonly abort: () => void;
  readonly timer: number;
}

function errorResponse(code: "unavailable" | "outcome_unknown"): Response {
  return new Response(JSON.stringify({ error: MESSAGES[code], code }), {
    status: 503,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    },
  });
}

/** One serialized HTTP path; persistent cross-process exclusion remains in Rust. */
export class ContainerCoordinator {
  private readonly runtime: ContainerRuntime;
  private readonly env: RuntimeBindings;
  private readonly origin: ValidatedPublicOrigin;
  private readonly clock: CoordinatorClock;
  private active = false;
  private readonly waiting: Waiter[] = [];
  private readiness: Promise<void> | undefined;

  constructor(runtime: ContainerRuntime, env: RuntimeBindings, origin: ValidatedPublicOrigin, clock = SYSTEM_CLOCK) {
    this.runtime = runtime;
    this.env = env;
    this.origin = origin;
    this.clock = clock;
  }

  /** Prepare is invoked only after admission and readiness; it may wrap a body lazily. */
  async forward(request: ValidatedRequest, prepare: () => Request, knownError?: KnownErrorResponse): Promise<Response> {
    let release: (() => void) | undefined;
    let attempted = false;
    try {
      release = await this.acquire(request.request.signal);
      if (request.request.signal.aborted) throw new CoordinatorError("unavailable");
      await this.ensureReady();
      if (request.request.signal.aborted) throw new CoordinatorError("unavailable");
      const response = await this.deadline(async (signal, expiresAt) => {
        let knownCompletion = false;
        try {
          await this.runtime.setInactivityTimeout(BUSY_INACTIVITY_MS);
          if (signal.aborted) throw new CoordinatorError("outcome_unknown");
          const prepared = prepare();
          if (prepared.signal.aborted) throw new CoordinatorError("unavailable");
          attempted = true;
          const upstream = await this.runtime.getTcpPort(PORT).fetch(prepared, { signal });
          const buffered = await this.readResponse(upstream, signal, expiresAt);
          knownCompletion = true;
          return buffered;
        } catch (error) {
          // Only trusted composition can recognize a concrete boundary error instance.
          const classified = this.classify(error, knownError);
          if (classified) { knownCompletion = true; return classified; }
          if (!attempted) { knownCompletion = true; return errorResponse("unavailable"); }
          throw error;
        } finally {
          if (knownCompletion && !signal.aborted && this.clock.now() < expiresAt && this.waiting.length === 0) {
            await this.runtime.setInactivityTimeout(IDLE_INACTIVITY_MS);
          }
        }
      }, FORWARD_DEADLINE_MS, "outcome_unknown", request.request.signal);
      return response;
    } catch {
      return errorResponse(attempted ? "outcome_unknown" : "unavailable");
    } finally {
      release?.();
    }
  }

  private classify(error: unknown, knownError?: KnownErrorResponse): Response | undefined {
    try {
      return knownError?.(error);
    } catch {
      return undefined;
    }
  }

  private acquire(signal: AbortSignal): Promise<() => void> {
    if (signal.aborted) return Promise.reject(new CoordinatorError("unavailable"));
    if (!this.active) {
      this.active = true;
      return Promise.resolve(this.releaseOnce());
    }
    if (this.waiting.length >= WAITING_LIMIT) return Promise.reject(new CoordinatorError("unavailable"));
    return new Promise((resolve, reject) => {
      const remove = () => {
        const index = this.waiting.indexOf(waiter);
        if (index === -1) return;
        this.waiting.splice(index, 1);
        this.clock.clearTimeout(waiter.timer);
        signal.removeEventListener("abort", waiter.abort);
        reject(new CoordinatorError("unavailable"));
      };
      const waiter: Waiter = {
        resolve, signal, abort: remove,
        timer: this.clock.setTimeout(remove, QUEUE_DEADLINE_MS),
      };
      this.waiting.push(waiter);
      signal.addEventListener("abort", remove, { once: true });
      if (signal.aborted) remove();
    });
  }

  private releaseOnce(): () => void {
    let released = false;
    return () => {
      if (released) return;
      released = true;
      const next = this.waiting.shift();
      if (next) {
        this.clock.clearTimeout(next.timer);
        next.signal.removeEventListener("abort", next.abort);
        next.resolve(this.releaseOnce());
      } else {
        this.active = false;
      }
    };
  }

  private ensureReady(): Promise<void> {
    if (!this.runtime.running) this.readiness = undefined;
    if (!this.readiness) {
      const pending = this.deadline(async (signal, expiresAt) => {
        const env = runtimeEnvironment(this.env, this.origin);
        if (!this.runtime.running) this.runtime.start({ env, enableInternet: true });
        await this.runtime.setInactivityTimeout(BUSY_INACTIVITY_MS);
        while (!signal.aborted && this.clock.now() < expiresAt) {
          try {
            const health = new Request(`http://${this.origin.host}/health`, {
              method: "GET", headers: { Host: this.origin.host }, redirect: "manual", signal,
            });
            const response = await this.runtime.getTcpPort(PORT).fetch(health, { signal });
            if (response.body) void response.body.cancel().catch(() => {});
            if (response.status === 200) return;
          } catch {
            if (signal.aborted) throw new CoordinatorError("unavailable");
          }
          await this.delay(100, signal);
        }
        throw new CoordinatorError("unavailable");
      }, STARTUP_DEADLINE_MS, "unavailable");
      this.readiness = pending;
      // Catch is installed immediately, including if an aborted client never awaits again.
      void pending.catch(() => {
        if (this.readiness === pending) this.readiness = undefined;
      });
    }
    return this.readiness;
  }

  private delay(duration: number, signal: AbortSignal): Promise<void> {
    return new Promise((resolve, reject) => {
      const abort = () => {
        this.clock.clearTimeout(timer);
        reject(new CoordinatorError("unavailable"));
      };
      const timer = this.clock.setTimeout(() => {
        signal.removeEventListener("abort", abort);
        resolve();
      }, duration);
      signal.addEventListener("abort", abort, { once: true });
      if (signal.aborted) abort();
    });
  }

  private deadline<T>(task: (signal: AbortSignal, expiresAt: number) => Promise<T>, duration: number,
    failure: "unavailable" | "outcome_unknown", externalSignal?: AbortSignal): Promise<T> {
    return new Promise((resolve, reject) => {
      const controller = new AbortController();
      const expiresAt = this.clock.now() + duration;
      let settled = false;
      const cleanup = () => {
        this.clock.clearTimeout(timer);
        externalSignal?.removeEventListener("abort", abort);
      };
      const abort = () => {
        if (settled) return;
        settled = true;
        cleanup();
        controller.abort(new CoordinatorError(failure));
        reject(new CoordinatorError(failure));
      };
      const timer = this.clock.setTimeout(abort, duration);
      externalSignal?.addEventListener("abort", abort, { once: true });
      if (externalSignal?.aborted) { abort(); return; }
      Promise.resolve().then(() => task(controller.signal, expiresAt)).then((value) => {
        if (settled) return;
        if (this.clock.now() >= expiresAt) { abort(); return; }
        settled = true;
        cleanup();
        resolve(value);
      }, (error: unknown) => {
        if (settled) return;
        settled = true;
        cleanup();
        controller.abort(new CoordinatorError(failure));
        reject(error);
      });
    });
  }

  private async readResponse(response: Response, signal: AbortSignal, expiresAt: number): Promise<Response> {
    const headers = new Headers(response.headers);
    headers.delete("Content-Length");
    if (!response.body) return new Response(null, { status: response.status, statusText: response.statusText, headers });
    const reader = response.body.getReader();
    // Retain at most 528 blocks, independent of the number of network chunks.
    const blocks: Uint8Array[] = [];
    let block: Uint8Array | undefined;
    let used = 0;
    let size = 0;
    const cancel = () => { void reader.cancel().catch(() => {}); };
    signal.addEventListener("abort", cancel, { once: true });
    try {
      while (true) {
        if (signal.aborted || this.clock.now() >= expiresAt) { cancel(); throw new CoordinatorError("outcome_unknown"); }
        const next = await reader.read();
        if (signal.aborted || this.clock.now() >= expiresAt) { cancel(); throw new CoordinatorError("outcome_unknown"); }
        if (next.done) break;
        if (next.value.byteLength > RESPONSE_LIMIT_BYTES - size) {
          cancel();
          throw new CoordinatorError("outcome_unknown");
        }
        size += next.value.byteLength;
        let offset = 0;
        while (offset < next.value.byteLength) {
          if (!block || used === block.byteLength) {
            block = new Uint8Array(64 * 1024);
            blocks.push(block);
            used = 0;
          }
          const count = Math.min(block.byteLength - used, next.value.byteLength - offset);
          block.set(next.value.subarray(offset, offset + count), used);
          offset += count;
          used += count;
        }
      }
    } finally {
      signal.removeEventListener("abort", cancel);
      reader.releaseLock();
    }
    if (block && used < block.byteLength) blocks[blocks.length - 1] = block.subarray(0, used);
    return new Response(new Blob(blocks), { status: response.status, statusText: response.statusText, headers });
  }
}
