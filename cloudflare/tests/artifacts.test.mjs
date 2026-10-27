import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import http from "node:http";
import { containerProbe, sha256, verifyOci } from "../scripts/artifacts.mjs";

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
