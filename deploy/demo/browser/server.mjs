// 公開デモのブラウザ試験用サーバー。実バイナリ（--public-demo）を起動し、
// 自己署名のHTTPS前段（本番と同じSecure cookie・https originの条件）を立てる。
// 外部のサービスにも、実データにも接続しない。保存先は試験ごとの新しい一時ディレクトリ。
import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import http from "node:http";
import https from "node:https";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

const port = Number(process.env.MEIBO_DEMO_TEST_PORT || 4441);
if (!Number.isInteger(port) || port < 1024 || port > 65535) {
  throw new Error("MEIBO_DEMO_TEST_PORT must be a port from 1024 to 65535");
}
// MEIBO_DEMO_BACKEND_PORT を指定すると、バイナリを起動せず、そのポートで動いているバックエンド
// （例: `docker run -p 127.0.0.1:PORT:4317 -e MEIBO_DOMAIN=127.0.0.1:<前段のポート> ...` のコンテナ）の前にだけ、
// 自己署名のHTTPS前段を立てる。公開デモと同じ起動引数のイメージを、同じ試験で確かめるため。
const externalBackendPort = process.env.MEIBO_DEMO_BACKEND_PORT ? Number(process.env.MEIBO_DEMO_BACKEND_PORT) : 0;
if (!externalBackendPort && !process.env.MEIBO_BINARY) {
  throw new Error("MEIBO_BINARY must name the built meibo binary (or set MEIBO_DEMO_BACKEND_PORT)");
}
const binary = externalBackendPort ? "" : resolve(process.env.MEIBO_BINARY);
// 自己終了などの確認で、起動引数を足す（例: "--demo-idle-seconds 2"）。空白で区切る。
const extraArgs = (process.env.MEIBO_DEMO_EXTRA_ARGS || "").split(/\s+/).filter(Boolean);
const scratch = await mkdtemp(join(tmpdir(), "meibo-demo-browser-"));
const root = join(scratch, "demo-root");
let backend;
let front;

async function shutdown(code = 0) {
  try { front?.close(); } catch { /* 既に閉じている */ }
  try { backend?.kill("SIGTERM"); } catch { /* 既に終了している */ }
  await rm(scratch, { recursive: true, force: true });
  process.exit(code);
}
process.on("SIGTERM", () => shutdown(0));
process.on("SIGINT", () => shutdown(0));

const key = join(scratch, "front-key.pem");
const cert = join(scratch, "front-cert.pem");
const made = spawnSync(process.env.OPENSSL_BINARY || "openssl", [
  "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", key, "-out", cert,
  "-days", "1", "-subj", "/CN=127.0.0.1",
], { stdio: "ignore" });
if (made.status !== 0) throw new Error("local certificate creation failed");

let backendPort = 0;
front = https.createServer({ key: await readFile(key), cert: await readFile(cert) }, (request, response) => {
  const upstream = http.request({
    host: "127.0.0.1", port: backendPort, method: request.method, path: request.url, headers: request.headers,
  }, (incoming) => {
    response.writeHead(incoming.statusCode, incoming.headers);
    incoming.pipe(response);
  });
  upstream.on("error", () => { response.writeHead(502); response.end(); });
  request.pipe(upstream);
});
await new Promise((ready) => front.listen(port, "127.0.0.1", ready));

const origin = `https://127.0.0.1:${port}`;
if (externalBackendPort) {
  backendPort = externalBackendPort;
  console.log(`demo test server ready: ${origin} (external backend 127.0.0.1:${backendPort})`);
} else {
backend = spawn(binary, [
  "hosted", root, "serve", "--public-demo", "--origin", origin, "--bind", "127.0.0.1:0", ...extraArgs,
], { stdio: ["ignore", "pipe", "inherit"] });
const lines = createInterface({ input: backend.stdout });
backendPort = await new Promise((resolvePort, reject) => {
  const timer = setTimeout(() => reject(new Error("demo backend startup timed out")), 30000);
  lines.on("line", (line) => {
    // 起動の行は、初期データの作成が終わった後に出る。
    const match = /^hosted listening: 127\.0\.0\.1:(\d+) /.exec(line);
    if (match) { clearTimeout(timer); resolvePort(Number(match[1])); }
    else console.log(`[backend] ${line}`);
  });
  backend.once("exit", (code) => { clearTimeout(timer); reject(new Error(`demo backend exited with ${code}`)); });
});
// 自己終了（アイドルなど）の試験では、バックエンドの終了をそのまま親へ伝える。
backend.once("exit", (code) => { console.log(`[backend] exit ${code}`); shutdown(code === 0 ? 0 : 1); });
console.log(`demo test server ready: ${origin}`);
}
