use prometheus::{
    CounterVec, Gauge, GaugeVec, Histogram, HistogramOpts, Opts, Registry, TextEncoder,
};

/// All Prometheus metrics for the jellyfin-exporter.
///
/// Metric names follow Prometheus naming conventions. The `_total` suffix is
/// reserved for monotonic Counters; every Gauge is named for its semantic
/// (current value at scrape time) without the suffix.
pub struct Metrics {
    registry: Registry,

    // -- Jellyfin server / session metrics --
    pub up: Gauge,
    pub metrics_stale: Gauge,
    pub server_info: GaugeVec,
    pub sessions_active: GaugeVec,
    pub session_paused: GaugeVec,
    pub sessions_active_count: Gauge,
    pub transcodes_active: GaugeVec,
    pub transcode_hw_accelerated: Gauge,
    pub transcode_reason_sessions: GaugeVec,
    pub stream_bitrate: GaugeVec,
    pub stream_direct: GaugeVec,
    pub transcode_completion: GaugeVec,
    pub play_method_sessions: GaugeVec,
    pub session_play_position_seconds: GaugeVec,
    pub session_remote_address: GaugeVec,
    pub library_items: GaugeVec,
    pub items_by_type: GaugeVec,
    pub items_count: Gauge,

    // -- Exporter-self metrics (Prometheus exporter convention) --
    pub exporter_build_info: GaugeVec,
    pub exporter_scrape_duration_seconds: Histogram,
    pub exporter_scrape_errors_total: CounterVec,
    pub exporter_last_successful_scrape_timestamp_seconds: Gauge,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    /// Construct a fresh metrics registry with all gauges pre-registered.
    ///
    /// # Panics
    ///
    /// Panics if `prometheus::Gauge::new` or `Registry::register` fails. Both
    /// failure modes are programmer-error conditions: invalid metric names
    /// (compile-time constants here) or duplicate registration on a fresh
    /// registry. Neither is reachable at runtime.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    // reason: 16 metric definitions form a registration table — splitting
    // for line-count alone would obscure the symmetry between metric and
    // registration step.
    pub fn new() -> Self {
        let registry = Registry::new();

        let up = Gauge::new(
            "jellyfin_up",
            "Whether Jellyfin API is reachable (1 = up, 0 = down)",
        )
        .unwrap();
        let metrics_stale = Gauge::new(
            "jellyfin_metrics_stale",
            "Whether metrics are stale (1 = stale, Jellyfin unreachable)",
        )
        .unwrap();
        let server_info = GaugeVec::new(
            Opts::new(
                "jellyfin_server_info",
                "Jellyfin server metadata (always 1, labels carry the info)",
            ),
            &["version", "os", "server_name"],
        )
        .unwrap();
        let sessions_active = GaugeVec::new(
            Opts::new(
                "jellyfin_sessions_active",
                "Active playback sessions with details",
            ),
            &["user", "client", "play_method", "device"],
        )
        .unwrap();
        let session_paused = GaugeVec::new(
            Opts::new(
                "jellyfin_session_paused",
                "Whether a session is paused (1 = paused, 0 = playing)",
            ),
            &["user", "client", "device"],
        )
        .unwrap();
        let sessions_active_count = Gauge::new(
            "jellyfin_sessions_active_count",
            "Number of currently active playback sessions",
        )
        .unwrap();
        let transcodes_active = GaugeVec::new(
            Opts::new(
                "jellyfin_transcodes_active",
                "Active transcode sessions by codec",
            ),
            &["video_codec", "audio_codec"],
        )
        .unwrap();
        let transcode_hw_accelerated = Gauge::new(
            "jellyfin_transcode_hw_accelerated",
            "Number of hardware-accelerated transcode sessions",
        )
        .unwrap();
        let transcode_reason_sessions = GaugeVec::new(
            Opts::new(
                "jellyfin_transcode_reason_sessions",
                "Number of active transcode sessions per transcode reason",
            ),
            &["reason"],
        )
        .unwrap();
        let stream_bitrate = GaugeVec::new(
            Opts::new(
                "jellyfin_stream_bitrate_bps",
                "Current stream bitrate in bits per second",
            ),
            &["user", "media_type"],
        )
        .unwrap();
        let stream_direct = GaugeVec::new(
            Opts::new(
                "jellyfin_stream_direct",
                "Whether stream is direct (1) or transcoded (0) per type",
            ),
            &["user", "type"],
        )
        .unwrap();
        let transcode_completion = GaugeVec::new(
            Opts::new(
                "jellyfin_transcode_completion_pct",
                "Transcode buffer completion percentage",
            ),
            &["user"],
        )
        .unwrap();
        let play_method_sessions = GaugeVec::new(
            Opts::new(
                "jellyfin_play_method_sessions",
                "Number of active sessions per play method",
            ),
            &["method"],
        )
        .unwrap();
        let library_items = GaugeVec::new(
            Opts::new("jellyfin_library_items", "Items per Jellyfin library"),
            &["library_name", "library_type"],
        )
        .unwrap();
        let items_by_type = GaugeVec::new(
            Opts::new("jellyfin_items_by_type", "Items by media type"),
            &["type"],
        )
        .unwrap();
        let items_count = Gauge::new(
            "jellyfin_items_count",
            "Aggregate count of all items across all types",
        )
        .unwrap();

        let session_play_position_seconds = GaugeVec::new(
            Opts::new(
                "jellyfin_session_play_position_seconds",
                "Current playback position in seconds, per active session",
            ),
            &["user", "item_type"],
        )
        .unwrap();
        let session_remote_address = GaugeVec::new(
            Opts::new(
                "jellyfin_session_remote_address",
                "Active session connecting from a remote address (1 per session). \
                 Off by default; enable via EXPOSE_REMOTE_ADDRESS=true.",
            ),
            &["user", "remote_address"],
        )
        .unwrap();

        // -- Exporter-self metrics --

        let exporter_build_info = GaugeVec::new(
            Opts::new(
                "jellyfin_exporter_build_info",
                "Build metadata for the running exporter binary (always 1; labels carry the info).",
            ),
            &["version", "git_sha", "rustc_version", "build_date"],
        )
        .unwrap();
        // Initialise the build_info series once at construction so it is
        // present in every scrape from the very first `/metrics` hit.
        exporter_build_info
            .with_label_values(&[
                env!("CARGO_PKG_VERSION"),
                env!("BUILD_GIT_SHA"),
                rustc_version(),
                env!("BUILD_DATE"),
            ])
            .set(1.0);

        // Buckets target the realistic scrape-cycle range. Default
        // SCRAPE_INTERVAL_MS is 10 s; histograms top out at 30 s to surface
        // tail-latency outliers when Jellyfin is slow without truncating.
        let exporter_scrape_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "jellyfin_exporter_scrape_duration_seconds",
                "Wall-clock duration of one collection cycle in seconds.",
            )
            .buckets(vec![0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0]),
        )
        .unwrap();

        let exporter_scrape_errors_total = CounterVec::new(
            Opts::new(
                "jellyfin_exporter_scrape_errors_total",
                "Number of failed sub-fetches in collection cycles, by failure kind.",
            ),
            &["kind"],
        )
        .unwrap();

        let exporter_last_successful_scrape_timestamp_seconds = Gauge::new(
            "jellyfin_exporter_last_successful_scrape_timestamp_seconds",
            "Unix timestamp of the last collection cycle that completed without errors.",
        )
        .unwrap();

        // Register all metrics
        registry.register(Box::new(up.clone())).unwrap();
        registry.register(Box::new(metrics_stale.clone())).unwrap();
        registry.register(Box::new(server_info.clone())).unwrap();
        registry
            .register(Box::new(sessions_active.clone()))
            .unwrap();
        registry.register(Box::new(session_paused.clone())).unwrap();
        registry
            .register(Box::new(sessions_active_count.clone()))
            .unwrap();
        registry
            .register(Box::new(transcodes_active.clone()))
            .unwrap();
        registry
            .register(Box::new(transcode_hw_accelerated.clone()))
            .unwrap();
        registry
            .register(Box::new(transcode_reason_sessions.clone()))
            .unwrap();
        registry.register(Box::new(stream_bitrate.clone())).unwrap();
        registry.register(Box::new(stream_direct.clone())).unwrap();
        registry
            .register(Box::new(transcode_completion.clone()))
            .unwrap();
        registry
            .register(Box::new(play_method_sessions.clone()))
            .unwrap();
        registry.register(Box::new(library_items.clone())).unwrap();
        registry.register(Box::new(items_by_type.clone())).unwrap();
        registry.register(Box::new(items_count.clone())).unwrap();
        registry
            .register(Box::new(session_play_position_seconds.clone()))
            .unwrap();
        registry
            .register(Box::new(session_remote_address.clone()))
            .unwrap();
        registry
            .register(Box::new(exporter_build_info.clone()))
            .unwrap();
        registry
            .register(Box::new(exporter_scrape_duration_seconds.clone()))
            .unwrap();
        registry
            .register(Box::new(exporter_scrape_errors_total.clone()))
            .unwrap();
        registry
            .register(Box::new(
                exporter_last_successful_scrape_timestamp_seconds.clone(),
            ))
            .unwrap();

        Self {
            registry,
            up,
            metrics_stale,
            server_info,
            sessions_active,
            session_paused,
            sessions_active_count,
            transcodes_active,
            transcode_hw_accelerated,
            transcode_reason_sessions,
            stream_bitrate,
            stream_direct,
            transcode_completion,
            play_method_sessions,
            session_play_position_seconds,
            session_remote_address,
            library_items,
            items_by_type,
            items_count,
            exporter_build_info,
            exporter_scrape_duration_seconds,
            exporter_scrape_errors_total,
            exporter_last_successful_scrape_timestamp_seconds,
        }
    }

    /// Encode all metrics as Prometheus text exposition format.
    ///
    /// Returns the exposition text on success. On encoding failure (which is
    /// extremely rare — it would mean the prometheus crate's `fmt::Write`
    /// implementation returned an error), logs the error and returns an empty
    /// string rather than panicking the HTTP handler that called us.
    pub fn encode(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        match encoder.encode_to_string(&metric_families) {
            Ok(text) => text,
            Err(err) => {
                tracing::error!(error = ?err, "failed to encode metrics");
                String::new()
            }
        }
    }
}

/// Best-effort `rustc` version label for the build-info metric.
///
/// `Cargo.toml` pins `rust-version = "1.85"` so the toolchain that built
/// this binary is at least 1.85; we report exactly that, which is stable
/// across rebuilds and avoids pulling a build script for `rustc -V` parsing.
const fn rustc_version() -> &'static str {
    "1.85+"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_register_without_conflict() {
        let _metrics = Metrics::new();
    }

    #[test]
    fn encode_produces_valid_prometheus_text() {
        let metrics = Metrics::new();

        metrics.up.set(1.0);
        metrics.sessions_active_count.set(3.0);
        metrics
            .sessions_active
            .with_label_values(&["alice", "Infuse", "Transcode", "Apple TV"])
            .set(1.0);
        metrics
            .items_by_type
            .with_label_values(&["Movie"])
            .set(150.0);

        let output = metrics.encode();

        assert!(output.contains("jellyfin_up 1"));
        assert!(output.contains("jellyfin_sessions_active_count 3"));
        // prometheus crate sorts labels alphabetically
        assert!(output.contains(
            r#"jellyfin_sessions_active{client="Infuse",device="Apple TV",play_method="Transcode",user="alice"} 1"#
        ));
        assert!(output.contains(r#"jellyfin_items_by_type{type="Movie"} 150"#));
    }

    #[test]
    fn reset_clears_label_sets() {
        let metrics = Metrics::new();

        metrics
            .sessions_active
            .with_label_values(&["alice", "Web", "DirectPlay", "Chrome"])
            .set(1.0);
        metrics
            .sessions_active
            .with_label_values(&["bob", "Infuse", "Transcode", "Apple TV"])
            .set(1.0);

        let output_before = metrics.encode();
        assert!(output_before.contains("alice"));
        assert!(output_before.contains("bob"));

        metrics.sessions_active.reset();

        let output_after = metrics.encode();
        assert!(!output_after.contains("alice"));
        assert!(!output_after.contains("bob"));
    }

    #[test]
    fn all_metrics_present_in_output() {
        let metrics = Metrics::new();

        // Set at least one value for each metric so they appear in output
        metrics.up.set(1.0);
        metrics.metrics_stale.set(0.0);
        metrics
            .server_info
            .with_label_values(&["10.9.11", "Linux", "jellyfin"])
            .set(1.0);
        metrics
            .sessions_active
            .with_label_values(&["user", "client", "method", "device"])
            .set(1.0);
        metrics
            .session_paused
            .with_label_values(&["user", "client", "device"])
            .set(0.0);
        metrics.sessions_active_count.set(1.0);
        metrics
            .transcodes_active
            .with_label_values(&["h264", "aac"])
            .set(1.0);
        metrics.transcode_hw_accelerated.set(0.0);
        metrics
            .transcode_reason_sessions
            .with_label_values(&["VideoCodecNotSupported"])
            .set(1.0);
        metrics
            .stream_bitrate
            .with_label_values(&["user", "Video"])
            .set(1000.0);
        metrics
            .stream_direct
            .with_label_values(&["user", "video"])
            .set(1.0);
        metrics
            .transcode_completion
            .with_label_values(&["user"])
            .set(50.0);
        metrics
            .play_method_sessions
            .with_label_values(&["DirectPlay"])
            .set(1.0);
        metrics
            .library_items
            .with_label_values(&["Movies", "movies"])
            .set(150.0);
        metrics
            .items_by_type
            .with_label_values(&["Movie"])
            .set(150.0);
        metrics.items_count.set(1595.0);

        let output = metrics.encode();

        let expected_names = [
            "jellyfin_up",
            "jellyfin_metrics_stale",
            "jellyfin_server_info",
            "jellyfin_sessions_active",
            "jellyfin_session_paused",
            "jellyfin_sessions_active_count",
            "jellyfin_transcodes_active",
            "jellyfin_transcode_hw_accelerated",
            "jellyfin_transcode_reason_sessions",
            "jellyfin_stream_bitrate_bps",
            "jellyfin_stream_direct",
            "jellyfin_transcode_completion_pct",
            "jellyfin_play_method_sessions",
            "jellyfin_library_items",
            "jellyfin_items_by_type",
            "jellyfin_items_count",
        ];

        for name in expected_names {
            assert!(
                output.contains(name),
                "metric {name} not found in output:\n{output}"
            );
        }
    }

    #[test]
    fn build_info_metric_populated_at_startup() {
        let metrics = Metrics::new();
        let output = metrics.encode();
        // The build_info series is set in Metrics::new and must therefore
        // show up on the very first scrape, before any collect() runs.
        assert!(output.contains("jellyfin_exporter_build_info{"));
        assert!(output.contains(concat!("version=\"", env!("CARGO_PKG_VERSION"), "\"")));
    }

    #[test]
    fn no_gauge_uses_total_suffix() {
        // Convention guard: _total is reserved for monotonic Counters.
        // Every metric this exporter ships is a Gauge; none should leak the
        // _total suffix.
        let metrics = Metrics::new();
        let output = metrics.encode();

        for line in output.lines() {
            // # HELP / # TYPE / blank lines are fine; data lines have no '#'.
            if line.starts_with("# HELP ") || line.starts_with("# TYPE ") {
                let name = line.split_whitespace().nth(2).unwrap_or("");
                assert!(
                    !name.ends_with("_total"),
                    "metric {name} ends with _total but is a Gauge"
                );
            }
        }
    }
}
