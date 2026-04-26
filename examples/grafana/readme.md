# Grafana — starter dashboard

`dashboard.json` is a 6-panel starter dashboard you can drop straight into a
Grafana 11.x instance that already scrapes the exporter.

**Panels**

1. Active sessions table — `jellyfin_sessions_active` (one row per active user/client/device/play_method combination)
2. Transcode reasons pie — `sum by (reason) (jellyfin_transcode_reason_sessions)`
3. Play method over time — stacked timeseries of `jellyfin_play_method_sessions`
4. Library by type — bar chart of `jellyfin_items_by_type`
5. Total bitrate served — stat panel summing `jellyfin_stream_bitrate_bps`
6. Exporter health — `jellyfin_up` and the scrape age computed from `jellyfin_exporter_last_successful_scrape_timestamp_seconds`

**Importing**

1. Grafana → Dashboards → New → Import.
2. Paste the contents of `dashboard.json`, or upload the file.
3. When prompted for the datasource, pick the Prometheus datasource that scrapes the exporter.

**Customising**

The dashboard is intentionally small — it covers the headline metrics
without becoming a maintenance liability for the exporter repo. For
heavier dashboards (per-user watch graphs, codec heatmaps, library growth
over time), branch from this one and add what you need; nothing in the
exporter codebase depends on this JSON.
