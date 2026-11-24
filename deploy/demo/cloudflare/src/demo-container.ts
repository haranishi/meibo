import { DurableObject } from "cloudflare:workers";
import { BoundaryError, boundaryErrorResponse } from "./boundary.ts";
import { DemoContainerController } from "./controller.ts";
import type { Env } from "./contracts.ts";

export { buildContainerEnvironment, DemoContainerController } from "./controller.ts";

/** 公開デモ用の単一コンテナを管理する Durable Object。 */
export class DemoContainer extends DurableObject<Env> {
  private controller: DemoContainerController | undefined;

  private getController(): DemoContainerController | undefined {
    const container = this.ctx.container;
    if (!container) return undefined;
    this.controller ??= new DemoContainerController(container, this.ctx.storage, this.env);
    return this.controller;
  }

  override async fetch(request: Request): Promise<Response> {
    const controller = this.getController();
    if (!controller) {
      return boundaryErrorResponse(new BoundaryError("unavailable"), request);
    }
    return await controller.handleFetch(request);
  }

  override async alarm(): Promise<void> {
    const controller = this.getController();
    if (!controller) return;
    await controller.handleAlarm();
  }
}
