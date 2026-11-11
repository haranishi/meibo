import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// No database argument is accepted: only this process's fresh synthetic fixture is served.
const root = fileURLToPath(new URL('../../', import.meta.url));
const target = resolve(root, process.env.CARGO_TARGET_DIR || 'target');
const binary = resolve(process.env.MEIBO_TEST_BINARY || join(target, 'debug', 'meibo'));
const port = process.env.MEIBO_TEST_PORT || '4331';
let directory;
let child;
let stopping = false;
let killTimer;

function stop() {
  stopping = true;
  if (child && child.exitCode === null && child.signalCode === null) {
    child.kill('SIGTERM');
    killTimer = setTimeout(() => child?.kill('SIGKILL'), 2000);
    killTimer.unref();
  }
}
process.once('SIGINT', stop);
process.once('SIGTERM', stop);

async function checkPort() {
  const server = createServer();
  await new Promise((done, fail) => {
    server.once('error', fail);
    server.listen(Number(port), '127.0.0.1', () => server.close(done));
  });
}

async function run(args, timeout) {
  if (stopping) return;
  await new Promise((done, fail) => {
    child = spawn(binary, args, { stdio: ['ignore', 'inherit', 'inherit'] });
    let timedOut = false;
    const deadline = timeout ? setTimeout(() => { timedOut = true; stop(); }, timeout) : undefined;
    child.once('error', fail);
    child.once('close', (code) => {
      clearTimeout(deadline);
      clearTimeout(killTimer);
      child = undefined;
      if (!timedOut && (code === 0 || stopping)) done();
      else fail(new Error('CLI failed'));
    });
  });
}

try {
  if (process.argv.length !== 2 || !/^\d+$/.test(port) || Number(port) < 1024 || Number(port) > 65535) {
    throw new Error('Invalid fixture configuration');
  }
  await checkPort();
  if (!stopping) {
    directory = await mkdtemp(join(tmpdir(), 'meibo-browser-'));
    const database = join(directory, 'synthetic.db');
    await run(['demo', database, '--rows', '100'], 15_000);
    if (!stopping) {
      console.log(`Synthetic test environment: http://127.0.0.1:${port}`);
      console.log('This temporary synthetic database is deleted when the test environment stops.');
      await run(['serve', database, '--port', port]);
      if (!stopping) throw new Error('Server stopped unexpectedly');
    }
  }
} catch {
  console.error('Synthetic test environment could not start or stopped unexpectedly. Check the binary and loopback port.');
  process.exitCode = 1;
} finally {
  clearTimeout(killTimer);
  if (directory) await rm(directory, { recursive: true, force: true });
}
