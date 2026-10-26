# Security policy

meibo is an early development release.
Security fixes target the current development branch.

## Report a vulnerability

Use GitHub's private [Report a vulnerability](https://github.com/haranishi/meibo/security/advisories/new) action.
If unavailable, open an issue containing only a request for a private reporting channel.
Do not post an exploit, credentials, personal data, or a customer database in a public issue.

Include the affected revision, reproduction steps using synthetic data, the expected behavior,
and the observed impact. Remove tokens, absolute local paths, and personal data from logs.
The maintainer will coordinate reproduction, remediation, and disclosure with the reporter.
There is no guaranteed response time during early development.

## Current boundary

Local `serve` binds to `127.0.0.1`; real databases require `--allow-real`.
Do not expose the unauthenticated local mode through a public interface or reverse proxy.
Use `hosted` for team access, with HTTPS, sessions, per-organization databases and roles.
See [hosting](docs/hosting.md) for persistent storage and recovery.
Remote MCP and public self-registration are not supported. Local MCP clients inherit access
to the selected database; only enable writes for a trusted client.
Database files, exports, backups, and previous copies need protection by the local user.
See [DISCLAIMER.md](DISCLAIMER.md) for durability and erasure limitations.

CI runs dependency policy checks and a separate online advisory check.
An unavailable advisory database is an incomplete check, not a clean security result.
