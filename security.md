# Security policy

## Reporting a vulnerability

If you believe you've found a security issue in `jellyfin-exporter`,
please report it privately. **Do not** open a public GitHub issue.

Two channels, in order of preference:

1. **GitHub Security Advisories** — open a draft advisory on this repo:
   <https://github.com/dlepaux/jellyfin-exporter/security/advisories/new>.
   This keeps the report private until a fix is coordinated and ships
   you a CVE if one is warranted.

2. **Email** — `d.lepaux@gmail.com`. Use a subject prefix like
   `[security][jellyfin-exporter]` so it doesn't drown in the inbox.
   PGP available on request if needed.

## What to expect

- **Acknowledgment within 72 hours.** If you don't hear back, please
  re-send — a missed mail is more likely than a deliberate ignore.
- **Coordinated disclosure.** I'll work with you on a timeline that
  balances getting users patched against giving you fair credit.
- **No public disclosure until a fix is available.** Once a fix ships,
  the advisory is published, the relevant release tagged, and (if you
  consent) you're credited.

## In scope

- The `jellyfin-exporter` binary and Docker image as published on GHCR.
- The HTTP surface: `/metrics`, `/health`, `/ready`, including the
  `METRICS_TOKEN` Bearer auth path on `/metrics`.
- The Jellyfin API client and the configuration layer (env-var parsing,
  secret handling, default behavior).
- Build artefacts (multi-arch manifest, OCI labels, SBOM, provenance
  attestations).

## Out of scope

- Vulnerabilities in upstream dependencies (axum, reqwest, prometheus,
  tokio, etc.) — please report those to the respective projects. We
  consume them via Cargo.lock and refresh on each `fix:` / `feat:`
  release.
- Vulnerabilities in Jellyfin itself — report to the
  [Jellyfin project](https://github.com/jellyfin/jellyfin/security).
- Issues that require already having admin access to the host running
  the exporter (filesystem reads, environment manipulation, etc.) —
  that's the threat model boundary.
- Denial-of-service via crafted Jellyfin API responses on a host the
  reporter controls — the operator running the exporter is assumed to
  trust their own Jellyfin server; a malicious *Jellyfin server* is
  outside the threat model.

## Supported versions

Security fixes are issued for the latest minor on `main`. There are no
LTS branches. If a vulnerability is discovered in v1.4.x, the fix lands
on `main` and ships as v1.4.x+1; users on older versions are expected to
upgrade.

## Credit

If you'd like to be named in the advisory and changelog, say so in your
report. Default is anonymous unless you ask otherwise.
