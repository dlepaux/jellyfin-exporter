![CI](https://github.com/dlepaux/jellyfin-exporter/actions/workflows/ci.yml/badge.svg)
![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)
![Docker Image](https://ghcr-badge.egpl.dev/dlepaux/jellyfin-exporter/size)
![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)

# jellyfin-exporter

Prometheus exporter for [Jellyfin](https://jellyfin.org) media server. Tracks active sessions, transcoding details, and library statistics with rich labels for Grafana dashboards.

Built in Rust. Multi-arch Docker images (`linux/amd64`, `linux/arm64`) — runs on any server.

## Quick start

```bash
docker run -d \
  --name jellyfin-exporter \
  -e JELLYFIN_URL=http://jellyfin:8096 \
  -e JELLYFIN_API_KEY=your-key \
  -p 9711:9711 \
  ghcr.io/dlepaux/jellyfin-exporter:latest
```

Verify: `curl http://localhost:9711/metrics`

## Docker Compose

```yaml
jellyfin-exporter:
  image: ghcr.io/dlepaux/jellyfin-exporter:latest
  environment:
    JELLYFIN_URL: http://jellyfin:8096
    JELLYFIN_API_KEY: ${JELLYFIN_API_KEY}
    # METRICS_TOKEN: ${METRICS_TOKEN}  # optional auth
  ports:
    - 9711:9711
  restart: unless-stopped
```

## Prometheus configuration

Without auth:

```yaml
scrape_configs:
  - job_name: jellyfin
    static_configs:
      - targets: ['jellyfin-exporter:9711']
```

With Bearer token auth:

```yaml
scrape_configs:
  - job_name: jellyfin
    bearer_token: 'your-metrics-token'
    static_configs:
      - targets: ['jellyfin-exporter:9711']
```

## Metrics reference

### Session metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `jellyfin_sessions_active` | GaugeVec | `user`, `client`, `play_method`, `device` | Active playback sessions (1 per session) |
| `jellyfin_session_paused` | GaugeVec | `user`, `client`, `device` | Whether session is paused (1) or playing (0) |
| `jellyfin_sessions_active_count` | Gauge | — | Number of currently active playback sessions |
| `jellyfin_play_method_sessions` | GaugeVec | `method` | Sessions per play method (`DirectPlay`, `Transcode`, `DirectStream`) |
| `jellyfin_stream_bitrate_bps` | GaugeVec | `user`, `media_type` | Current stream bitrate in bits per second |
| `jellyfin_stream_direct` | GaugeVec | `user`, `type` | Whether stream is direct (1) or transcoded (0), per type (`video`, `audio`) |

### Transcode metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `jellyfin_transcodes_active` | GaugeVec | `video_codec`, `audio_codec` | Active transcode sessions by codec pair |
| `jellyfin_transcode_hw_accelerated` | Gauge | — | Number of hardware-accelerated transcode sessions |
| `jellyfin_transcode_reason_sessions` | GaugeVec | `reason` | Active transcode sessions per reason (e.g. `ContainerBitrateExceedsLimit`, `VideoCodecNotSupported`) |
| `jellyfin_transcode_completion_pct` | GaugeVec | `user` | Transcode buffer completion percentage |

### Library metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `jellyfin_library_items` | GaugeVec | `library_name`, `library_type` | Items per Jellyfin library |
| `jellyfin_items_by_type` | GaugeVec | `type` | Items by media type (`Movie`, `Series`, `Episode`, `Book`, `Song`, `Album`, `Artist`, `Trailer`, `MusicVideo`, `BoxSet`) |
| `jellyfin_items_count` | Gauge | — | Aggregate count of all items across all types |

### Server metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `jellyfin_up` | Gauge | — | API reachability (1 = up, 0 = down) |
| `jellyfin_metrics_stale` | Gauge | — | Whether metrics are stale due to API unreachability (1 = stale) |
| `jellyfin_server_info` | GaugeVec | `version`, `os`, `server_name` | Server metadata (always 1, labels carry the info) |

### Per-session detail metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `jellyfin_session_play_position_seconds` | GaugeVec | `user`, `item_type` | Current playback position in seconds (sourced from Jellyfin's `PositionTicks`, converted from 10M-tick-per-second units) |
| `jellyfin_session_remote_address` | GaugeVec | `user`, `remote_address` | Active session connecting from a remote address (1 per session). **Off by default** — enable with `EXPOSE_REMOTE_ADDRESS=true`. PII-adjacent. |

### Exporter-self metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `jellyfin_exporter_build_info` | GaugeVec | `version`, `git_sha`, `rustc_version`, `build_date` | Build metadata (always 1, labels carry the info). Populated at startup. |
| `jellyfin_exporter_scrape_duration_seconds` | Histogram | — | Wall-clock duration of one collection cycle |
| `jellyfin_exporter_scrape_errors_total` | Counter | `kind` | Number of failed sub-fetches, classified by error kind (`http`, `timeout`, `parse`, `circuit_open`, `unreachable`) |
| `jellyfin_exporter_last_successful_scrape_timestamp_seconds` | Gauge | — | Unix timestamp of the last collection cycle that completed without errors |

Only sessions with active playback (`NowPlayingItem` present) are counted — idle browser sessions are excluded.

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `JELLYFIN_URL` | Yes | — | Jellyfin base URL (e.g. `http://jellyfin:8096`) |
| `JELLYFIN_API_KEY` | Yes | — | API key from Jellyfin admin dashboard |
| `PORT` | No | `9711` | HTTP listen port (1–65535) |
| `SCRAPE_INTERVAL_MS` | No | `10000` | Collection interval in ms (min: 1000) |
| `LOG_LEVEL` | No | `info` | Log verbosity: `trace`, `debug`, `info`, `warn`, `error` |
| `METRICS_TOKEN` | No | — | Bearer token for `/metrics` auth. When set, requests need `Authorization: Bearer <token>`. When unset, no auth required. |
| `REQUEST_TIMEOUT_MS` | No | `5000` | Timeout per Jellyfin API call in ms (min: 100) |
| `RETRY_MAX_ATTEMPTS` | No | `3` | Max retry attempts on transient API failures |
| `RETRY_BASE_DELAY_MS` | No | `500` | Base delay for exponential backoff in ms (min: 50). Max delay = base × 10. |
| `CIRCUIT_BREAKER_THRESHOLD` | No | `5` | Consecutive failures before circuit opens (min: 1) |
| `CIRCUIT_BREAKER_RESET_MS` | No | `60000` | Time before half-open retry after circuit opens in ms (min: 1000) |
| `EXPOSE_REMOTE_ADDRESS` | No | `false` | Set `true` to expose `jellyfin_session_remote_address{user, remote_address}` for each active session. PII-adjacent — leave off unless you understand the privacy implications. Accepts `true`/`false`, `1`/`0`, `yes`/`no`, `on`/`off`. |

### Generating a Jellyfin API key

1. Open Jellyfin admin dashboard
2. Go to **Dashboard → API Keys**
3. Click **+** to create a new key
4. Name it `jellyfin-exporter` (or similar)
5. Copy the key into `JELLYFIN_API_KEY`

## Endpoints

| Path | Auth | Description |
|------|------|-------------|
| `/metrics` | Optional Bearer | Prometheus metrics (text exposition format). 401 returns `WWW-Authenticate: Bearer realm="jellyfin-exporter"`. |
| `/health` | None — even when `METRICS_TOKEN` is set | Health check — always returns `ok`. Intentionally unauthenticated so orchestrator probes (Docker, k8s, load balancers) work without credentials. |
| `/ready` | None — even when `METRICS_TOKEN` is set | Readiness check — `ready` (200) when the circuit breaker is closed or half-open, `not ready` (503) when open. Same rationale as `/health`. |

## Architecture

The exporter runs a background collection loop on the configured `SCRAPE_INTERVAL_MS`. The `/metrics` endpoint serves the last-collected snapshot — it does not trigger a live API call.

**Resilience pipeline**: each Jellyfin API call passes through retry (exponential backoff with jitter) → circuit breaker. If the circuit opens after consecutive failures, the exporter serves stale metrics with `jellyfin_up=0` and `jellyfin_metrics_stale=1` until the server recovers.

**Bitrate resolution**: stream bitrate is resolved via a fallback chain — `TranscodingInfo.Bitrate` → `NowPlayingItem.Bitrate` → sum of `MediaStreams[].BitRate`.

## Examples

The [`examples/`](examples/) directory ships ready-to-use snippets:

- [`examples/grafana/dashboard.json`](examples/grafana/dashboard.json) — 6-panel starter dashboard for Grafana 11.x. Covers active sessions, transcode reasons, play-method-over-time, library by type, total bitrate, and exporter health. Import via Grafana → Dashboards → Import.
- [`examples/prometheus/scrape.yml`](examples/prometheus/scrape.yml) — copy-paste Prometheus scrape config, with both anonymous and Bearer-token variants.
- [`docker-compose.example.yml`](docker-compose.example.yml) — complete Jellyfin + jellyfin-exporter + Prometheus stack. `docker compose -f docker-compose.example.yml up -d`, generate a Jellyfin API key, drop it in `.env`, restart the exporter, and Prometheus is scraping at `http://localhost:9090`.

## Development

```bash
cp .env.example .env  # fill in JELLYFIN_URL and JELLYFIN_API_KEY

cargo build            # compile
cargo test             # run tests
cargo clippy           # lint
cargo fmt --check      # format check
```

## Reporting a security issue

Vulnerabilities go through [security.md](security.md), not the public
issue tracker. GitHub Security Advisories preferred; email also works.

## License

[MIT](license.md)
