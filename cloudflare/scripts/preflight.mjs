import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { validatePublicOrigin } from "../src/boundary.ts";
import { runtimeEnvironment } from "../src/container.ts";
import { readPrivateJson } from "./preflight-input.mjs";

// Conservative byte interpretation of the documented 5 KB binding limit.
const MAX_BINDING_BYTES = 5000;
const PLAN_KEYS = ["version", "accountId", "workerName", "origin", "sourceRevision", "imageDigest", "bundleSha256", "remoteConfig", "restoreConfigs", "databases"];
const REPORT_KEYS = ["version", "kind", "checkedAtMs", "planSha256", "account", "worker", "turso", "image", "bundle"];
export class PreflightError extends Error {
  constructor(code) { super("公開前検査に失敗しました。"); this.name = "PreflightError"; this.code = code; }
}
function stable(value) {
  if (Array.isArray(value)) return value.map(stable);
  if (value && typeof value === "object") return Object.fromEntries(Object.keys(value).sort().map((key) => [key, stable(value[key])]));
  return value;
}
export function planFingerprint(plan, secrets) {
  return createHash("sha256").update(JSON.stringify(stable({ plan, secrets }))).digest("hex");
}
function reject(code) { throw new PreflightError(code); }
function record(value) { return value !== null && typeof value === "object" && !Array.isArray(value); }
function exact(value, keys, code = "invalid_input") {
  if (!record(value) || Object.keys(value).length !== keys.length || keys.some((key) => !Object.hasOwn(value, key))) reject(code);
}
function integer(value) { return Number.isSafeInteger(value) && value >= 0; }
function identifier(value) { return typeof value === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(value); }
function sum(values) {
  let value = 0;
  for (const item of values) { if (!integer(item) || !integer(value += item)) reject("capacity"); }
  return value;
}
function capacity(used, limit, additional) {
  if (!integer(used) || !integer(limit) || !integer(additional) || used >= limit || sum([used, additional]) > limit) reject("capacity");
}
function endpoint(raw) {
  if (typeof raw !== "string" || /[\s\\?#]/u.test(raw)) reject("database");
  const authority = raw.split("://")[1]?.split(/[/?#]/)[0];
  if (!authority || authority.includes("@")) reject("database");
  let url;
  try { url = new URL(raw.replace(/^libsql:/i, "https:")); } catch { reject("database"); }
  if (url.protocol !== "https:" || !url.hostname || url.username || url.password || url.search || url.hash || url.pathname !== "/") reject("database");
  return url.href;
}
function configuration(plan, secrets) {
  if (!Array.isArray(plan.restoreConfigs) || plan.restoreConfigs.length > 8 || !record(secrets)) reject("invalid_plan");
  const references = new Set(), endpoints = new Set(), databases = [], profiles = [];
  for (const [index, config] of [plan.remoteConfig, ...plan.restoreConfigs].entries()) {
    let origin, environment;
    try {
      origin = validatePublicOrigin(plan.origin);
      environment = runtimeEnvironment({ ...secrets, MEIBO_ORIGIN: plan.origin, MEIBO_REMOTE_CONFIG: JSON.stringify(config) }, origin);
    } catch { reject("configuration"); }
    if (Buffer.byteLength(environment.MEIBO_REMOTE_CONFIG) > MAX_BINDING_BYTES) reject("binding_limit");
    const profileNames = [], namespace = index === 0 ? "source" : `restore${index - 1}`;
    for (const [key, reference] of [[`${namespace}:control`, config.control],
      ...Object.entries(config.tenants).map(([slug, value]) => [`${namespace}:tenant:${slug}`, value])]) {
      for (const name of [reference.url_env, reference.token_env]) {
        const value = secrets[name];
        if (references.has(name) || typeof value !== "string" || value.length === 0 || Buffer.byteLength(value) > MAX_BINDING_BYTES) reject("secret");
        references.add(name); profileNames.push(name);
      }
      if (!/^[!-~]+$/.test(secrets[reference.token_env])) reject("secret");
      const url = endpoint(secrets[reference.url_env]);
      if (endpoints.has(url)) reject("database");
      endpoints.add(url); databases.push({ key, url });
    }
    profiles.push(profileNames);
  }
  if (Object.keys(secrets).length !== references.size || Object.keys(secrets).some((key) => !references.has(key))) reject("secret");
  return { databases, profiles };
}
function bindings(worker, profiles) {
  if (!Array.isArray(worker.existingBindings) || worker.existingBindings.length > 1024) reject("binding_limit");
  const existing = new Map();
  for (const binding of worker.existingBindings) {
    exact(binding, ["name", "type", "providerAccepted"]);
    if (typeof binding.name !== "string" || !/^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(binding.name)
      || !["secret_text", "plain_text", "json", "other"].includes(binding.type)
      || binding.providerAccepted !== true || existing.has(binding.name)) reject("binding_limit");
    existing.set(binding.name, binding.type);
  }
  for (const [index, references] of profiles.entries()) {
    // Only source is deployed now. A restored candidate gets a fresh source
    // preflight, including then-current bindings, before any later switch.
    const names = new Set(index === 0 ? [...existing].filter(([, type]) => type !== "other").map(([name]) => name) : []);
    for (const name of ["MEIBO_ORIGIN", "MEIBO_REMOTE_CONFIG", ...references]) {
      if (index === 0 && existing.get(name) === "other") reject("binding_limit");
      names.add(name);
    }
    if (names.size > 128) reject("binding_limit");
  }
}
function provider(plan, report, graph) {
  const turso = report.turso;
  exact(turso, ["organizationVerified", "bytesUsed", "bytesLimit", "writesUsed", "writesLimit", "databases"]);
  if (turso.organizationVerified !== true || !Array.isArray(turso.databases) || !Array.isArray(plan.databases)
    || plan.databases.length !== graph.databases.length || turso.databases.length !== graph.databases.length) reject("database");
  const planned = new Map(), identities = new Set(), facts = new Map();
  for (const item of plan.databases) {
    exact(item, ["key", "id", "additionalBytes", "additionalWrites"]);
    if (typeof item.key !== "string" || !identifier(item.id) || identities.has(item.id) || planned.has(item.key)
      || !integer(item.additionalBytes) || !integer(item.additionalWrites)) reject("database");
    planned.set(item.key, item); identities.add(item.id);
  }
  for (const item of turso.databases) {
    exact(item, ["id", "apiVerified", "cliVerified", "databaseTokenVerified", "instances", "cliUrl", "bytesUsed", "bytesLimit", "writesUsed", "writesLimit"]);
    if (!identifier(item.id) || facts.has(item.id)) reject("database");
    facts.set(item.id, item);
  }
  for (const node of graph.databases) {
    const item = planned.get(node.key), fact = facts.get(item?.id);
    if (!item || !fact || fact.apiVerified !== true || fact.cliVerified !== true || fact.databaseTokenVerified !== true
      || !Array.isArray(fact.instances) || fact.instances.length !== 1) reject("database");
    const primary = fact.instances[0]; exact(primary, ["role", "hostname"]);
    const hostname = typeof primary.hostname === "string" ? primary.hostname.replace(/\.$/, "") : "";
    if (primary.role !== "primary" || !hostname || hostname.length > 253
      || !hostname.split(".").every((label) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(label))
      || endpoint(`https://${primary.hostname}`) !== node.url || endpoint(fact.cliUrl) !== node.url) reject("database");
    capacity(fact.bytesUsed, fact.bytesLimit, item.additionalBytes);
    capacity(fact.writesUsed, fact.writesLimit, item.additionalWrites);
  }
  capacity(turso.bytesUsed, turso.bytesLimit, sum(plan.databases.map((item) => item.additionalBytes)));
  capacity(turso.writesUsed, turso.writesLimit, sum(plan.databases.map((item) => item.additionalWrites)));
}
export function validatePreflight(input) {
  try {
    const { plan, report, secrets, expectedAccountId, kind, nowMs = Date.now() } = input;
    exact(plan, PLAN_KEYS, "invalid_plan"); exact(report, REPORT_KEYS, "invalid_report");
    if (plan.version !== 1 || report.version !== 1 || !["fixture", "live"].includes(kind) || kind !== report.kind) reject("invalid_report");
    if (!integer(nowMs) || !integer(report.checkedAtMs) || nowMs - report.checkedAtMs > 300000 || report.checkedAtMs - nowMs > 30000
      || report.planSha256 !== planFingerprint(plan, secrets)) reject("stale_report");
    exact(report.account, ["id", "accessVerified", "workersPaid", "containersAllowed", "workersSubdomain"]);
    if (!/^[a-f0-9]{32}$/.test(expectedAccountId || "") || expectedAccountId !== plan.accountId
      || report.account.id !== expectedAccountId || report.account.accessVerified !== true) reject("account");
    if (report.account.workersPaid !== true || report.account.containersAllowed !== true) reject("contract");
    if (typeof report.account.workersSubdomain !== "string" || !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(report.account.workersSubdomain)
      || plan.workerName !== "meibo-beta" || plan.origin !== `https://${plan.workerName}.${report.account.workersSubdomain}.workers.dev`) reject("origin");
    exact(report.worker, ["name", "accountId", "ownershipVerified", "existingBindings"]);
    if (report.worker.name !== plan.workerName || report.worker.accountId !== expectedAccountId || report.worker.ownershipVerified !== true) reject("worker");
    const graph = configuration(plan, secrets); bindings(report.worker, graph.profiles); provider(plan, report, graph);
    exact(report.image, ["os", "architecture", "digest", "sourceRevision", "bootVerified", "nonRootUid", "credentialsInImage"]);
    exact(report.bundle, ["sha256", "sourceRevision", "buildVerified"]);
    if (typeof plan.sourceRevision !== "string" || typeof plan.imageDigest !== "string" || typeof plan.bundleSha256 !== "string"
      || !/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(plan.sourceRevision) || !/^sha256:[a-f0-9]{64}$/.test(plan.imageDigest)
      || !/^[a-f0-9]{64}$/.test(plan.bundleSha256) || report.image.os !== "linux" || report.image.architecture !== "amd64"
      || report.image.digest !== plan.imageDigest || report.image.sourceRevision !== plan.sourceRevision || report.image.bootVerified !== true
      || report.image.nonRootUid !== 10001 || report.image.credentialsInImage !== false || report.bundle.sha256 !== plan.bundleSha256
      || report.bundle.sourceRevision !== plan.sourceRevision || report.bundle.buildVerified !== true) reject("artifact");
    return { ok: true, kind, deploymentAuthorized: false, imageDigest: plan.imageDigest, bundleSha256: plan.bundleSha256,
      secretBatchCount: Math.ceil((graph.profiles[0].length + 1) / 100) };
  } catch (error) {
    if (error instanceof PreflightError) throw error;
    throw new PreflightError("invalid_input");
  }
}
async function cli() {
  try {
    const values = new Map(); const args = process.argv.slice(2);
    const allowed = ["--plan", "--report", "--secrets", "--expected-account-env", "--kind"];
    if (args.length !== allowed.length * 2) reject("invalid_input");
    for (let i = 0; i < args.length; i += 2) {
      if (!allowed.includes(args[i]) || !args[i + 1] || values.has(args[i])) reject("invalid_input");
      values.set(args[i], args[i + 1]);
    }
    if (values.get("--expected-account-env") !== "MEIBO_EXPECTED_CF_ACCOUNT_ID") reject("invalid_input");
    const read = (name) => readPrivateJson(values.get(name));
    const [plan, report, secrets] = await Promise.all([read("--plan"), read("--report"), read("--secrets")]);
    const result = validatePreflight({ plan, report, secrets, expectedAccountId: process.env[values.get("--expected-account-env")], kind: values.get("--kind") });
    console.log(JSON.stringify({ ...result, deploymentAuthorized: false }));
  } catch (error) {
    console.log(JSON.stringify({ ok: false, code: error instanceof PreflightError ? error.code : "invalid_input", deploymentAuthorized: false })); process.exitCode = 1;
  }
}
if (resolve(process.argv[1] || "") === fileURLToPath(import.meta.url)) await cli();
