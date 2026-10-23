# Security policy

meibo is in private development and has no supported public release yet.
Security fixes target the current development branch.

## Report a vulnerability

Contact the repository maintainer through the private channel used to grant you access.
If the repository provides GitHub's **Report a vulnerability** action, you may use that instead.
Do not post an exploit, credentials, personal data, or a customer database in a public issue.

Include the affected revision, reproduction steps using synthetic data, the expected behavior,
and the observed impact. Remove tokens, absolute local paths, and personal data from logs.
The maintainer will coordinate reproduction, remediation, and disclosure with the reporter.
There is no guaranteed response time during private development.

## Current boundary

The web server binds to `127.0.0.1` and accepts only synthetic demo databases.
It has no team authentication or authorization and is intended for a single local user.
Do not expose it through a public interface or reverse proxy.
Database files, exports, backups, and previous copies need protection by the local user.
See [DISCLAIMER.md](DISCLAIMER.md) for durability and erasure limitations.

CI runs dependency policy checks and a separate online advisory check.
An unavailable advisory database is an incomplete check, not a clean security result.
