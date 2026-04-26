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
