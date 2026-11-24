import {
  BoundaryError,
  boundaryErrorResponse,
  buildForwardedRequest,
  forwardResponse,
  isValidContactUrl,
  readBoundedPostBody,
  startupPendingResponse,
  validateDemoDomain,
  validateMethod,
  validateRequestUrlAndHost,
} from "./boundary.ts";
import { resolveDemoConfig, SYSTEM_CLOCK } from "./config.ts";
import type {
  AlarmStorage,
  ContainerRuntime,
  DemoClock,
  DemoConfig,
  DemoContainerEnv,
} from "./contracts.ts";

/**
 * 保存先のアラームを設定し直す間隔の下限。設定済みのアラームより、新しい期限がこれ以上先になったときだけ書き込む。
 * DO が退避したあとにアラームが発火しても、最後の要求から（アイドル時間 - この値）以上たっていることを保つ。
 */
export const ALARM_REFRESH_SLACK_MS = 2 * 60 * 1000;

/** コンテナの /health 準備待ちが上限時間内に終わらなかったエラー。 */
class StartupTimeoutError extends Error {
  constructor() {
    super("container_startup_timeout");
    this.name = "StartupTimeoutError";
  }
}

/** コンテナへ渡す環境変数を許可一覧だけで組み立てる（秘密や他設定を渡さない）。 */
export function buildContainerEnvironment(
  env: DemoContainerEnv,
  domain: string,
  port: number,
): Record<string, string> {
  const containerEnv: Record<string, string> = {
    MEIBO_DOMAIN: domain,
    PORT: String(port),
  };
  const contact = typeof env.MEIBO_DEMO_CONTACT === "string" ? env.MEIBO_DEMO_CONTACT.trim() : "";
  if (contact.length > 0) {
    if (!isValidContactUrl(contact)) {
      throw new BoundaryError("invalid_configuration");
    }
    containerEnv.MEIBO_DEMO_CONTACT = contact;
  }
  return containerEnv;
}

/**
 * 公開デモ用コンテナの起動・単一飛行・終了検知・アイドル停止を管理する制御器。
 * Durable Object の保存領域へ営業データや Cookie は一切書き込まず、アラームだけを使う。
 */
export class DemoContainerController {
  private readonly runtime: ContainerRuntime;
  private readonly alarmStorage: AlarmStorage;
  private readonly env: DemoContainerEnv;
  private readonly config: DemoConfig;
  private readonly clock: DemoClock;
  private readyPromise: Promise<void> | undefined;
  private destroying: Promise<void> | undefined;
  private runGeneration = 0;
  private monitoredGeneration = 0;
  private exitedGeneration = 0;
  private idleDeadlineMs: number | undefined;
  /** 保存先に設定済みのアラームの時刻（メモリ上の印）。DO が退避すると消え、次の要求で設定し直す。 */
  private armedAlarmMs: number | undefined;

  constructor(
    runtime: ContainerRuntime,
    alarmStorage: AlarmStorage,
    env: DemoContainerEnv,
    config?: Partial<DemoConfig>,
    clock: DemoClock = SYSTEM_CLOCK,
  ) {
    this.runtime = runtime;
    this.alarmStorage = alarmStorage;
    this.env = env;
    this.config = resolveDemoConfig(config);
    this.clock = clock;

    // DO 再起動時にコンテナが動き続けていれば終了監視を再接続する。
    if (this.runtime.running) {
      this.runGeneration = 1;
      this.attachMonitor(this.runGeneration);
    }
  }

  /** 要求をコンテナへ転送する。必要なら起動・準備待ちを行い、アイドル停止アラームを延ばす。 */
  async handleFetch(request: Request): Promise<Response> {
    try {
      const domain = validateDemoDomain(this.env.MEIBO_DOMAIN);
      const method = validateMethod(request.method);
      const url = validateRequestUrlAndHost(request, domain);

      // DO 境界でも POST 本文を上限付きで受け切り、Content-Length 明示でコンテナへ渡す。
      const body = method === "POST"
        ? await readBoundedPostBody(
          request,
          this.config.maxPostBodyBytes,
          this.config.bodyReadTimeoutMs,
          this.clock,
        )
        : null;

      await this.extendIdleAlarm();
      await this.ensureReady(domain);

      const containerRequest = buildForwardedRequest(request, url, domain, "http:", body);
      const upstream = await this.runtime.getTcpPort(this.config.containerPort).fetch(containerRequest);
      return forwardResponse(upstream);
    } catch (error) {
      if (error instanceof BoundaryError) {
        return boundaryErrorResponse(error, request);
      }
      if (error instanceof StartupTimeoutError) {
        return startupPendingResponse(request, this.config.startupRetryAfterSeconds);
      }
      if (!this.runtime.running) {
        this.readyPromise = undefined;
        return startupPendingResponse(request, this.config.startupRetryAfterSeconds);
      }
      return boundaryErrorResponse(new BoundaryError("unavailable"), request);
    }
  }

  /** アイドル停止アラーム発火時の処理。最後の要求から設定時間が過ぎていればコンテナを止める。 */
  async handleAlarm(): Promise<void> {
    // 発火したアラームは消費済み。次に要るときは設定し直す。
    this.armedAlarmMs = undefined;
    if (!this.runtime.running) {
      this.readyPromise = undefined;
      this.idleDeadlineMs = undefined;
      return;
    }
    const now = this.clock.now();
    if (this.idleDeadlineMs !== undefined && now < this.idleDeadlineMs) {
      await this.armAlarm(this.idleDeadlineMs);
      return;
    }
    this.readyPromise = undefined;
    this.idleDeadlineMs = undefined;
    const stopPromise = this.runtime.destroy();
    this.destroying = stopPromise;
    try {
      await stopPromise;
    } finally {
      if (this.destroying === stopPromise) {
        this.destroying = undefined;
      }
    }
  }

  /**
   * アイドル期限をメモリ上で延ばす。保存先への書き込み（setAlarm）は、アラームが未設定か、
   * 設定済みのアラームより新しい期限が ALARM_REFRESH_SLACK_MS 以上先のときだけ行う（要求のたびには書き込まない）。
   * 発火時（handleAlarm）に期限がまだ先なら、その期限で設定し直す。DO が退避して印が消えたら、次の要求で設定し直す。
   * 退避したあとに要求がないまま発火した場合は、メモリ上の期限が無いので止める（アプリが固まったときの保険）。
   * このとき止めるのは、最後の要求から（アイドル時間 - ALARM_REFRESH_SLACK_MS）以上たってからになる。
   */
  private async extendIdleAlarm(): Promise<void> {
    const deadline = this.clock.now() + this.config.idleTimeoutMs;
    this.idleDeadlineMs = deadline;
    if (this.armedAlarmMs !== undefined && deadline - this.armedAlarmMs < ALARM_REFRESH_SLACK_MS) {
      return;
    }
    await this.armAlarm(deadline);
  }

  /** 保存先のアラームを設定し、成功したら印を更新する。失敗したら印を元に戻して例外を返す。 */
  private async armAlarm(at: number): Promise<void> {
    const previous = this.armedAlarmMs;
    this.armedAlarmMs = at;
    try {
      await this.alarmStorage.setAlarm(at);
    } catch (error) {
      this.armedAlarmMs = previous;
      throw error;
    }
  }

  /** 同時に複数要求が来てもコンテナ起動と /health 待ちを 1 回だけ行う（単一飛行）。 */
  private ensureReady(domain: string): Promise<void> {
    if (!this.runtime.running || this.exitedGeneration === this.runGeneration) {
      this.readyPromise = undefined;
    }
    if (this.readyPromise) {
      return this.readyPromise;
    }

    const pending = this.startAndWaitUntilHealthy(domain);
    this.readyPromise = pending;
    // 待機側が途中で中断されても未処理拒否を残さず、失敗時は次回要求で再試行できるようにする。
    void pending.catch(() => {
      if (this.readyPromise === pending) {
        this.readyPromise = undefined;
      }
    });
    return pending;
  }

  private async startAndWaitUntilHealthy(domain: string): Promise<void> {
    if (this.destroying) {
      await this.destroying;
    }
    const needsStart = !this.runtime.running || this.exitedGeneration === this.runGeneration;
    if (needsStart) {
      const generation = ++this.runGeneration;
      const containerEnv = buildContainerEnvironment(this.env, domain, this.config.containerPort);
      this.runtime.start({ env: containerEnv, enableInternet: false });
      this.attachMonitor(generation);
    } else if (this.monitoredGeneration !== this.runGeneration) {
      if (this.runGeneration === 0) this.runGeneration = 1;
      this.attachMonitor(this.runGeneration);
    }

    if (this.runtime.setInactivityTimeout) {
      // アラームによる停止を主としつつ、DO 退避時の保険として非活動タイムアウトも設定する。
      await this.runtime.setInactivityTimeout(this.config.idleTimeoutMs);
    }

    const generationAtStart = this.runGeneration;
    const deadline = this.clock.now() + this.config.startupTimeoutMs;

    while (this.clock.now() < deadline) {
      if (!this.runtime.running || this.exitedGeneration === generationAtStart) {
        throw new StartupTimeoutError();
      }
      try {
        const healthRequest = new Request(`http://${domain}/health`, {
          method: "GET",
          headers: { Host: domain },
          redirect: "manual",
        });
        const response = await this.runtime.getTcpPort(this.config.containerPort).fetch(healthRequest);
        if (response.body) {
          void response.body.cancel().catch(() => {});
        }
        if (response.status === 200) {
          return;
        }
      } catch {
        // 起動直後の接続拒否等は想定内なので上限時間まで再確認する。
      }

      const remaining = deadline - this.clock.now();
      if (remaining <= 0) break;
      await this.sleep(Math.min(this.config.startupPollIntervalMs, remaining));
    }

    throw new StartupTimeoutError();
  }

  private attachMonitor(generation: number): void {
    this.monitoredGeneration = generation;
    const onExit = (): void => {
      if (this.runGeneration !== generation) return;
      this.exitedGeneration = generation;
      this.readyPromise = undefined;
    };
    try {
      void this.runtime.monitor().then(onExit, onExit);
    } catch {
      onExit();
    }
  }

  private sleep(durationMs: number): Promise<void> {
    return new Promise((resolve) => {
      this.clock.setTimeout(resolve, durationMs);
    });
  }
}
