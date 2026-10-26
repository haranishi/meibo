# Contributing

meibo is an early development release. The project name is provisional.
Use synthetic data for development, tests, screenshots, and performance measurements.

## Develop and verify

Use Rust 1.99.0 with rustfmt and clippy. The minimum supported Rust version is 1.88.0.
Cargo downloads dependencies on the first run; use `--offline` after fetching them if needed.

```sh
cargo fetch --locked
cargo install cargo-deny --version 0.20.2 --locked
cargo xtask check-all
cargo deny --manifest-path crates/app/Cargo.toml --config deny.toml --locked check all
cargo +1.88.0 check --workspace --all-targets --locked
```

`check-all` runs formatting, clippy, default workspace tests, Markdown links, specification trace,
benchmark documentation checks, and all four dependency policy checks. cargo-deny is required.
The default policy uses the app manifest as its root and keeps default features and every network ban.
Policy checks refresh the advisory database online; passing offline Rust checks does not replace them.

## Verify the cloud profile

Use a compatible sqld 0.24.32 binary for isolated synthetic fixtures. On Linux x86_64, the setup
helper downloads the pinned official archive and verifies its SHA-256 before installing it:

```sh
sh scripts/install-ci-sqld.sh /tmp/meibo-ci-tools
export MEIBO_SQLD_BINARY=/tmp/meibo-ci-tools/sqld
cargo xtask check-cloud
cargo +1.88.0 check --workspace --all-targets --locked \
  --features meibo-db/integration-tests,meibo-core/integration-tests,meibo-app/integration-tests
```

On other hosts, set `MEIBO_SQLD_BINARY` to an existing compatible native binary.
`check-cloud` requires sqld and cargo-deny, builds the production cloud profile, then lints and
runs every workspace target with all three explicit `integration-tests` features. It audits the
complete cloud and test dependency graph with `deny-cloud.toml`, including online advisories.
Use `RUST_TEST_THREADS=2` to limit contention without filtering tests. The fixtures use loopback
and synthetic data; these checks do not verify a Cloudflare deployment or Turso Cloud account.

## Propose a change

Read the relevant requirements, design, and tasks in `.kiro/specs/` before editing.
Update the specification when behavior changes, then record validation in its tasks.
Keep PRs focused and include the problem, resulting behavior, and validation results.
Use English for PR titles and descriptions. Preserve existing databases and provide migrations
when storage changes. Do not include personal data, credentials, local configuration, caches,
or `.agent-harness/` execution history. Benchmark records in `docs/benchmarks/` are shared.

## License and sign-off

Code is contributed under MIT OR Apache-2.0; documentation is under CC BY 4.0.
Each commit must include a DCO sign-off. Read the [Developer Certificate of Origin](https://developercertificate.org/)
and use `git commit --signoff` with your own identity.

Report security issues using [SECURITY.md](SECURITY.md).
