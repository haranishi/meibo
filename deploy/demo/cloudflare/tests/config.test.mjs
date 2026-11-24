import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import Ajv from "ajv";
import { parse } from "jsonc-parser";

const require = createRequire(import.meta.url);
const demoDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(demoDir, "..", "..", "..");
const configPath = join(demoDir, "wrangler.jsonc");
const demoDockerfilePath = join(demoDir, "Dockerfile");
const rootDockerfilePath = join(repoRoot, "Dockerfile");

async function readWranglerConfig() {
  const raw = await readFile(configPath, "utf8");
  const errors = [];
  const config = parse(raw, errors);
  assert.deepEqual(errors, [], "wrangler.jsonc は有効な JSONC でなければならない");
  return { raw, config };
}

/** Dockerfile から指定ステージの命令一覧を取り出す。 */
function extractStageInstructions(dockerfileText, stageName) {
  const lines = dockerfileText.split(/\r?\n/);
  let currentStage = null;
  const instructions = [];
  const stages = [];

  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) continue;
    const fromMatch = /^FROM\s+.+\s+AS\s+([A-Za-z0-9._-]+)$/i.exec(line);
    if (fromMatch) {
      currentStage = fromMatch[1];
      stages.push(currentStage);
      continue;
    }
    if (/^FROM\s+/i.test(line)) {
      currentStage = null;
      stages.push("(unnamed)");
      continue;
    }
    if (currentStage === stageName) {
      instructions.push(line);
    }
  }
  return { instructions, stages };
}

test("wrangler.jsonc が導入済み Wrangler の JSON Schema に適合する", async () => {
  const { config } = await readWranglerConfig();
  const schemaPath = join(dirname(require.resolve("wrangler/package.json")), "config-schema.json");
  const schema = JSON.parse(await readFile(schemaPath, "utf8"));
  const validate = new Ajv({ strict: false, allErrors: true }).compile(schema);
  assert.equal(validate(config), true, JSON.stringify(validate.errors));
});

test("wrangler.jsonc が公開デモの安全条件と無秘密条件を満たす", async () => {
  const { raw, config } = await readWranglerConfig();

  assert.equal(config.name, "meibo-demo");
  assert.equal(config.workers_dev, false);
  assert.equal(config.preview_urls, false);
  assert.deepEqual(config.observability, { enabled: false });

  // ドメイン・アカウントID・ルート・秘密を含めない
  assert.equal(Object.hasOwn(config, "account_id"), false);
  assert.equal(Object.hasOwn(config, "routes"), false);
  assert.equal(Object.hasOwn(config, "route"), false);
  assert.equal(Object.hasOwn(config, "secrets"), false);
  assert.equal(Object.hasOwn(config, "migrations"), false);
  assert.equal(/[0-9a-f]{32}/i.test(raw), false, "アカウントIDらしい32桁16進文字列を含めない");

  // 公開ドメインは空文字で置き、停止切替は既定 "true"
  assert.equal(config.vars.MEIBO_DOMAIN, "");
  assert.equal(config.vars.DEMO_ENABLED, "true");
  assert.equal(config.vars.MEIBO_DEMO_CONTACT, "");

  // コンテナは 1 つ、max_instances=1、instance_type は lite か basic
  assert.equal(Array.isArray(config.containers), true);
  assert.equal(config.containers.length, 1);
  const [container] = config.containers;
  assert.equal(container.class_name, "DemoContainer");
  assert.equal(container.scheduling_policy, "default");
  assert.equal(container.max_instances, 1);
  assert.equal(["lite", "basic"].includes(container.instance_type), true);
  assert.equal(container.image, "./Dockerfile");
  assert.equal(resolve(demoDir, container.image_build_context), repoRoot);
  assert.equal(Object.hasOwn(container, "image_vars"), false);

  // Durable Object と SQLite export 定義
  assert.deepEqual(config.durable_objects, {
    bindings: [{ name: "DEMO_CONTAINER", class_name: "DemoContainer" }],
  });
  assert.deepEqual(config.exports, {
    DemoContainer: { type: "durable-object", storage: "sqlite" },
  });

  // レート制限バインディング（入室・その他POST・コンテナ到達GET）
  assert.equal(Array.isArray(config.ratelimits), true);
  const byName = new Map(config.ratelimits.map((entry) => [entry.name, entry]));
  assert.deepEqual(byName.get("RATE_LIMIT_DEMO_ENTER")?.simple, { limit: 20, period: 60 });
  assert.deepEqual(byName.get("RATE_LIMIT_POST")?.simple, { limit: 60, period: 60 });
  assert.deepEqual(byName.get("RATE_LIMIT_GET")?.simple, { limit: 600, period: 60 });
  const namespaceIds = new Set(config.ratelimits.map((entry) => entry.namespace_id));
  assert.equal(namespaceIds.size, config.ratelimits.length);
});

test("デモ専用 Dockerfile の demo ステージ ENTRYPOINT がリポジトリ直下の Dockerfile と一致する", async () => {
  const [rootContent, demoContent] = await Promise.all([
    readFile(rootDockerfilePath, "utf8"),
    readFile(demoDockerfilePath, "utf8"),
  ]);

  const rootDemo = extractStageInstructions(rootContent, "demo");
  const cloudflareDemo = extractStageInstructions(demoContent, "demo");

  const rootEntrypoints = rootDemo.instructions.filter((line) => line.startsWith("ENTRYPOINT "));
  const demoEntrypoints = cloudflareDemo.instructions.filter((line) => line.startsWith("ENTRYPOINT "));

  assert.equal(rootEntrypoints.length, 1, "直下の Dockerfile に demo ステージの ENTRYPOINT が 1 件あること");
  assert.equal(demoEntrypoints.length, 1, "デモ専用 Dockerfile に demo ステージの ENTRYPOINT が 1 件あること");
  assert.equal(demoEntrypoints[0], rootEntrypoints[0], "demo ステージの ENTRYPOINT が完全一致すること");

  // wrangler が最終ステージをビルドするため、最終ステージが demo であること
  assert.equal(cloudflareDemo.stages.at(-1), "demo");
  assert.equal(cloudflareDemo.instructions.includes("ENV PORT=4317"), true);

  // runtime ステージが非 root ユーザー（10001:10001）で動くこと
  const cloudflareRuntime = extractStageInstructions(demoContent, "runtime");
  assert.equal(cloudflareRuntime.instructions.includes("USER 10001:10001"), true);
});
