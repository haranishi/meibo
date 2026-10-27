import { randomUUID } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { parse } from "jsonc-parser";
import { runtimeEnvironment } from "../src/container.ts";
import { validatePublicOrigin } from "../src/boundary.ts";
import { imageConfigDigest, sha256, sourceSnapshot, writePrivate } from "./artifacts.mjs";
import { ProviderError } from "./providers.mjs";
import { capture, cloudRoot, wrangler } from "./runtime-io.mjs";

function requirePublication(condition) { if (!condition) throw new ProviderError("publication"); }
export function secretBatches(plan, secrets) {
  const runtime = runtimeEnvironment({ ...secrets, MEIBO_ORIGIN: plan.origin, MEIBO_REMOTE_CONFIG: JSON.stringify(plan.remoteConfig) }, validatePublicOrigin(plan.origin));
  const entries = Object.entries(runtime).filter(([name]) => name !== "MEIBO_ORIGIN");
  const batches = [];
  for (let index = 0; index < entries.length; index += 100) batches.push(Object.fromEntries(entries.slice(index, index + 100)));
  return batches;
}

/** Deploys only a previously absent target. Every secret batch carries the real
 * code with workers.dev disabled; the public route is enabled after completion.
 * No placeholder script, default account, force-stop, or upgrade command exists. */
export function publishingIo({ provider, image, artifacts }) {
  const docker = process.env.MEIBO_DOCKER || "docker", env = provider.deploymentEnv;
  async function cfOk(route) {
    const response = await provider.cf(route);
    requirePublication(response.status === 200 && response.body?.success === true); return response.body.result;
  }
  async function uploadedBundleHash(plan) {
    const response = await fetch(`https://api.cloudflare.com/client/v4/accounts/${plan.accountId}/workers/scripts/${plan.workerName}/content`, {
      headers: { Authorization: `Bearer ${env.CLOUDFLARE_API_TOKEN}` }, redirect: "error", signal: AbortSignal.timeout(30000),
    });
    requirePublication(response.status === 200);
    const chunks = []; let size = 0;
    for await (const chunk of response.body ?? []) {
      size += chunk.length; requirePublication(size <= 64 * 1024 * 1024); chunks.push(chunk);
    }
    const bytes = Buffer.concat(chunks);
    if (!response.headers.get("Content-Type")?.startsWith("multipart/")) return sha256(bytes);
    const parts = await new Response(bytes, { headers: response.headers }).formData();
    const module = parts.get("index.js");
    requirePublication(module && typeof module !== "string");
    return sha256(Buffer.from(await module.arrayBuffer()));
  }
  async function deployedMetadata(plan, registry, publiclyEnabled) {
    const base = `/accounts/${plan.accountId}/workers/scripts/${plan.workerName}`;
    const settings = await cfOk(`${base}/settings`);
    requirePublication(Array.isArray(settings.bindings) && settings.observability?.enabled === false
      && await uploadedBundleHash(plan) === plan.bundleSha256);
    const namespace = settings.bindings.filter((b) => b.name === "MEIBO_CONTAINER" && b.type === "durable_object_namespace");
    requirePublication(namespace.length === 1 && /^[a-f0-9]{32}$/.test(namespace[0].namespace_id));
    const apps = await cfOk(`/accounts/${plan.accountId}/containers/applications`);
    requirePublication(Array.isArray(apps));
    const matched = apps.filter((app) => app.durable_objects?.namespace_id === namespace[0].namespace_id);
    requirePublication(matched.length === 1 && matched[0].configuration?.image === registry.image
      && matched[0].configuration.instance_type === "lite" && matched[0].max_instances === 1);
    const subdomain = await cfOk(`${base}/subdomain`);
    requirePublication(subdomain.enabled === publiclyEnabled && subdomain.previews_enabled === false);
    const deployments = (await cfOk(`${base}/deployments`)).deployments;
    requirePublication(Array.isArray(deployments) && deployments[0]?.versions?.length === 1);
    const version = deployments[0].versions[0];
    requirePublication(version.percentage === 100 && /^[a-f0-9-]{36}$/.test(version.version_id));
    return { workerVersion: version.version_id, containerApplicationId: matched[0].id,
      durableObjectNamespaceId: namespace[0].namespace_id, registryImage: registry.image, publiclyEnabled };
  }
  return {
    async push(plan) {
      requirePublication(/^meibo-beta:[a-zA-Z0-9_.-]+$/.test(image));
      await wrangler(["containers", "push", image, "--config", join(cloudRoot, "wrangler.jsonc")], { env, timeout: 300000 });
      const [info] = JSON.parse(await capture(docker, ["image", "inspect", image]));
      const prefix = `registry.cloudflare.com/${plan.accountId}/meibo-beta@`;
      const references = info.RepoDigests?.filter((ref) => ref.startsWith(prefix));
      requirePublication(references?.length === 1);
      const reference = references[0], digest = reference.split("@")[1];
      const manifest = JSON.parse(await capture(docker, ["manifest", "inspect", "--verbose", reference], { timeout: 120000 }));
      const configDigest = (manifest.SchemaV2Manifest ?? manifest.OCIManifest)?.config?.digest;
      requirePublication(manifest.Descriptor?.digest === digest && configDigest === imageConfigDigest(info));
      return { digest, configDigest, image: reference };
    },
    async install(plan, secrets, registry, checkedAtMs) {
      const errors = [], tracked = parse(await readFile(join(cloudRoot, "wrangler.jsonc"), "utf8"), errors);
      requirePublication(errors.length === 0 && env.CLOUDFLARE_ACCOUNT_ID === plan.accountId);
      const bundlePath = join(artifacts, "bundle/index.js");
      const candidate = { ...tracked, main: bundlePath, account_id: plan.accountId, workers_dev: false,
        vars: { MEIBO_ORIGIN: plan.origin }, containers: tracked.containers.map(({ image_build_context, ...container }) => ({ ...container, image: registry.image })) };
      delete candidate.$schema;
      const configPath = join(artifacts, `deploy-${randomUUID()}.json`);
      await writePrivate(configPath, candidate);
      async function unchanged() {
        requirePublication(Date.now() - checkedAtMs <= 300000 && checkedAtMs <= Date.now()
          && (await sourceSnapshot()).sourceRevision === plan.sourceRevision
          && sha256(await readFile(bundlePath)) === plan.bundleSha256);
      }
      const batches = secretBatches(plan, secrets);
      for (const batch of batches) {
        await unchanged();
        // /dev/stdin is read by the pinned Wrangler parseBulkInputToObject.
        // The value is piped; no secret file or secret argv value is created.
        await wrangler(["deploy", "--no-bundle", "--config", configPath, "--secrets-file", "/dev/stdin"],
          { input: `${JSON.stringify(batch)}\n`, env, timeout: 180000 });
      }
      const required = batches.flatMap((batch) => Object.keys(batch));
      const privateVersion = await deployedMetadata(plan, registry, false);
      const settings = await cfOk(`/accounts/${plan.accountId}/workers/scripts/${plan.workerName}/settings`);
      requirePublication(required.every((name) => settings.bindings.some((binding) => binding.name === name && binding.type === "secret_text")));
      await writePrivate(join(artifacts, `private-version-${randomUUID()}.json`), privateVersion);
      await unchanged();
      const publicConfig = { ...candidate, workers_dev: true, secrets: { required } };
      await writeFile(configPath, `${JSON.stringify(publicConfig)}\n`, { mode: 0o600 });
      await wrangler(["deploy", "--no-bundle", "--config", configPath], { env, timeout: 180000 });
      return deployedMetadata(plan, registry, true);
    },
  };
}
