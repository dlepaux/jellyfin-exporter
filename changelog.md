# [2.0.0](https://github.com/dlepaux/jellyfin-exporter/compare/v1.0.2...v2.0.0) (2026-08-25)


### Documentation

* state the MSRV in the readme, not just the badge ([d34bb29](https://github.com/dlepaux/jellyfin-exporter/commit/d34bb2988f44a42a4d44d83d85d1578136dda3a7))


### BREAKING CHANGES

* minimum supported Rust version raised from 1.85 to 1.88.

Required to take the patched time crate (RUSTSEC-2026-0009, stack-exhaustion
DoS), whose fix needs 1.88. Only affects building from source; the published
Docker images carry their own toolchain.

This footer also exists because the previous commit used 'fix(deps)!:' and cut
NO release — the configured commit-analyzer uses the ANGULAR preset, which does
not understand the '!' breaking-change marker and failed to parse the type at
all ('Analysis of 1 commits complete: no release'). Angular wants a
* footer, which is what this is. Without a release there is no
docker job, so the security fix would never have reached the image.

## [1.0.2](https://github.com/dlepaux/jellyfin-exporter/compare/v1.0.1...v1.0.2) (2026-08-25)


### Bug Fixes

* **deny:** restore the rand advisory ignore — CI detects what my laptop did not ([a608473](https://github.com/dlepaux/jellyfin-exporter/commit/a6084732d1674064c14145e9051bf50ab3bb0a92))
* **deps:** patch two advisories that were failing CI ([f464532](https://github.com/dlepaux/jellyfin-exporter/commit/f46453271e977b9bc50e9a8e6bea002127771f28))
* **metrics:** derive items_count from the per-type counts, not Jellyfin's ItemCount ([a666e78](https://github.com/dlepaux/jellyfin-exporter/commit/a666e7888ab01cc30dd562c5099fac7bd416c16d))

## [1.0.1](https://github.com/dlepaux/jellyfin-exporter/compare/v1.0.0...v1.0.1) (2026-06-07)


### Bug Fixes

* **security:** apt upgrade base packages at build time ([0ccf917](https://github.com/dlepaux/jellyfin-exporter/commit/0ccf91742e8bf32876a81b7ecd57711992828c67))

# 1.0.0 (2026-04-26)


### Features

* initial public release of jellyfin-exporter ([575aad5](https://github.com/dlepaux/jellyfin-exporter/commit/575aad53f7666f9b2a3cb38398aec09a3ff6b4ce))

# 1.0.0 (TBD)

Initial public release.

`jellyfin-exporter` is a Prometheus exporter for [Jellyfin] media server,
written in Rust. It scrapes the Jellyfin API on a configurable interval and
exposes everything you'd want to graph about an active Jellyfin instance:
sessions, transcoding details with reasons, library statistics, and the
exporter's own pipeline health.

## Highlights

- **Multi-arch Docker images** — `linux/amd64` and `linux/arm64` published
  natively (not via QEMU), so it runs on any server architecture you'd
  point Jellyfin at.
- **Resilient by design** — every Jellyfin API call goes through a retry
  layer wrapped in a circuit breaker. When Jellyfin is unreachable the
  exporter serves the last successful snapshot with `jellyfin_up=0` and
  `jellyfin_metrics_stale=1`, so dashboards never just go blank.
- **Optional `/metrics` Bearer auth** via `METRICS_TOKEN` (constant-time
  compare, RFC 7235 challenge on 401).
- **Sixteen domain metrics + four exporter-self metrics** following
  Prometheus naming conventions (`_total` reserved for monotonic Counters).
- **Examples included** — Grafana starter dashboard, Prometheus scrape
  snippet, and a complete `docker-compose.example.yml` for zero-to-green
  in 60 seconds.

See [readme.md] for the full metrics reference, configuration table, and
endpoint list.

[Jellyfin]: https://jellyfin.org
[readme.md]: ./readme.md
