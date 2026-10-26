import assert from "node:assert/strict";
import test from "node:test";
import { collectProviders, ProviderError } from "../scripts/providers.mjs";
import { planFingerprint, validatePreflight } from "../scripts/preflight.mjs";

const accountId = "a".repeat(32);
function fixture() {
  const reference = { url_env: "MEIBO_DB_CONTROL_URL", token_env: "MEIBO_DB_CONTROL_TOKEN" };
  const plan = { version: 1, accountId, workerName: "meibo-beta", origin: "https://meibo-beta.fixture.workers.dev",
    sourceRevision: "b".repeat(64), imageDigest: `sha256:${"c".repeat(64)}`, bundleSha256: "d".repeat(64),
    remoteConfig: { version: 1, control: reference, tenants: {} }, restoreConfigs: [],
    databases: [{ key: "source:control", id: "fixture-db-id", additionalBytes: 1000, additionalWrites: 10 }] };
  const secrets = { MEIBO_DB_CONTROL_URL: "libsql://primary.fixture.turso.io", MEIBO_DB_CONTROL_TOKEN: "synthetic-token" };
  const calls = [], routes = new Map([
    ["", { organization: { slug: "fixture" } }],
    ["/subscription", { subscription: { plan: "starter" } }],
    ["/plans", { plans: [{ name: "starter", quotas: { storage: 5000000, rowsWritten: 100000 } }] }],
    ["/usage", { organization: { usage: { storage_bytes: 4096, rows_written: 5 } } }],
    ["/databases", { databases: [{ Name: "fixture-control", DbId: "fixture-db-id" }] }],
    ["/databases/fixture-control", { database: { Name: "fixture-control", DbId: "fixture-db-id", block_reads: false, block_writes: false } }],
    ["/databases/fixture-control/instances", { instances: [{ type: "primary", name: "fixture-primary", hostname: "primary.fixture.turso.io" }] }],
    ["/databases/fixture-control/configuration", { size_limit: "10mb", block_reads: false, block_writes: false }],
    ["/databases/fixture-control/usage", { database: { uuid: "fixture-db-id", total: { storage_bytes: 4096, rows_written: 5 } } }],
  ]);
  const cfRoutes = new Map([
    [`/accounts/${accountId}`, { status: 200, body: { success: true, result: { id: accountId } } }],
    [`/accounts/${accountId}/workers/subdomain`, { status: 200, body: { success: true, result: { subdomain: "fixture" } } }],
    [`/accounts/${accountId}/containers/applications`, { status: 200, body: { success: true, result: [] } }],
    [`/accounts/${accountId}/workers/scripts/meibo-beta/settings`, { status: 404, body: { success: false, errors: [{ code: 10007 }] } }],
  ]);
  const adapters = { expectedAccountId: accountId, organization: "fixture",
    async cf(path) { calls.push(["cf", path]); assert.ok(cfRoutes.has(path)); return cfRoutes.get(path); },
    async turso(path) { calls.push(["turso", path]); const route = path.slice("/v1/organizations/fixture".length); assert.ok(routes.has(route)); return { status: 200, body: routes.get(route) }; },
    async instanceUrl(name, instance) { calls.push(["cli", name, instance]); return "https://primary.fixture.turso.io"; },
    async databaseRead(url, token) { calls.push(["database", url]); assert.equal(token, "synthetic-token"); return true; },
  };
  return { plan, secrets, adapters, routes, cfRoutes, calls };
}
async function collect(f) { return collectProviders(f.plan, f.secrets, f.adapters); }
function validate(f, providers) {
  return validatePreflight({ plan: f.plan, secrets: f.secrets, expectedAccountId: accountId, kind: "live", report: {
    version: 1, kind: "live", checkedAtMs: Date.now(), planSha256: planFingerprint(f.plan, f.secrets), ...providers,
    image: { os: "linux", architecture: "amd64", digest: f.plan.imageDigest, sourceRevision: f.plan.sourceRevision, bootVerified: true, nonRootUid: 10001, credentialsInImage: false },
    bundle: { sha256: f.plan.bundleSha256, sourceRevision: f.plan.sourceRevision, buildVerified: true },
  } });
}

test("fresh API/CLI/database facts feed the same production preflight", async () => {
  const f = fixture(), providers = await collect(f);
  assert.equal(validate(f, providers).ok, true);
  assert.equal(validate(f, providers).deploymentAuthorized, false);
  assert.equal(providers.turso.databases[0].bytesLimit, 5000000);
  assert.equal(f.calls.filter(([type]) => type === "database").length, 1);
  await collect(f);
  assert.equal(f.calls.filter(([type]) => type === "database").length, 2, "a prior successful report is never cached as permission");
});

test("the known unpaid Containers entitlement is reported as a contract refusal", async () => {
  const f = fixture();
  f.cfRoutes.set(`/accounts/${accountId}/containers/applications`, { status: 401, body: { success: false, errors: [{ code: 1000, message: "Requires Workers Paid" }] } });
  const providers = await collect(f);
  assert.equal(providers.account.workersPaid, false);
  assert.throws(() => validate(f, providers), (e) => e.code === "contract");
});

test("current top-level database usage is verified against its database UUID", async () => {
  const f = fixture();
  const previous = f.routes.get("/databases/fixture-control/usage").database;
  f.routes.set("/databases/fixture-control/usage", { database: { uuid: previous.uuid }, total: previous.total });
  assert.equal(validate(f, await collect(f)).ok, true);
});

test("unverified existing Worker is refused instead of being overwritten", async () => {
  const f = fixture();
  f.cfRoutes.set(`/accounts/${accountId}/workers/scripts/meibo-beta/settings`, { status: 200, body: { success: true, result: { bindings: [] } } });
  const providers = await collect(f);
  assert.equal(providers.worker.ownershipVerified, false);
  assert.throws(() => validate(f, providers), (e) => e.code === "worker");
});

const invalid = [
  ["independent account mismatch", (f) => { f.adapters.expectedAccountId = "e".repeat(32); }],
  ["unknown Containers authorization failure", (f) => { f.cfRoutes.set(`/accounts/${accountId}/containers/applications`, { status: 403, body: { success: false, errors: [{ code: 10000 }] } }); }],
  ["unproven Worker absence", (f) => { f.cfRoutes.set(`/accounts/${accountId}/workers/scripts/meibo-beta/settings`, { status: 404, body: { success: false, errors: [{ code: 99999 }] } }); }],
  ["organization mismatch", (f) => { f.routes.set("", { organization: { slug: "another-org" } }); }],
  ["unknown plan quota", (f) => { f.routes.get("/plans").plans[0].quotas.storage = null; }],
  ["provider DB identity mismatch", (f) => { f.routes.get("/databases/fixture-control").database.DbId = "other"; }],
  ["replica topology", (f) => { f.routes.get("/databases/fixture-control/instances").instances.push({ type: "replica", name: "replica", hostname: "replica.fixture.turso.io" }); }],
  ["CLI primary mismatch", (f) => { f.adapters.instanceUrl = async () => "https://another.fixture.turso.io"; }],
  ["configured URL mismatch", (f) => { f.secrets.MEIBO_DB_CONTROL_URL = "https://attacker.invalid"; }],
  ["blocked database", (f) => { f.routes.get("/databases/fixture-control/configuration").block_writes = true; }],
  ["missing database usage", (f) => { f.routes.get("/databases/fixture-control/usage").database.total = null; }],
  ["invalid database token", (f) => { f.adapters.databaseRead = async () => false; }],
];
for (const [name, change] of invalid) test(`fresh collection refuses ${name}`, async () => {
  const f = fixture(); change(f);
  await assert.rejects(collect(f), ProviderError);
  if (name !== "invalid database token") assert.equal(f.calls.filter(([type]) => type === "database").length, 0);
});
