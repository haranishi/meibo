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
cargo deny check licenses bans sources
cargo +1.88.0 check --workspace --all-targets --locked
```

`check-all` runs formatting, clippy, workspace tests, Markdown links, specification trace,
and benchmark documentation checks. CI installs cargo-deny explicitly and requires policy checks.
The separate advisory job needs network access to its advisory database. A skipped or failed
advisory check must be reported separately from passing offline checks.

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
