import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { ProviderError } from "./providers.mjs";

export const projectRoot = fileURLToPath(new URL("../../", import.meta.url));
export const cloudRoot = fileURLToPath(new URL("../", import.meta.url));
export const wranglerPath = fileURLToPath(new URL("../node_modules/wrangler/bin/wrangler.js", import.meta.url));

/** Child diagnostics are deliberately not echoed: provider tools may include
 * credentials, bindings, or secret-bearing URLs even when a command fails. */
export function capture(command, args, { input, env = process.env, cwd = projectRoot, timeout = 120000, limit = 1048576 } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, env, stdio: ["pipe", "pipe", "pipe"], timeout });
    let size = 0; const output = []; let failed = false;
    function fail() { if (failed) return; failed = true; child.kill(); reject(new ProviderError("external_command")); }
    child.on("error", fail);
    child.stdout.on("data", (data) => { size += data.length; if (size > limit) fail(); else output.push(data); });
    child.stderr.on("data", (data) => { size += data.length; if (size > limit) fail(); });
    child.stdin.on("error", () => {});
    child.on("close", (code) => {
      if (failed) return;
      if (code !== 0) { fail(); return; }
      resolve(Buffer.concat(output).toString("utf8").trim());
    });
    child.stdin.end(input);
  });
}
export function wrangler(args, options = {}) {
  return capture(process.execPath, [wranglerPath, ...args], { cwd: cloudRoot, ...options,
    env: { ...process.env, WRANGLER_SEND_METRICS: "false", CI: "true", ...options.env } });
}

/** Bounded, verified HTTPS and no redirects or automatic retry. */
export async function requestJson(url, token, { method = "GET", body } = {}) {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:" || parsed.username || parsed.password || parsed.hash) throw new ProviderError();
    const response = await fetch(parsed, { method, redirect: "error", signal: AbortSignal.timeout(30000),
      headers: { Authorization: `Bearer ${token}`, Accept: "application/json", ...(body ? { "Content-Type": "application/json" } : {}) },
      ...(body ? { body: JSON.stringify(body) } : {}) });
    const chunks = []; let size = 0;
    for await (const chunk of response.body ?? []) {
      size += chunk.length; if (size > 1048576) throw new ProviderError(); chunks.push(chunk);
    }
    return { status: response.status, body: JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks))) };
  } catch { throw new ProviderError("provider_transport"); }
}
export async function liveProviderAdapters(expectedAccountId, organization) {
  const auth = process.env.CLOUDFLARE_API_TOKEN ? { token: process.env.CLOUDFLARE_API_TOKEN }
    : JSON.parse(await wrangler(["auth", "token", "--json"]));
  if (typeof auth.token !== "string" || !/^[!-~]+$/.test(auth.token)) throw new ProviderError("authentication");
  const tursoCli = process.env.MEIBO_TURSO_CLI || "turso";
  const tursoToken = await capture(tursoCli, ["auth", "token"]);
  if (!/^[!-~]+$/.test(tursoToken)) throw new ProviderError("authentication");
  return {
    expectedAccountId, organization,
    cf: (route) => requestJson(`https://api.cloudflare.com/client/v4${route}`, auth.token),
    turso: (route) => requestJson(`https://api.turso.tech${route}`, tursoToken),
    instanceUrl: (name, instance) => capture(tursoCli, ["db", "show", name, "--instance-url", instance]),
    databaseCapacity: async (primary, token) => {
      const response = await requestJson(new URL("/v2/pipeline", primary), token, { method: "POST", body: {
        requests: [
          { type: "execute", stmt: { sql: "PRAGMA page_size", want_rows: true } },
          { type: "execute", stmt: { sql: "PRAGMA max_page_count", want_rows: true } },
          { type: "close" },
        ],
      } });
      if (response.status !== 200 || response.body.results?.length !== 3) throw new ProviderError();
      const values = response.body.results.slice(0, 2).map((result) => {
        const rows = result.response?.result?.rows, value = rows?.[0]?.[0];
        if (result.type !== "ok" || result.response?.type !== "execute" || !Array.isArray(rows) || rows.length !== 1 || rows[0].length !== 1
          || value?.type !== "integer" || typeof value.value !== "string" || !/^[1-9][0-9]*$/.test(value.value)) throw new ProviderError();
        return Number(value.value);
      });
      const close = response.body.results[2];
      if (close.type !== "ok" || close.response?.type !== "close") throw new ProviderError();
      return { pageSize: values[0], maxPageCount: values[1] };
    },
    databaseRead: async (primary, token) => {
      const response = await requestJson(new URL("/v2/pipeline", primary), token, { method: "POST", body: {
        requests: [{ type: "execute", stmt: { sql: "SELECT 1 AS meibo_preflight", want_rows: true } }, { type: "close" }],
      } });
      const result = response.body.results?.[0];
      return response.status === 200 && result?.type === "ok" && result.response?.type === "execute"
        && result.response.result?.rows?.[0]?.[0]?.type === "integer" && result.response.result.rows[0][0].value === "1";
    },
    deploymentEnv: { CLOUDFLARE_ACCOUNT_ID: expectedAccountId, CLOUDFLARE_API_TOKEN: auth.token },
  };
}
