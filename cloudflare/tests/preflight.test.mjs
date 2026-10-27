import assert from "node:assert/strict";
import { test } from "node:test";
import { spawnSync } from "node:child_process";
import { mkdtemp, writeFile, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { planFingerprint, validatePreflight } from "../scripts/preflight.mjs";

function fixture() {
  const secrets = {}, databases = [], facts = [];
  function profile(prefix, namespace) {
    function reference(slug, control = false) {
      const id = `${namespace}-${slug}`, key = control ? `${namespace}:control` : `${namespace}:tenant:${slug}`;
      const url_env = `MEIBO_DB_${prefix}_${slug.toUpperCase()}_URL`, token_env = `MEIBO_DB_${prefix}_${slug.toUpperCase()}_TOKEN`;
      const hostname = `${id}.example.test`;
      secrets[url_env] = `libsql://${hostname}`; secrets[token_env] = "synthetic-preflight-token";
      databases.push({ key, id, additionalBytes: 4096, additionalWrites: 100 });
      facts.push({ id, apiVerified: true, cliVerified: true, databaseTokenVerified: true,
        instances: [{ role: "primary", hostname }], cliUrl: `https://${hostname}/`, bytesUsed: 1024, bytesLimit: 10_000_000, writesUsed: 100, writesLimit: 1_000_000 });
      return { url_env, token_env };
    }
    return { version: 1, control: reference("control", true), tenants: { alpha: reference("alpha") } };
  }
  const plan = { version: 1, accountId: "1".repeat(32), workerName: "meibo-beta", origin: "https://meibo-beta.synthetic-team.workers.dev",
    sourceRevision: "a".repeat(40), imageDigest: `sha256:${"b".repeat(64)}`, bundleSha256: "c".repeat(64),
    remoteConfig: profile("SOURCE", "source"), restoreConfigs: [profile("RESTORE", "restore0")], databases };
  const report = { version: 1, kind: "fixture", checkedAtMs: Date.now(), planSha256: planFingerprint(plan, secrets),
    account: { id: plan.accountId, accessVerified: true, workersPaid: true, containersAllowed: true, workersSubdomain: "synthetic-team" },
    worker: { name: "meibo-beta", accountId: plan.accountId, ownershipVerified: true, existingBindings: [] },
    turso: { organizationVerified: true, bytesUsed: 4096, bytesLimit: 5_000_000_000, writesUsed: 400, writesLimit: 10_000_000, databases: facts },
    image: { os: "linux", architecture: "amd64", digest: plan.imageDigest, sourceRevision: plan.sourceRevision, bootVerified: true, nonRootUid: 10001, credentialsInImage: false },
    bundle: { sha256: plan.bundleSha256, sourceRevision: plan.sourceRevision, buildVerified: true } };
  return { plan, report, secrets, expectedAccountId: plan.accountId, kind: "fixture", nowMs: report.checkedAtMs };
}
function refreshed(input) { input.report.planSha256 = planFingerprint(input.plan, input.secrets); return input; }

test("valid explicit account, all source/restore primaries and amd64 artifact validate as fixture only", () => {
  const result = validatePreflight(fixture());
  assert.equal(result.ok, true); assert.equal(result.kind, "fixture"); assert.equal(result.deploymentAuthorized, false);
});
test("canonical libsql/HTTPS host and default port aliases are accepted only when verified equal to primary", () => {
  const input = fixture(); input.secrets.MEIBO_DB_SOURCE_ALPHA_URL = "https://SOURCE-ALPHA.example.test:443/";
  assert.equal(validatePreflight(refreshed(input)).ok, true);
});
test("replacing existing bindings counts each final variable once", () => {
  const input = fixture(); input.report.worker.existingBindings = [
    ...Array.from({ length: 122 }, (_, i) => ({ name: `EXISTING_${i}`, type: "plain_text", providerAccepted: true })),
    { name: "MEIBO_ORIGIN", type: "plain_text", providerAccepted: true },
    { name: "MEIBO_DB_SOURCE_CONTROL_URL", type: "secret_text", providerAccepted: true },
  ];
  assert.equal(validatePreflight(input).ok, true);
});
test("full paid variable capacity remains valid with secret registration split into bounded batches", () => {
  const input = fixture(), config = { version: 1, control: null, tenants: {} }, secrets = {}, plans = [], facts = [];
  for (let i = 0; i < 63; i++) {
    const url_env = `MEIBO_DB_U${i}`, token_env = `MEIBO_DB_T${i}`, id = `synthetic-db-${i}`, host = `${id}.example.test`;
    const reference = { url_env, token_env }; if (i === 0) config.control = reference; else config.tenants[`t${i}`] = reference;
    secrets[url_env] = `libsql://${host}`; secrets[token_env] = "synthetic-preflight-token";
    plans.push({ key: i === 0 ? "source:control" : `source:tenant:t${i}`, id, additionalBytes: 4096, additionalWrites: 100 });
    facts.push({ ...input.report.turso.databases[0], id, cliUrl: `https://${host}/`, instances: [{ role: "primary", hostname: host }] });
  }
  input.plan.remoteConfig = config; input.plan.restoreConfigs = []; input.plan.databases = plans;
  input.secrets = secrets; input.report.turso.databases = facts;
  const result = validatePreflight(refreshed(input)); assert.equal(result.ok, true); assert.equal(result.secretBatchCount, 2);
});
test("fingerprint is stable under key order but binds configuration and secret values", () => {
  const input = fixture(), reordered = Object.fromEntries(Object.entries(input.secrets).reverse());
  assert.equal(planFingerprint(input.plan, reordered), input.report.planSha256);
  input.secrets.MEIBO_DB_SOURCE_ALPHA_TOKEN += "-changed";
  assert.notEqual(planFingerprint(input.plan, input.secrets), input.report.planSha256);
});

const rejected = [
  ["missing explicit expected account", (x) => { x.expectedAccountId = ""; }],
  ["wrong selected account", (x) => { x.plan.accountId = "2".repeat(32); }],
  ["account access unavailable", (x) => { x.report.account.accessVerified = false; }],
  ["paid contract missing", (x) => { x.report.account.workersPaid = false; }],
  ["containers permission missing", (x) => { x.report.account.containersAllowed = false; }],
  ["worker ownership unknown", (x) => { x.report.worker.ownershipVerified = false; }],
  ["worker belongs to another account", (x) => { x.report.worker.accountId = "2".repeat(32); }],
  ["wrong public origin", (x) => { x.plan.origin = "https://elsewhere.example.test"; }],
  ["preview origin", (x) => { x.plan.origin = "https://preview-meibo-beta.synthetic-team.workers.dev"; }],
  ["unknown plan field", (x) => { x.plan.unknown = true; }],
  ["unknown report field", (x) => { x.report.unknown = true; }],
  ["unknown configuration field", (x) => { x.plan.remoteConfig.unknown = true; }],
  ["stale provider report", (x) => { x.report.checkedAtMs -= 300001; }],
  ["future provider report", (x) => { x.report.checkedAtMs += 30001; }],
  ["fixture cannot be relabelled live", (x) => { x.kind = "live"; }],
  ["missing secret", (x) => { delete x.secrets.MEIBO_DB_SOURCE_ALPHA_TOKEN; }],
  ["extra unreferenced secret", (x) => { x.secrets.MEIBO_DB_EXTRA_TOKEN = "synthetic-extra"; }],
  ["oversized secret", (x) => { x.secrets.MEIBO_DB_SOURCE_ALPHA_TOKEN = "x".repeat(5121); }],
  ["UTF8 byte size applies", (x) => { x.secrets.MEIBO_DB_SOURCE_ALPHA_TOKEN = "あ".repeat(2000); }],
  ["existing variables count toward final paid limit", (x) => { x.report.worker.existingBindings = Array.from({ length: 128 }, (_, i) => ({ name: `EXISTING_${i}`, type: "plain_text", providerAccepted: true })); }],
  ["non-variable binding name collision", (x) => { x.report.worker.existingBindings = [{ name: "MEIBO_REMOTE_CONFIG", type: "other", providerAccepted: true }]; }],
  ["duplicate resolved endpoint across restore target", (x) => { x.secrets.MEIBO_DB_RESTORE_ALPHA_URL = x.secrets.MEIBO_DB_SOURCE_ALPHA_URL; }],
  ["missing target mapping", (x) => { x.plan.databases.pop(); }],
  ["duplicate provider database", (x) => { x.plan.databases[3].id = x.plan.databases[0].id; }],
  ["missing provider fact", (x) => { x.report.turso.databases.pop(); }],
  ["provider organization unknown", (x) => { x.report.turso.organizationVerified = false; }],
  ["provider API failed", (x) => { x.report.turso.databases[0].apiVerified = false; }],
  ["instance CLI failed", (x) => { x.report.turso.databases[0].cliVerified = false; }],
  ["unverified database token", (x) => { x.report.turso.databases[0].databaseTokenVerified = false; }],
  ["replica exists", (x) => { x.report.turso.databases[0].instances.push({ role: "replica", hostname: "replica.example.test" }); }],
  ["primary role unknown", (x) => { x.report.turso.databases[0].instances[0].role = "unknown"; }],
  ["unverified general URL", (x) => { x.report.turso.databases[0].cliUrl = "libsql://unverified.example.test"; }],
  ["provider hostname mismatch", (x) => { x.report.turso.databases[0].instances[0].hostname = "unverified.example.test"; }],
  ["API hostname contains URL syntax", (x) => { x.report.turso.databases[0].instances[0].hostname += "/"; }],
  ["database capacity exceeded", (x) => { x.report.turso.databases[0].bytesLimit = 1024; }],
  ["account capacity exceeded", (x) => { x.report.turso.bytesLimit = 4096; }],
  ["writes quota exceeded", (x) => { x.report.turso.writesLimit = 400; }],
  ["unsafe integer quota", (x) => { x.report.turso.bytesLimit = Number.MAX_SAFE_INTEGER + 1; }],
  ["arm64 image", (x) => { x.report.image.architecture = "arm64"; }],
  ["image not booted", (x) => { x.report.image.bootVerified = false; }],
  ["root image", (x) => { x.report.image.nonRootUid = 0; }],
  ["credentials baked into image", (x) => { x.report.image.credentialsInImage = true; }],
  ["image digest mismatch", (x) => { x.report.image.digest = `sha256:${"d".repeat(64)}`; }],
  ["image source mismatch", (x) => { x.report.image.sourceRevision = "d".repeat(40); }],
  ["bundle mismatch", (x) => { x.report.bundle.sha256 = "d".repeat(64); }],
  ["bundle not built", (x) => { x.report.bundle.buildVerified = false; }],
];
for (const [name, mutate] of rejected) test(`preflight rejects ${name}`, () => {
  const input = fixture(); mutate(input);
  assert.throws(() => validatePreflight(refreshed(input)), (error) => error.name === "PreflightError" && error.code !== "feature_disabled");
});
test("a changed private plan cannot reuse an otherwise successful report", () => {
  const input = fixture(); input.plan.databases[0].additionalBytes++;
  assert.throws(() => validatePreflight(input), (error) => error.code === "stale_report");
});

async function invoke(input, transform) {
  const directory = await mkdtemp(join(tmpdir(), "meibo-preflight-test-"));
  try {
    for (const [name, value] of Object.entries({ plan: input.plan, report: input.report, secrets: input.secrets }))
      await writeFile(join(directory, `${name}.json`), JSON.stringify(value), { mode: 0o600 });
    if (transform) await transform(directory);
    const result = spawnSync(process.execPath, [resolve("scripts/preflight.mjs"), "--plan", join(directory, "plan.json"),
      "--report", join(directory, "report.json"), "--secrets", join(directory, "secrets.json"),
      "--expected-account-env", "MEIBO_EXPECTED_CF_ACCOUNT_ID", "--kind", "fixture"],
    { env: { ...process.env, MEIBO_EXPECTED_CF_ACCOUNT_ID: input.expectedAccountId }, encoding: "utf8" });
    assert.equal(result.stderr, "");
    assert.ok(!result.stdout.includes("synthetic-preflight-token") && !result.stdout.includes("example.test")
      && !result.stdout.includes(input.plan.accountId) && !result.stdout.includes(directory));
    return { status: result.status, output: JSON.parse(result.stdout) };
  } finally { await rm(directory, { recursive: true, force: true }); }
}
test("real read-only CLI validates fixture without claiming deployment permission", async () => {
  const result = await invoke(fixture()); assert.equal(result.status, 0); assert.equal(result.output.ok, true); assert.equal(result.output.deploymentAuthorized, false);
});
test("CLI rejects malformed/duplicate JSON without reflecting private input", async () => {
  for (const text of ['{"version":1,"version":1}', '{"private-secret":"unterminated']) {
    const result = await invoke(fixture(), (directory) => writeFile(join(directory, "plan.json"), text));
    assert.equal(result.status, 1); assert.equal(result.output.code, "invalid_input");
  }
});
test("CLI refuses symlink input before reading its contents", async () => {
  const result = await invoke(fixture(), async (directory) => {
    await rm(join(directory, "plan.json")); await symlink(join(directory, "report.json"), join(directory, "plan.json"));
  });
  assert.equal(result.status, 1); assert.equal(result.output.code, "invalid_input");
});
