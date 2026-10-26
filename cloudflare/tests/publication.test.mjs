import assert from "node:assert/strict";
import test from "node:test";
import { publication } from "../scripts/publication.mjs";
import { secretBatches } from "../scripts/publishing-io.mjs";

function fixture() {
  const accountId = "a".repeat(32), configDigest = `sha256:${"f".repeat(64)}`;
  const plan = { version: 1, accountId, workerName: "meibo-beta", origin: "https://meibo-beta.fixture.workers.dev",
    sourceRevision: "b".repeat(64), imageDigest: `sha256:${"c".repeat(64)}`, bundleSha256: "d".repeat(64),
    remoteConfig: { version: 1, control: { url_env: "MEIBO_DB_CONTROL_URL", token_env: "MEIBO_DB_CONTROL_TOKEN" }, tenants: {} }, restoreConfigs: [],
    databases: [{ key: "source:control", id: "fixture-db", additionalBytes: 1000, additionalWrites: 10 }] };
  const secrets = { MEIBO_DB_CONTROL_URL: "https://primary.fixture.turso.io", MEIBO_DB_CONTROL_TOKEN: "fixture-token" };
  const calls = [], providers = {
    account: { id: accountId, accessVerified: true, workersPaid: true, containersAllowed: true, workersSubdomain: "fixture" },
    worker: { name: plan.workerName, accountId, ownershipVerified: true, existingBindings: [] },
    turso: { organizationVerified: true, bytesUsed: 4096, bytesLimit: 5000000, writesUsed: 5, writesLimit: 100000, databases: [{
      id: "fixture-db", apiVerified: true, cliVerified: true, databaseTokenVerified: true,
      instances: [{ role: "primary", hostname: "primary.fixture.turso.io" }], cliUrl: secrets.MEIBO_DB_CONTROL_URL,
      bytesUsed: 4096, bytesLimit: 1048576, writesUsed: 5, writesLimit: 100000,
    }] },
  };
  const artifacts = {
    image: { os: "linux", architecture: "amd64", digest: plan.imageDigest, sourceRevision: plan.sourceRevision, bootVerified: true, nonRootUid: 10001, credentialsInImage: false },
    bundle: { sha256: plan.bundleSha256, sourceRevision: plan.sourceRevision, buildVerified: true }, imageConfigDigest: configDigest,
  };
  const registry = { digest: `sha256:${"e".repeat(64)}`, configDigest, image: "registry.fixture/meibo@sha256:fixture" };
  const io = {
    async collect() { calls.push("collect"); return structuredClone(providers); },
    async verify() { calls.push("verify"); return structuredClone(artifacts); },
    async record() { calls.push("record"); },
    async push() { calls.push("push"); return registry; },
    async install(current) { calls.push("install"); assert.equal(current.imageDigest, registry.digest); return { workerVersion: "fixture-version" }; },
    async recordDeployment() { calls.push("record-deployment"); },
  };
  return { plan, secrets, providers, artifacts, registry, io, calls, options: { expectedAccountId: accountId, mode: "publish", io } };
}

test("check freshly verifies providers and actual artifacts without mutating providers", async () => {
  const f = fixture(); f.options.mode = "check";
  assert.deepEqual(await publication(f.plan, f.secrets, f.options), { ok: true, deployed: false, deploymentAuthorized: false });
  assert.deepEqual(f.calls, ["collect", "verify", "record"]);
});
test("publish binds the registry config to a second fresh gate before installation", async () => {
  const f = fixture();
  assert.equal((await publication(f.plan, f.secrets, f.options)).deployed, true);
  assert.deepEqual(f.calls, ["collect", "verify", "record", "push", "collect", "verify", "record", "install", "record-deployment"]);
});
for (const [name, change] of [
  ["unpaid plan", (f) => { f.providers.account.workersPaid = false; }],
  ["unverified existing Worker", (f) => { f.providers.worker.ownershipVerified = false; }],
  ["unknown quota", (f) => { f.providers.turso.bytesLimit = null; }],
  ["changed image", (f) => { f.artifacts.image.digest = `sha256:${"1".repeat(64)}`; }],
  ["unverified boot", (f) => { f.artifacts.image.bootVerified = false; }],
  ["changed source", (f) => { f.artifacts.image.sourceRevision = "2".repeat(64); }],
]) test(`publication refuses ${name} before any provider mutation`, async () => {
  const f = fixture(); change(f);
  await assert.rejects(publication(f.plan, f.secrets, f.options));
  assert.equal(f.calls.includes("push"), false); assert.equal(f.calls.includes("install"), false);
});
test("an independent account mismatch performs no queries or mutation", async () => {
  const f = fixture(); f.options.expectedAccountId = "0".repeat(32);
  await assert.rejects(publication(f.plan, f.secrets, f.options)); assert.deepEqual(f.calls, []);
});
test("a changed registry image config is refused after upload, before installation", async () => {
  const f = fixture(); f.registry.configDigest = `sha256:${"0".repeat(64)}`;
  await assert.rejects(publication(f.plan, f.secrets, f.options));
  assert.equal(f.calls.filter((x) => x === "collect").length, 2); assert.equal(f.calls.includes("install"), false);
});
test("a primary change during image push is caught by the second fresh gate", async () => {
  const f = fixture(); f.io.push = async () => { f.calls.push("push"); f.providers.turso.databases[0].instances.push({ role: "replica", hostname: "replica.fixture.turso.io" }); return f.registry; };
  await assert.rejects(publication(f.plan, f.secrets, f.options)); assert.equal(f.calls.includes("install"), false);
});
test("lost installation response is marked uncertain without a retry", async () => {
  const f = fixture(); let attempts = 0;
  f.io.install = async () => { attempts++; throw new Error("private-diagnostic"); };
  await assert.rejects(publication(f.plan, f.secrets, f.options), (e) => e.installAttempted === true && !e.message.includes("private-diagnostic"));
  assert.equal(attempts, 1); assert.equal(f.calls.includes("record-deployment"), false);
});
test("failure to persist the final version cannot be reported as never deployed", async () => {
  const f = fixture();
  f.io.recordDeployment = async () => { throw new Error("disk-full-private-detail"); };
  await assert.rejects(publication(f.plan, f.secrets, f.options), (e) => e.installAttempted === true && !e.message.includes("disk-full-private-detail"));
});
test("source secrets alone are batched below the provider bulk limit", () => {
  const f = fixture();
  f.plan.restoreConfigs = [{ version: 1, control: { url_env: "MEIBO_DB_RESTORE_URL", token_env: "MEIBO_DB_RESTORE_TOKEN" }, tenants: {} }];
  f.secrets.MEIBO_DB_RESTORE_URL = "https://restore.fixture.turso.io"; f.secrets.MEIBO_DB_RESTORE_TOKEN = "do-not-upload";
  for (let i = 0; i < 51; i++) {
    const url_env = `MEIBO_DB_T${i}_URL`, token_env = `MEIBO_DB_T${i}_TOKEN`;
    f.plan.remoteConfig.tenants[`tenant-${i}`] = { url_env, token_env };
    f.secrets[url_env] = `https://t${i}.fixture.turso.io`; f.secrets[token_env] = "synthetic";
  }
  const batches = secretBatches(f.plan, f.secrets);
  assert.deepEqual(batches.map((batch) => Object.keys(batch).length), [100, 5]);
  const uploaded = Object.assign({}, ...batches);
  assert.equal(uploaded.MEIBO_ORIGIN, undefined); assert.equal(uploaded.MEIBO_DB_RESTORE_TOKEN, undefined);
  assert.equal(JSON.parse(uploaded.MEIBO_REMOTE_CONFIG).version, 1);
});
