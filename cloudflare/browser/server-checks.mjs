import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readdir, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const binary = process.env.MEIBO_TEST_BINARY || resolve('..', 'target', 'debug', 'meibo');

async function fixture(t, overrides = {}, args = []) {
  const directory = await mkdtemp(join(tmpdir(), 'meibo-browser-check-'));
  const reservation = createServer();
  reservation.listen(0, '127.0.0.1');
  await once(reservation, 'listening');
  const port = reservation.address().port;
  await new Promise((done) => reservation.close(done));
  const child = spawn(process.execPath, ['browser/server.mjs', ...args], {
    env: { ...process.env, MEIBO_TEST_BINARY: binary, MEIBO_TEST_PORT: String(port), TMPDIR: directory, ...overrides },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let log = '';
  child.stdout.on('data', (data) => { log += data; });
  child.stderr.on('data', (data) => { log += data; });
  const exited = once(child, 'exit');
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
    await exited;
    await rm(directory, { recursive: true, force: true });
  });
  return { directory, child, port, exited, log: () => log };
}

test('generation failure returns failure and removes its temporary fixture', { timeout: 10_000 }, async (t) => {
  const run = await fixture(t, { MEIBO_TEST_BINARY: '/absent/meibo-browser-no-binary' });
  const [code] = await run.exited;
  assert.notEqual(code, 0);
  assert.deepEqual(await readdir(run.directory), []);
});

test('an existing listener is refused and remains alive', { timeout: 10_000 }, async (t) => {
  const listener = createServer((socket) => socket.end());
  listener.listen(0, '127.0.0.1');
  await once(listener, 'listening');
  t.after(() => new Promise((done) => listener.close(done)));
  const port = listener.address().port;
  const run = await fixture(t, { MEIBO_TEST_PORT: String(port) });
  assert.notEqual((await run.exited)[0], 0);
  assert.equal(listener.listening, true);
  assert.deepEqual(await readdir(run.directory), []);
});

test('database arguments are refused before creating a fixture', { timeout: 10_000 }, async (t) => {
  const run = await fixture(t, {}, ['existing-real.db']);
  assert.notEqual((await run.exited)[0], 0);
  assert.deepEqual(await readdir(run.directory), []);
});

test('real CLI creates only a synthetic fixture and interruption removes it', { timeout: 20_000 }, async (t) => {
  const run = await fixture(t);
  const url = `http://127.0.0.1:${run.port}`;
  let summary;
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      const response = await fetch(`${url}/api/summary`);
      if (response.ok) { summary = await response.json(); break; }
    } catch { /* The owned child is still starting. */ }
    if (run.child.exitCode !== null) assert.fail(`Fixture exited early: ${run.log()}`);
    await new Promise((done) => setTimeout(done, 50));
  }
  assert.equal(summary?.origin, 'synthetic', run.log());
  assert.equal(summary.total, 100);
  assert.equal((await readdir(run.directory)).length, 1);
  run.child.kill('SIGINT');
  assert.equal((await run.exited)[0], 0, run.log());
  assert.deepEqual(await readdir(run.directory), []);
  await assert.rejects(fetch(`${url}/health`));
});
