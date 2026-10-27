import { randomUUID } from "node:crypto";
import { resolve, join } from "node:path";
import { fileURLToPath } from "node:url";
import { privateDirectory, verifyArtifacts, writePrivate } from "./artifacts.mjs";
import { readPrivateJson } from "./preflight-input.mjs";
import { collectProviders, ProviderError } from "./providers.mjs";
import { publication } from "./publication.mjs";
import { publishingIo } from "./publishing-io.mjs";
import { liveProviderAdapters } from "./runtime-io.mjs";

export async function deploy(args) {
  const flags = ["--plan", "--secrets", "--image", "--artifacts", "--mode"];
  if (args.length !== flags.length * 2 || flags.some((flag, i) => args[i * 2] !== flag)) throw new ProviderError("invalid_input");
  const [, planPath, , secretsPath, , image, , artifactPath, , mode] = args;
  const expectedAccountId = process.env.MEIBO_EXPECTED_CF_ACCOUNT_ID;
  if (process.env.CLOUDFLARE_ACCOUNT_ID !== expectedAccountId || !/^[a-f0-9]{32}$/.test(expectedAccountId || "")) throw new ProviderError("account");
  const plan = await readPrivateJson(planPath), secrets = await readPrivateJson(secretsPath), artifacts = resolve(artifactPath);
  if (plan.accountId !== expectedAccountId || !/^meibo-beta:[a-zA-Z0-9_.-]+$/.test(image) || !["check", "publish"].includes(mode)) throw new ProviderError("invalid_input");
  await privateDirectory(artifacts);
  const provider = await liveProviderAdapters(expectedAccountId, process.env.MEIBO_TURSO_ORGANIZATION);
  const publisher = publishingIo({ provider, image, artifacts });
  return publication(plan, secrets, { expectedAccountId, mode, io: {
    collect: (currentPlan, values) => collectProviders(currentPlan, values, provider),
    verify: (currentPlan, values) => verifyArtifacts(currentPlan, values, image, artifacts),
    record: async (report, currentPlan) => writePrivate(join(artifacts, `live-preflight-${randomUUID()}.json`), { plan: currentPlan, report }),
    push: publisher.push, install: publisher.install,
    recordDeployment: async (record) => writePrivate(join(artifacts, `deployment-${randomUUID()}.json`), record),
  } });
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { process.stdout.write(`${JSON.stringify(await deploy(process.argv.slice(2)))}\n`); }
  catch (error) {
    process.stderr.write(`${JSON.stringify({ ok: false, code: error?.code || "publication", deployed: error?.installAttempted ? "unknown" : false })}\n`);
    process.exitCode = 1;
  }
}
