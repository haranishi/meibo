import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import test from "node:test";
import Ajv from "ajv";
import { parse } from "jsonc-parser";

const require = createRequire(import.meta.url);
const configPath = new URL("../wrangler.jsonc", import.meta.url);

async function readConfig() {
  const errors = [];
  const config = parse(await readFile(configPath, "utf8"), errors);
  assert.deepEqual(errors, [], "Wrangler configuration must be valid JSONC");
  return config;
}

test("configuration conforms to the installed Wrangler schema", async () => {
  const schemaPath = join(dirname(require.resolve("wrangler/package.json")), "config-schema.json");
  const schema = JSON.parse(await readFile(schemaPath, "utf8"));
  const validate = new Ajv({ strict: false, allErrors: true }).compile(schema);
  assert.equal(validate(await readConfig()), true, JSON.stringify(validate.errors));
});

test("tracked configuration requires a private origin and contains no database values", async () => {
  const config = await readConfig();
  assert.deepEqual(config.vars, { MEIBO_ORIGIN: "" });
  assert.equal(Object.hasOwn(config, "account_id"), false);
  assert.equal(Object.hasOwn(config, "secrets"), false);
  assert.equal(Object.hasOwn(config, "migrations"), false);
  for (const container of config.containers) {
    assert.equal(Object.hasOwn(container, "image_vars"), false);
  }
});
