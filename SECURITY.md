# Security Policy

## Local-Only & Zero Telemetry Guarantee

`bbstats` runs **100% locally** on your machine:
- No analytics, crash uploads, update pings, or outbound network connections.
- Local Docker container discovery communicates exclusively over the local Docker named pipe (`\\.\pipe\docker_engine`) or Unix socket (`/var/run/docker.sock`) when Docker is running.
- Sensitive command-line arguments (passwords, API keys, bearer tokens, basic-auth URLs) are redacted before display, snapshot dumps, or state comparisons.

## Supported Versions

| Version | Supported |
| ------- | --------- |
| 0.2.x   | Yes       |
| < 0.2.0 | No        |

## Reporting a Vulnerability

If you discover a security vulnerability (such as secret leakage, privilege/process-termination bypass, or command-injection handling), please **do not** open a public issue.

1. Use GitHub Security Advisories (**Security -> Advisories -> Report a vulnerability**) on `F0Rextasy/bbstats` if enabled, or contact the maintainer directly via GitHub.
2. Include the `bbstats --version` output, OS version, reproduction steps, and potential impact.
3. We aim to acknowledge reports within 72 hours and coordinate a fix before public disclosure.

## Windows SmartScreen / Antivirus Notes

Until signed release binaries are distributed via an organizational certificate, newly published binaries from GitHub Releases may trigger an initial SmartScreen prompt on Windows because `bbstats` enumerates running processes and handles. Every release archive includes `SHA256SUMS.txt` and GitHub Actions build-provenance attestations so you can verify binary integrity before launching.
