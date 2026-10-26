import { DurableObject } from "cloudflare:workers";
import { BoundaryError, boundaryErrorResponse, forwardResponse, prepareContainerRequest, validatePublicOrigin, validateRequest } from "./boundary.ts";
import { ContainerCoordinator } from "./container.ts";
import type { Env } from "./contracts.ts";

const INSTANCE_NAME = "meibo-beta-v1";

function unknownOutcome(): Response {
  return new Response(JSON.stringify({
    error: "処理結果を確認できません。再送せず管理者へ連絡してください。",
    code: "outcome_unknown",
  }), {
    status: 503,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    },
  });
}

/** The namespace is selected only after validating the public request metadata. */
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    let attempted = false;
    try {
      validateRequest(request, validatePublicOrigin(env.MEIBO_ORIGIN));
      const stub = env.MEIBO_CONTAINER.getByName(INSTANCE_NAME);
      attempted = true;
      return forwardResponse(await stub.fetch(request));
    } catch (error) {
      return attempted ? unknownOutcome() : boundaryErrorResponse(error);
    }
  },
} satisfies ExportedHandler<Env>;

/** One beta runtime; SQL-backed operation gates remain authoritative in Rust. */
export class MeiboContainer extends DurableObject<Env> {
  private coordinator: ContainerCoordinator | undefined;

  override async fetch(request: Request): Promise<Response> {
    try {
      const origin = validatePublicOrigin(this.env.MEIBO_ORIGIN);
      const validated = validateRequest(request, origin);
      if (!this.ctx.container) throw new BoundaryError("unavailable");
      this.coordinator ??= new ContainerCoordinator(this.ctx.container, this.env, origin);
      return await this.coordinator.forward(validated, () => prepareContainerRequest(validated),
        (error) => error instanceof BoundaryError ? boundaryErrorResponse(error) : undefined);
    } catch (error) {
      return boundaryErrorResponse(error);
    }
  }
}
