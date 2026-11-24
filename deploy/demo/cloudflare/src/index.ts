import type { Env } from "./contracts.ts";
import { DemoContainer } from "./demo-container.ts";
import { createDemoWorker } from "./worker.ts";

const handler = createDemoWorker();

export { DemoContainer };

export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    return handler.fetch(request, env, ctx);
  },
} satisfies ExportedHandler<Env>;
