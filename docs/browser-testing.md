# Test the browser before merging

Use this disposable environment to review a PR without touching a working CRM database.
Each server start creates a new synthetic 100-company database and binds only to loopback.
The existing Rust, MSRV, cloud integration, and Worker CI jobs remain separate gates.

## Prerequisites and checks

Use the Rust version in [Contributing](../CONTRIBUTING.md), Node 24.22.1, and npm 12.2.1.
Build from the repository root. `CARGO_TARGET_DIR`, `TMPDIR`, npm's cache, and
`PLAYWRIGHT_BROWSERS_PATH` may point to mounted external storage. Check its identity and
free space before installing dependencies or building. Do not create a directory in place
of an unmounted drive. Set `RUST_TEST_THREADS=1` on hosts with limited temporary storage.

```sh
cargo build --locked -p meibo-app
cd cloudflare
npm ci --ignore-scripts
npx --no-install playwright install chromium --only-shell
export MEIBO_TEST_BINARY="$(cd .. && pwd)/target/debug/meibo"
npm run test:browser:server
npm run test:browser
```

When using `CARGO_TARGET_DIR`, set `MEIBO_TEST_BINARY` to that build's absolute `debug/meibo`
path instead. Linux hosts may need `playwright install --with-deps chromium --only-shell`.
Playwright 1.64.1 is pinned with its matching browser revision. Package installation and
browser installation are separate; `npm ci --ignore-scripts` never downloads a browser.

The tests exercise search, empty-result recovery, keyboard access to a company, activity
saving, persistence after reload, and CSV download in three viewport widths. They reject
JavaScript/console errors, external requests, and document overflow. Failure traces and
screenshots, a JSON report, and synthetic record screenshots use `MEIBO_TEST_OUTPUT` or a temporary
`meibo-browser-results` directory. Generated databases, results, and downloaded CSVs stay
out of Git. The browser tests never accept a staging or production URL.

## Review the synthetic UI manually

```sh
cd cloudflare
MEIBO_TEST_BINARY=/absolute/path/to/meibo npm run test:browser:serve
```

Open `http://127.0.0.1:4331`. Do not enter real customer data: Ctrl+C stops the owned server
and deletes its temporary database, including edits. A restart creates another fresh demo.
Database arguments are refused. The runner does not use `--allow-real`, an existing database,
or an existing listener. If 4331 is busy, stop your own test instance or choose another
`MEIBO_TEST_PORT` from 1024 to 65535; use the same variable for the browser test command.
Run automated tests after stopping the manual instance, since server reuse is disabled.

## CI scope

Every push/PR runs the `browser` job using the lockfile, a built CLI, Chromium headless shell,
and fresh synthetic data. Fixture tests also verify failure cleanup, port conflicts, and
interruption cleanup. The job has read-only repository permissions and a finite timeout.
Synthetic screenshots and failure traces are retained as CI artifacts for seven days.
This checks the local app, not Turso Cloud authentication, a paid plan, or a live deployment.

The setup follows the official [web server](https://playwright.dev/docs/test-webserver) and
[CI](https://playwright.dev/docs/ci) guidance. Always require successful checks for the actual
PR head before merging; a prior run on a different commit is not evidence for that head.
