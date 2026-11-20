import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, readdir, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import http from "node:http";
import { containerProbe, sha256, verifiedLoginMode, verifyArtifactCommand, verifyOci } from "../scripts/artifacts.mjs";
import { planFingerprint } from "../scripts/preflight.mjs";

async function fixture(t, layerBytes = Buffer.from("synthetic layer data")) {
  const root = await mkdtemp(join(tmpdir(), "meibo-oci-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, "blobs/sha256"), { recursive: true, mode: 0o700 });
  async function blob(bytes, mediaType) {
    const digest = `sha256:${sha256(bytes)}`;
    await writeFile(join(root, "blobs/sha256", digest.slice(7)), bytes, { mode: 0o600 });
    return { digest, size: bytes.length, mediaType };
  }
  const layer = await blob(layerBytes, "application/vnd.oci.image.layer.v1.tar");
  const config = await blob(Buffer.from(JSON.stringify({ architecture: "amd64", os: "linux", rootfs: { type: "layers", diff_ids: [layer.digest] } })), "application/vnd.oci.image.config.v1+json");
  const manifest = await blob(Buffer.from(JSON.stringify({ schemaVersion: 2, mediaType: "application/vnd.oci.image.manifest.v1+json", config, layers: [layer] })), "application/vnd.oci.image.manifest.v1+json");
  await writeFile(join(root, "oci-layout"), JSON.stringify({ imageLayoutVersion: "1.0.0" }), { mode: 0o600 });
  await writeFile(join(root, "index.json"), JSON.stringify({ schemaVersion: 2, manifests: [manifest] }), { mode: 0o600 });
  return { root, config, manifest, layer, info: { Id: config.digest, RootFS: { Layers: [layer.digest] } } };
}

test("OCI proof verifies the actual manifest and every blob, distinct from the config ID", async (t) => {
  const f = await fixture(t);
  assert.notEqual(f.manifest.digest, f.config.digest);
  assert.deepEqual(await verifyOci(f.root, f.info, f.manifest.digest, ["private-credential-not-present"]), { digest: f.manifest.digest, configDigest: f.config.digest });
});
test("a saved manifest digest cannot stand in for changed image layer bytes", async (t) => {
  const f = await fixture(t), path = join(f.root, "blobs/sha256", f.layer.digest.slice(7));
  const bytes = await readFile(path); bytes[0] ^= 1; await writeFile(path, bytes);
  await assert.rejects(verifyOci(f.root, f.info, f.manifest.digest, ["private-credential-not-present"]));
});
test("a differently configured runtime cannot reuse a valid OCI proof", async (t) => {
  const f = await fixture(t); f.info.Id = `sha256:${"0".repeat(64)}`;
  await assert.rejects(verifyOci(f.root, f.info, f.manifest.digest, ["private-credential-not-present"]));
});
test("a containerd manifest image ID uses its bound config digest", async (t) => {
  const f = await fixture(t), imageId = `sha256:${"1".repeat(64)}`;
  f.info.Id = imageId;
  f.info.Descriptor = { digest: imageId, mediaType: "application/vnd.docker.distribution.manifest.v2+json",
    annotations: { "config.digest": f.config.digest } };
  assert.deepEqual(await verifyOci(f.root, f.info, f.manifest.digest, ["private-credential-not-present"]),
    { digest: f.manifest.digest, configDigest: f.config.digest });
});
test("a config annotation belonging to a different manifest is refused", async (t) => {
  const f = await fixture(t);
  f.info.Descriptor = { digest: `sha256:${"1".repeat(64)}`, mediaType: "application/vnd.docker.distribution.manifest.v2+json",
    annotations: { "config.digest": f.config.digest } };
  await assert.rejects(verifyOci(f.root, f.info, f.manifest.digest, ["private-credential-not-present"]));
});
test("a manifest without an explicit config identity is refused", async (t) => {
  const f = await fixture(t);
  f.info.Descriptor = { digest: f.info.Id, mediaType: "application/vnd.oci.image.manifest.v1+json" };
  await assert.rejects(verifyOci(f.root, f.info, f.manifest.digest, ["private-credential-not-present"]));
});
test("a credential embedded in any layer refuses publication", async (t) => {
  const f = await fixture(t, Buffer.from("header private-credential-do-not-publish tail"));
  await assert.rejects(verifyOci(f.root, f.info, f.manifest.digest, ["private-credential-do-not-publish"]));
});
test("an OCI blob symlink is rejected before its target is read", async (t) => {
  const f = await fixture(t), path = join(f.root, "blobs/sha256", f.layer.digest.slice(7));
  const copy = join(f.root, "private-copy"); await writeFile(copy, await readFile(path));
  await rm(path); await symlink(copy, path);
  await assert.rejects(verifyOci(f.root, f.info, f.manifest.digest, ["private-credential-not-present"]));
});
test("the actual container probe preserves the public Host and refuses redirects", async (t) => {
  let host, requests = 0;
  const server = http.createServer((req, res) => { host = req.headers.host; requests++; res.writeHead(302, { Location: "https://untrusted.invalid" }); res.end("synthetic"); });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  t.after(() => { server.closeAllConnections(); return new Promise((done) => server.close(done)); });
  const result = await containerProbe(`http://127.0.0.1:${server.address().port}`, "meibo-beta.fixture.workers.dev", "/health");
  assert.equal(host, "meibo-beta.fixture.workers.dev"); assert.equal(result.status, 302); assert.equal(requests, 1);
});
test("the private container probe rejects an external destination", () => {
  assert.throws(() => containerProbe("http://untrusted.invalid:4317", "meibo-beta.fixture.workers.dev", "/health"));
});

test("the boot probe identifies the login mode by its configuration and HTTP response", () => {
  const origin = "https://meibo.example.test";
  const headers = { "cache-control": "no-store", "content-security-policy": "default-src 'self'" };
  const passwordLogin = { status: 200, headers, body: `${origin}<input id="login-password"><button id="login-submit">` };
  const accessLogin = { status: 401, headers, body: "Cloudflareでログインし直してください。" };
  const access = { MEIBO_ACCESS_CONFIG: "synthetic" };
  assert.equal(verifiedLoginMode({}, origin, passwordLogin), "password");
  assert.equal(verifiedLoginMode(access, origin, accessLogin), "cloudflare-access");
  assert.throws(() => verifiedLoginMode(access, origin, passwordLogin));
  assert.throws(() => verifiedLoginMode({}, origin, accessLogin));
});

test("local boot checks the prepared plan on every run and preserves earlier evidence on failure", async (t) => {
  const dir = await mkdtemp(join(tmpdir(), "meibo-local-boot-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const plan = { sourceRevision: "1".repeat(64), imageDigest: `sha256:${"2".repeat(64)}`,
    bundleSha256: "3".repeat(64), origin: "https://meibo.example.test" };
  const secrets = { MEIBO_DB_TOKEN: "synthetic-secret-for-local-boot" };
  const secretFile = join(dir, "fixture.json"), planFile = join(dir, "plan.json");
  await writeFile(planFile, JSON.stringify(plan), { mode: 0o600 });
  await writeFile(secretFile, JSON.stringify(secrets), { mode: 0o600 });
  const args = ["--artifacts", dir, "--secrets", secretFile, "--image", "meibo-beta:fixture"];
  const proof = { image: { digest: plan.imageDigest, sourceRevision: plan.sourceRevision, bootVerified: true },
    bundle: { sha256: plan.bundleSha256, sourceRevision: plan.sourceRevision, buildVerified: true },
    imageConfigDigest: `sha256:${"4".repeat(64)}`, authenticationMode: "password" };
  const verify = async (planned, values, image, out) => {
    assert.deepEqual(planned, plan);
    assert.deepEqual(values, secrets);
    assert.equal(image, "meibo-beta:fixture");
    assert.equal(out, dir);
    return proof;
  };
  const first = await verifyArtifactCommand(args, verify);
  const second = await verifyArtifactCommand(args, verify);
  assert.notEqual(first.report, second.report);
  assert.equal(first.deploymentAuthorized, false);
  assert.equal(first.deployed, false);
  const path = join(dir, first.report), raw = await readFile(path, "utf8"), receipt = JSON.parse(raw);
  assert.equal((await stat(path)).mode & 0o777, 0o600);
  assert.equal(receipt.kind, "local-boot");
  assert.equal(receipt.planSha256, planFingerprint(plan, secrets));
  assert.deepEqual(receipt.image, proof.image);
  assert.deepEqual(receipt.bundle, proof.bundle);
  assert.equal(receipt.imageConfigDigest, proof.imageConfigDigest);
  assert.equal(receipt.authenticationMode, "password");
  assert.ok(!raw.includes(plan.origin) && !raw.includes(secrets.MEIBO_DB_TOKEN));
  // The real verifier must reject this stale revision before touching Docker.
  const before = (await readdir(dir)).sort();
  await assert.rejects(verifyArtifactCommand(args), (error) => error.code === "artifact");
  assert.deepEqual((await readdir(dir)).sort(), before);
  assert.equal(await readFile(planFile, "utf8"), JSON.stringify(plan));
  assert.equal(await readFile(path, "utf8"), raw);
});
