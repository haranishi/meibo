import { createHash } from "node:crypto";
import { lstat, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { runtimeEnvironment } from "../src/container.ts";
import { validatePublicOrigin } from "../src/boundary.ts";
import { readPrivateJson } from "./preflight-input.mjs";
import { ProviderError } from "./providers.mjs";
import { capture, cloudRoot, projectRoot, wrangler } from "./runtime-io.mjs";

const docker = process.env.MEIBO_DOCKER || "docker";
export function sha256(value) { return createHash("sha256").update(value).digest("hex"); }
function requireArtifact(condition) { if (!condition) throw new ProviderError("artifact"); }
export async function privateDirectory(path) {
  const stat = await lstat(path);
  requireArtifact(stat.isDirectory() && !stat.isSymbolicLink() && (stat.mode & 0o077) === 0);
}
export async function writePrivate(path, value) {
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600, flag: "wx" });
}

/** Freeze all Git-visible image, Worker and deployment-tool inputs, including
 * untracked new source; private ignored files never enter the manifest. */
export async function sourceSnapshot() {
  const output = await capture("git", ["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "Cargo.toml", "Cargo.lock", "crates", "cloudflare", ".dockerignore"]);
  const files = [...new Set(output.split("\0").filter(Boolean))].sort();
  requireArtifact(files.includes("cloudflare/src/index.ts") && files.includes("cloudflare/Dockerfile"));
  const rows = [];
  for (const path of files) {
    const absolute = join(projectRoot, path), stat = await lstat(absolute);
    requireArtifact(stat.isFile() && !stat.isSymbolicLink());
    rows.push({ path, sha256: sha256(await readFile(absolute)) });
  }
  return { sourceRevision: sha256(JSON.stringify(rows)), files: rows };
}

async function workerBundle(out) {
  await wrangler(["deploy", "--config", "wrangler.jsonc", "--dry-run", "--containers-rollout", "none", "--outdir", out], { timeout: 120000 });
  const bytes = await readFile(join(out, "index.js"));
  requireArtifact(bytes.length > 0 && bytes.length < 64 * 1024 * 1024);
  return { sha256: sha256(bytes), bytes: bytes.length };
}
async function inspectImage(image, sourceRevision) {
  requireArtifact(/^[a-z0-9][a-z0-9._/-]*:[a-zA-Z0-9._-]+$/.test(image));
  const [info] = JSON.parse(await capture(docker, ["image", "inspect", image]));
  requireArtifact(info?.Os === "linux" && info.Architecture === "amd64" && info.Config?.User === "10001:10001"
    && info.Config.Labels?.["org.opencontainers.image.revision"] === sourceRevision && /^sha256:[a-f0-9]{64}$/.test(info.Id)
    && info.Size > 0 && info.Size <= 2 * 1024 ** 3 && !info.Config.Env?.some((v) => /^MEIBO_(?:DB_|REMOTE_CONFIG=)/.test(v)));
  return info;
}

/** Re-hash every OCI blob and compare the config to the actual local runtime. */
export async function verifyOci(path, info, expectedDigest, secretValues) {
  const layout = JSON.parse(await readFile(join(path, "oci-layout"), "utf8"));
  const index = JSON.parse(await readFile(join(path, "index.json"), "utf8"));
  requireArtifact(layout.imageLayoutVersion === "1.0.0" && index.manifests?.length === 1);
  const descriptor = index.manifests[0];
  requireArtifact(descriptor.digest === expectedDigest && descriptor.mediaType === "application/vnd.oci.image.manifest.v1+json");
  async function blob(item) {
    requireArtifact(/^sha256:[a-f0-9]{64}$/.test(item.digest) && Number.isSafeInteger(item.size) && item.size >= 0 && item.size <= 256 * 1024 * 1024);
    const file = join(path, "blobs", "sha256", item.digest.slice(7)), stat = await lstat(file);
    requireArtifact(stat.isFile() && !stat.isSymbolicLink() && stat.size === item.size);
    const bytes = await readFile(file);
    requireArtifact(`sha256:${sha256(bytes)}` === item.digest && !secretValues.some((value) => bytes.includes(Buffer.from(value))));
    return bytes;
  }
  const manifest = JSON.parse(await blob(descriptor));
  requireArtifact(manifest.config?.digest === info.Id && manifest.layers?.length === info.RootFS?.Layers?.length);
  await blob(manifest.config);
  for (const [index, layer] of manifest.layers.entries()) {
    requireArtifact(layer.digest === info.RootFS.Layers[index] && layer.mediaType === "application/vnd.oci.image.layer.v1.tar");
    await blob(layer);
  }
  return { digest: descriptor.digest, configDigest: info.Id };
}

/** Read-only boot proof with the exact immutable image config. The container is
 * private, synthetic/approved DB selection is explicit, and no update is sent. */
export async function bootImage(image, plan, secrets) {
  const runtime = runtimeEnvironment({ ...secrets, MEIBO_ORIGIN: plan.origin,
    MEIBO_REMOTE_CONFIG: JSON.stringify(plan.remoteConfig) }, validatePublicOrigin(plan.origin));
  const args = ["run", "--detach", "--platform", "linux/amd64", "--log-driver", "none", "--publish", "127.0.0.1::4317"];
  for (const name of Object.keys(runtime)) args.push("--env", name);
  const id = await capture(docker, [...args, image], { env: { ...process.env, ...runtime } });
  requireArtifact(/^[a-f0-9]{64}$/.test(id));
  try {
    const ports = JSON.parse(await capture(docker, ["inspect", "--format", "{{json .NetworkSettings.Ports}}", id]));
    const binding = ports["4317/tcp"];
    requireArtifact(binding?.length === 1 && binding[0].HostIp === "127.0.0.1" && /^[0-9]+$/.test(binding[0].HostPort));
    const address = `http://127.0.0.1:${binding[0].HostPort}`;
    const deadline = Date.now() + 30000; let ready = false;
    while (Date.now() < deadline) {
      try {
        const response = await fetch(`${address}/health`, { headers: { Host: new URL(plan.origin).host }, redirect: "manual", signal: AbortSignal.timeout(1000) });
        await response.body?.cancel(); if (response.status === 200) { ready = true; break; }
      } catch { /* Read-only startup polling only. */ }
      await new Promise((done) => setTimeout(done, 100));
    }
    requireArtifact(ready);
    const login = await fetch(`${address}/login`, { headers: { Host: new URL(plan.origin).host }, redirect: "manual", signal: AbortSignal.timeout(30000) });
    const text = await login.text();
    requireArtifact(login.status === 200 && text.includes('id="login-submit"') && text.includes(plan.origin)
      && login.headers.get("Content-Security-Policy") && login.headers.get("Cache-Control")?.includes("no-store"));
    const configMode = await capture(docker, ["exec", id, "stat", "-c", "%a:%u", "/tmp/meibo-remote.json"]);
    requireArtifact(configMode === "600:10001");
    return { bootVerified: true, nonRootUid: 10001 };
  } finally {
    await capture(docker, ["stop", "--time", "30", id], { timeout: 45000 });
    await capture(docker, ["rm", id]);
  }
}

/** Preparation may build, inspect and boot locally. It never calls provider
 * management APIs, registers secrets, deploys a Worker, or changes a plan. */
export async function prepareArtifacts(plan, secrets, image, out) {
  await privateDirectory(out);
  const snapshot = await sourceSnapshot();
  await capture(docker, ["buildx", "build", "--platform", "linux/amd64", "--load", "--provenance=false",
    "--build-arg", `MEIBO_SOURCE_REVISION=${snapshot.sourceRevision}`, "--tag", image, "--file", "cloudflare/Dockerfile", "."], { timeout: 1800000, limit: 16 * 1024 * 1024 });
  const info = await inspectImage(image, snapshot.sourceRevision);
  const ociPath = join(out, "oci");
  const oci = JSON.parse(await capture(process.env.MEIBO_PYTHON || "python3", [join(cloudRoot, "scripts/image-oci.py"), image, ociPath],
    { input: JSON.stringify(Object.values(secrets)), timeout: 120000, env: { ...process.env, MEIBO_DOCKER: docker } }));
  await verifyOci(ociPath, info, oci.digest, Object.values(secrets));
  const bundle = await workerBundle(join(out, "bundle"));
  requireArtifact((await sourceSnapshot()).sourceRevision === snapshot.sourceRevision);
  const prepared = { ...plan, sourceRevision: snapshot.sourceRevision, imageDigest: oci.digest, bundleSha256: bundle.sha256 };
  await writePrivate(join(out, "source.json"), snapshot);
  await writePrivate(join(out, "plan.json"), prepared);
  await writePrivate(join(out, "local-artifacts.json"), { imageConfigDigest: info.Id, ociManifestDigest: oci.digest,
    imageBytes: info.Size, workerBytes: bundle.bytes, bootVerified: false, credentialsInImage: false });
  return prepared;
}

/** Verification does not accept saved boot/build success assertions. It hashes
 * the source and OCI again, rebuilds the bundle, and boots the actual image. */
export async function verifyArtifacts(plan, secrets, image, out) {
  await privateDirectory(out);
  requireArtifact((await sourceSnapshot()).sourceRevision === plan.sourceRevision);
  const info = await inspectImage(image, plan.sourceRevision);
  await verifyOci(join(out, "oci"), info, plan.imageDigest, Object.values(secrets));
  const scratch = await mkdtemp(join(out, "verify-"));
  try {
    const bundle = await workerBundle(join(scratch, "bundle"));
    requireArtifact(bundle.sha256 === plan.bundleSha256
      && sha256(await readFile(join(out, "bundle/index.js"))) === plan.bundleSha256);
    const boot = await bootImage(image, plan, secrets);
    requireArtifact((await sourceSnapshot()).sourceRevision === plan.sourceRevision);
    return {
      image: { os: "linux", architecture: "amd64", digest: plan.imageDigest, sourceRevision: plan.sourceRevision,
        ...boot, credentialsInImage: false },
      bundle: { sha256: bundle.sha256, sourceRevision: plan.sourceRevision, buildVerified: true },
      imageConfigDigest: info.Id,
    };
  } finally { await rm(scratch, { recursive: true, force: true }); }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const args = process.argv.slice(2);
    if (args.length !== 8 || args[0] !== "--plan" || args[2] !== "--secrets" || args[4] !== "--image" || args[6] !== "--out") throw new ProviderError("invalid_input");
    const plan = await readPrivateJson(args[1]), secrets = await readPrivateJson(args[3]);
    await prepareArtifacts(plan, secrets, args[5], resolve(args[7]));
    process.stdout.write(`${JSON.stringify({ ok: true, prepared: true, deployed: false })}\n`);
  } catch (error) {
    process.stderr.write(`${JSON.stringify({ ok: false, code: error instanceof ProviderError ? error.code : "artifact", deployed: false })}\n`);
    process.exitCode = 1;
  }
}
