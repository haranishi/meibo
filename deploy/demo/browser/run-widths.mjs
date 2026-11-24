// 幅390・768・1440pxを、それぞれ新しいサーバーで順に試験する。1つでも失敗したら、終了コードは0以外。
import { spawnSync } from "node:child_process";

let failed = false;
for (const width of [390, 768, 1440]) {
  console.log(`\n=== 幅 ${width}px ===`);
  const run = spawnSync("npx", ["playwright", "test", "--config", "playwright.config.mjs"], {
    stdio: "inherit",
    env: { ...process.env, MEIBO_DEMO_WIDTH: String(width) },
  });
  if (run.status !== 0) failed = true;
}
process.exit(failed ? 1 : 0);
