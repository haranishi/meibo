import { runtimeEnvironment } from "../src/container.ts";
import { validatePublicOrigin } from "../src/boundary.ts";

export class ProviderError extends Error {
  constructor(code = "provider") { super("提供者の照合に失敗しました。"); this.code = code; }
}
function requireValue(condition) { if (!condition) throw new ProviderError(); }
function number(value) { requireValue(Number.isSafeInteger(value) && value >= 0); return value; }
function bytes(value) {
  if (Number.isSafeInteger(value)) return number(value);
  requireValue(typeof value === "string" && /^\d+(?:kb|mb|gb)?$/i.test(value));
  const [, count, unit = ""] = /^(\d+)(kb|mb|gb)?$/i.exec(value);
  return number(Number(count) * ({ "": 1, kb: 1024, mb: 1024 ** 2, gb: 1024 ** 3 }[unit.toLowerCase()]));
}
function endpoint(value) {
  requireValue(typeof value === "string" && !/[\s\\?#]/u.test(value));
  const url = new URL(value.replace(/^libsql:/, "https:"));
  requireValue(url.protocol === "https:" && !url.username && !url.password && !url.search && !url.hash && url.pathname === "/");
  return url.href;
}
function sourceGraph(plan, secrets) {
  const graph = [];
  for (const [index, config] of [plan.remoteConfig, ...plan.restoreConfigs].entries()) {
    runtimeEnvironment({ ...secrets, MEIBO_ORIGIN: plan.origin, MEIBO_REMOTE_CONFIG: JSON.stringify(config) }, validatePublicOrigin(plan.origin));
    const prefix = index === 0 ? "source" : `restore${index - 1}`;
    graph.push({ key: `${prefix}:control`, reference: config.control });
    for (const [slug, reference] of Object.entries(config.tenants)) graph.push({ key: `${prefix}:tenant:${slug}`, reference });
  }
  return graph;
}

/** All inputs are read-only. Only independently selected account/org are queried.
 * API adapters return {status, body}; CLI output stays in memory, never logs. */
export async function collectProviders(plan, secrets, { expectedAccountId, organization, cf, turso, instanceUrl, databaseRead }) {
  requireValue(/^[a-f0-9]{32}$/.test(expectedAccountId || "") && plan.accountId === expectedAccountId);
  requireValue(/^[a-z0-9][a-z0-9-]{0,62}$/.test(organization || ""));
  const graph = sourceGraph(plan, secrets), base = `/accounts/${expectedAccountId}`;
  async function cfOk(path) {
    const response = await cf(path);
    requireValue(response.status === 200 && response.body?.success === true);
    return response.body.result;
  }
  const account = await cfOk(base);
  requireValue(account.id === expectedAccountId);
  const subdomain = await cfOk(`${base}/workers/subdomain`);
  requireValue(plan.workerName === "meibo-beta" && plan.origin === `https://meibo-beta.${subdomain.subdomain}.workers.dev`);
  const containers = await cf(`${base}/containers/applications`);
  // Containers' successful entitlement check requires Workers Paid. Never infer
  // a subscription from default_usage_model or silently change a contract.
  const allowed = containers.status === 200 && containers.body?.success === true && Array.isArray(containers.body.result);
  const paidRequired = containers.status === 401 && containers.body?.success === false
    && containers.body.errors?.some((e) => e.code === 1000 && /workers paid/i.test(e.message));
  requireValue(allowed || paidRequired);
  const target = await cf(`${base}/workers/scripts/${encodeURIComponent(plan.workerName)}/settings`);
  const absent = target.status === 404 && target.body?.success === false && target.body.errors?.some((e) => e.code === 10007);
  requireValue(absent || (target.status === 200 && target.body?.success === true && Array.isArray(target.body.result?.bindings)));
  const existingBindings = absent ? [] : target.body.result.bindings.map((b) => ({
    name: b.name, type: ["secret_text", "plain_text", "json"].includes(b.type) ? b.type : "other", providerAccepted: true,
  }));
  async function tursoOk(route) {
    const response = await turso(`/v1/organizations/${organization}${route}`);
    requireValue(response.status === 200); return response.body;
  }
  const org = await tursoOk("");
  requireValue(org.organization?.slug === organization);
  const { subscription } = await tursoOk("/subscription");
  const { plans } = await tursoOk("/plans");
  requireValue(subscription && Array.isArray(plans));
  const selected = plans.filter((p) => p.name === subscription.plan);
  requireValue(selected.length === 1);
  const quota = selected[0].quotas;
  const usage = (await tursoOk("/usage")).organization?.usage;
  requireValue(usage && quota);
  const organizationCapacity = { bytesUsed: number(usage.storage_bytes), bytesLimit: number(quota.storage),
    writesUsed: number(usage.rows_written), writesLimit: number(quota.rowsWritten) };
  const listed = (await tursoOk("/databases")).databases;
  requireValue(Array.isArray(listed));
  const databases = [];
  for (const node of graph) {
    const mappings = plan.databases.filter((p) => p.key === node.key);
    requireValue(mappings.length === 1);
    const rows = listed.filter((db) => db.DbId === mappings[0].id);
    requireValue(rows.length === 1);
    const name = rows[0].Name;
    requireValue(typeof name === "string" && /^[a-z0-9][a-z0-9-]{0,62}$/.test(name));
    const route = `/databases/${name}`;
    const info = (await tursoOk(route)).database;
    requireValue(info?.DbId === mappings[0].id && info.Name === name && info.block_reads === false && info.block_writes === false);
    const { instances } = await tursoOk(`${route}/instances`);
    requireValue(Array.isArray(instances) && instances.length === 1 && instances[0].type === "primary");
    const instance = instances[0];
    requireValue(typeof instance.hostname === "string" && /^[a-z0-9.-]+\.turso\.io$/.test(instance.hostname));
    requireValue(typeof instance.name === "string" && /^[a-zA-Z0-9_-]{1,128}$/.test(instance.name));
    const primary = endpoint(`https://${instance.hostname}`);
    const cliUrl = await instanceUrl(name, instance.name);
    requireValue(endpoint(cliUrl) === primary && endpoint(secrets[node.reference.url_env]) === primary);
    const configuration = await tursoOk(`${route}/configuration`);
    requireValue(configuration.block_reads === false && configuration.block_writes === false);
    const usageResponse = await tursoOk(`${route}/usage`), dbUsage = usageResponse.database;
    // Current API returns the aggregate at the top level; earlier documented
    // responses nested it under database. Identity is required in both forms.
    const total = usageResponse.total ?? dbUsage?.total;
    requireValue(dbUsage?.uuid === mappings[0].id && total);
    const capacity = { bytesUsed: number(total.storage_bytes), bytesLimit: Math.min(bytes(configuration.size_limit), organizationCapacity.bytesLimit),
      writesUsed: number(total.rows_written), writesLimit: organizationCapacity.writesLimit };
    // Execute a constant read-only query with this DB-scoped token only after
    // independent API/CLI endpoint agreement; tokens never enter CLI arguments.
    requireValue(await databaseRead(primary, secrets[node.reference.token_env]));
    databases.push({ id: mappings[0].id, apiVerified: true, cliVerified: true, databaseTokenVerified: true,
      instances: [{ role: "primary", hostname: instance.hostname }], cliUrl,
      ...capacity });
  }
  return {
    account: { id: expectedAccountId, accessVerified: true, workersPaid: allowed, containersAllowed: allowed, workersSubdomain: subdomain.subdomain },
    // Initial publication requires a verified absent target. A pre-existing
    // Worker needs a separately reviewed update/drain path; never overwrite it.
    worker: { name: plan.workerName, accountId: expectedAccountId, ownershipVerified: absent, existingBindings },
    turso: { organizationVerified: true, ...organizationCapacity, databases },
  };
}
