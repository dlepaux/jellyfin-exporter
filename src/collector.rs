use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerError, CircuitStatus};
use crate::client::{ClientError, JellyfinApi};
use crate::metrics::Metrics;
use crate::retry::with_retry;

/// Maximum length of a Prometheus label value (in characters). Anything longer
/// is truncated by `sanitize_label` to keep cardinality bounded and exposition
/// payloads reasonable.
const MAX_LABEL_CHARS: usize = 64;

pub struct CollectorConfig {
    pub scrape_interval: Duration,
    pub failure_threshold: u32,
    pub reset_timeout: Duration,
    pub retry_max_attempts: u32,
    pub retry_base_delay: Duration,
    pub retry_max_delay: Duration,
    /// When `true`, populate `jellyfin_session_remote_address` with the
    /// client IP for each active session. Default `false` (privacy).
    pub expose_remote_address: bool,
}

pub struct Collector {
    client: Arc<dyn JellyfinApi>,
    metrics: Arc<Metrics>,
    circuit_breaker: CircuitBreaker,
    scrape_interval: Duration,
    retry_max_attempts: u32,
    retry_base_delay: Duration,
    retry_max_delay: Duration,
    expose_remote_address: bool,
}

impl Collector {
    /// Construct a collector wired to a Jellyfin client and a metrics
    /// registry, with the resilience parameters in `config`.
    #[must_use]
    pub fn new(
        client: Arc<dyn JellyfinApi>,
        metrics: Arc<Metrics>,
        config: &CollectorConfig,
    ) -> Self {
        Self {
            client,
            metrics,
            circuit_breaker: CircuitBreaker::new(config.failure_threshold, config.reset_timeout),
            scrape_interval: config.scrape_interval,
            retry_max_attempts: config.retry_max_attempts,
            retry_base_delay: config.retry_base_delay,
            retry_max_delay: config.retry_max_delay,
            expose_remote_address: config.expose_remote_address,
        }
    }

    /// Run the collection loop until cancellation.
    pub async fn run(&self, cancel: CancellationToken) {
        // Collect immediately on start
        self.collect().await;

        let mut interval = tokio::time::interval(self.scrape_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        interval.tick().await; // consume the first immediate tick

        loop {
            tokio::select! {
                _ = interval.tick() => self.collect().await,
                () = cancel.cancelled() => {
                    tracing::info!("collector shutting down");
                    return;
                }
            }
        }
    }

    /// Whether the exporter is ready to serve (circuit breaker not open).
    pub fn is_ready(&self) -> bool {
        self.circuit_breaker.status() != CircuitStatus::Open
    }

    async fn collect(&self) {
        // Histogram timer auto-observes on drop, capturing wall-clock duration
        // of the entire collection cycle.
        let _timer = self.metrics.exporter_scrape_duration_seconds.start_timer();

        // Phase 1: unauthenticated reachability probe — establishes whether
        // the network path to Jellyfin is up at all. This does NOT validate
        // the API key; an authenticated endpoint failing afterwards will
        // surface that case via the circuit breaker.
        let reachable = self.client.is_publicly_reachable().await;

        if !reachable {
            self.metrics.up.set(0.0);
            self.metrics.metrics_stale.set(1.0);
            self.metrics
                .exporter_scrape_errors_total
                .with_label_values(&["unreachable"])
                .inc();
            tracing::debug!("jellyfin unreachable, serving stale metrics");
            return;
        }

        self.metrics.up.set(1.0);
        self.metrics.metrics_stale.set(0.0);

        // Phase 2: fan out to all collection tasks concurrently
        let (sessions_result, libraries_result, counts_result, system_result) = tokio::join!(
            self.fetch_with_resilience(|| self.client.get_sessions()),
            self.fetch_libraries_with_counts(),
            self.fetch_with_resilience(|| self.client.get_item_counts()),
            self.fetch_with_resilience(|| self.client.get_system_info()),
        );

        let any_err = sessions_result.is_err()
            || libraries_result.is_err()
            || counts_result.is_err()
            || system_result.is_err();

        self.record_scrape_error(&sessions_result);
        self.record_scrape_error(&libraries_result);
        self.record_scrape_error(&counts_result);
        self.record_scrape_error(&system_result);

        self.collect_sessions(sessions_result);
        self.collect_libraries(libraries_result);
        self.collect_item_counts(counts_result);
        self.collect_system_info(system_result);

        if !any_err {
            let unix_now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            self.metrics
                .exporter_last_successful_scrape_timestamp_seconds
                .set(unix_now);
        }
    }

    /// Increment `jellyfin_exporter_scrape_errors_total{kind=...}` if the
    /// sub-fetch failed, classifying by error variant. Counters are the
    /// right primitive here (monotonically increasing across the lifetime
    /// of the process), so the `_total` suffix is correct convention.
    fn record_scrape_error<T>(&self, result: &Result<T, CollectorError>) {
        let Err(err) = result else {
            return;
        };
        let kind = match err {
            CollectorError::CircuitOpen => "circuit_open",
            CollectorError::Api(ClientError::Http(_)) => "http",
            CollectorError::Api(ClientError::Timeout) => "timeout",
            CollectorError::Api(ClientError::Deserialization(_)) => "parse",
        };
        self.metrics
            .exporter_scrape_errors_total
            .with_label_values(&[kind])
            .inc();
    }

    /// Wrap an API call with circuit breaker → retry.
    ///
    /// Layering rationale: the breaker is the *outer* layer so that when it
    /// is open, no retry attempts are made — every scrape against an open
    /// breaker is a single fast-fail, not `retry_max_attempts × delay`
    /// burned in dead sleep. The breaker counts whole retry-exhausted
    /// failures as one signal, so a transient blip that retry recovers from
    /// never trips the breaker.
    async fn fetch_with_resilience<F, Fut, T>(&self, f: F) -> Result<T, CollectorError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, ClientError>>,
    {
        let cb = &self.circuit_breaker;
        let max_attempts = self.retry_max_attempts;
        let base_delay = self.retry_base_delay;
        let max_delay = self.retry_max_delay;

        let result = cb
            .execute(|| with_retry(f, max_attempts, base_delay, max_delay))
            .await;

        result.map_err(|e| match e {
            CircuitBreakerError::Open { retry_after } => {
                tracing::debug!(retry_after_ms = retry_after.as_millis(), "circuit open");
                CollectorError::CircuitOpen
            }
            CircuitBreakerError::Inner(client_err) => CollectorError::Api(client_err),
        })
    }

    /// Fetch libraries and then fan out per-library item count requests.
    async fn fetch_libraries_with_counts(&self) -> Result<Vec<LibraryWithCount>, CollectorError> {
        let libraries = self
            .fetch_with_resilience(|| self.client.get_libraries())
            .await?;

        let mut results = Vec::with_capacity(libraries.len());

        // Fan out item count requests concurrently
        let count_futures: Vec<_> = libraries
            .iter()
            .map(|lib| {
                let item_id = lib.item_id.clone();
                async move {
                    self.fetch_with_resilience(|| self.client.get_library_item_count(&item_id))
                        .await
                }
            })
            .collect();

        let counts = futures_util::future::join_all(count_futures).await;

        for (lib, count_result) in libraries.into_iter().zip(counts) {
            let count = match count_result {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        library = lib.name,
                        error = ?e,
                        "failed to fetch item count, using 0"
                    );
                    0
                }
            };
            results.push(LibraryWithCount {
                name: lib.name,
                collection_type: lib.collection_type.unwrap_or_default(),
                item_count: count,
            });
        }

        Ok(results)
    }

    #[allow(clippy::too_many_lines)]
    // reason: this is the central session-fanout function. Splitting per-metric
    // (paused, bitrate, transcode reason, play method, etc.) trades one
    // top-to-bottom read for several private helpers all touching the same
    // session-loop locals — a net loss in clarity at this size.
    fn collect_sessions(&self, result: Result<Vec<crate::client::Session>, CollectorError>) {
        // Reset session gauges to clear stale label combinations
        self.metrics.sessions_active.reset();
        self.metrics.session_paused.reset();
        self.metrics.sessions_active_count.set(0.0);
        self.metrics.transcodes_active.reset();
        self.metrics.transcode_hw_accelerated.set(0.0);
        self.metrics.transcode_reason_sessions.reset();
        self.metrics.stream_bitrate.reset();
        self.metrics.stream_direct.reset();
        self.metrics.transcode_completion.reset();
        self.metrics.play_method_sessions.reset();
        self.metrics.session_play_position_seconds.reset();
        self.metrics.session_remote_address.reset();

        let sessions = match result {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = ?e, "failed to fetch sessions");
                return;
            }
        };

        // Only count sessions with active playback
        let active: Vec<_> = sessions
            .iter()
            .filter(|s| s.now_playing_item.is_some())
            .collect();

        self.metrics.sessions_active_count.set(active.len() as f64);

        let mut direct_play_count = 0u64;
        let mut transcode_count = 0u64;
        let mut direct_stream_count = 0u64;
        let mut hw_accel_count = 0u64;
        // Transcode reasons are bounded (a handful per session, a handful of
        // distinct reasons across all sessions), so a small initial capacity
        // covers the common case without reallocating.
        let mut reason_counts: std::collections::HashMap<String, u64> =
            std::collections::HashMap::with_capacity(8);

        for session in &active {
            // Defense in depth: outer filter already excludes sessions with no
            // now_playing_item, but skip rather than panic if one slips through.
            let Some(item) = session.now_playing_item.as_ref() else {
                continue;
            };

            let play_method = sanitize_label(
                session
                    .play_state
                    .as_ref()
                    .and_then(|ps| ps.play_method.as_deref()),
            );
            let user = sanitize_label(session.user_name.as_deref());
            let client = sanitize_label(session.client.as_deref());
            let device = sanitize_label(session.device_name.as_deref());

            self.metrics
                .sessions_active
                .with_label_values(&[&user, &client, &play_method, &device])
                .set(1.0);

            // Paused state
            let is_paused = session
                .play_state
                .as_ref()
                .and_then(|ps| ps.is_paused)
                .unwrap_or(false);
            self.metrics
                .session_paused
                .with_label_values(&[&user, &client, &device])
                .set(if is_paused { 1.0 } else { 0.0 });

            // Playback position (Jellyfin reports ticks at 10,000,000 per second).
            if let Some(ticks) = session.play_state.as_ref().and_then(|ps| ps.position_ticks) {
                let item_type = sanitize_label(item.media_type.as_deref());
                #[allow(clippy::cast_precision_loss)]
                // reason: position_ticks is i64; for any realistic media item
                // (sub-billion-second runtimes) the f64 mantissa is ample.
                let seconds = ticks as f64 / 10_000_000.0;
                self.metrics
                    .session_play_position_seconds
                    .with_label_values(&[&user, &item_type])
                    .set(seconds);
            }

            // Remote address (opt-in; PII-adjacent).
            if self.expose_remote_address {
                if let Some(remote) = session.remote_end_point.as_deref() {
                    let remote_addr = sanitize_label(Some(remote));
                    self.metrics
                        .session_remote_address
                        .with_label_values(&[&user, &remote_addr])
                        .set(1.0);
                }
            }

            match play_method.as_ref() {
                "DirectPlay" => direct_play_count += 1,
                "Transcode" => transcode_count += 1,
                "DirectStream" => direct_stream_count += 1,
                _ => {}
            }

            // Bitrate: TranscodingInfo.Bitrate → NowPlayingItem.Bitrate → sum(MediaStreams)
            let bitrate = session
                .transcoding_info
                .as_ref()
                .and_then(|t| t.bitrate)
                .or(item.bitrate)
                .unwrap_or_else(|| {
                    item.media_streams
                        .as_ref()
                        .map_or(0, |streams| streams.iter().filter_map(|s| s.bit_rate).sum())
                });

            if bitrate > 0 {
                let media_type = item
                    .media_type
                    .as_deref()
                    .unwrap_or("unknown")
                    .to_lowercase();
                self.metrics
                    .stream_bitrate
                    .with_label_values(&[&*user, &media_type])
                    .set(bitrate as f64);
            }

            if play_method == "Transcode" {
                if let Some(ref transcoding) = session.transcoding_info {
                    // Per-codec transcode tracking — codec strings are
                    // case-normalized so H264/h264 don't double-up.
                    let video_codec = sanitize_codec_label(transcoding.video_codec.as_deref());
                    let audio_codec = sanitize_codec_label(transcoding.audio_codec.as_deref());
                    self.metrics
                        .transcodes_active
                        .with_label_values(&[&video_codec, &audio_codec])
                        .inc();

                    if transcoding.hardware_acceleration_type.is_some() {
                        hw_accel_count += 1;
                    }
                    if let Some(pct) = transcoding.completion_percentage {
                        self.metrics
                            .transcode_completion
                            .with_label_values(&[&user])
                            .set(pct);
                    }

                    // Transcode reasons — sanitize each before bucketing so a
                    // future Jellyfin extension or plugin emitting whitespace
                    // or oversized strings doesn't pollute label cardinality.
                    if let Some(ref reasons) = transcoding.transcode_reasons {
                        for reason in reasons {
                            let sanitized = sanitize_label(Some(reason.as_str()));
                            *reason_counts.entry(sanitized.into_owned()).or_insert(0) += 1;
                        }
                    }

                    // Stream direct/transcode per type
                    if let Some(is_direct) = transcoding.is_video_direct {
                        self.metrics
                            .stream_direct
                            .with_label_values(&[&*user, "video"])
                            .set(if is_direct { 1.0 } else { 0.0 });
                    }
                    if let Some(is_direct) = transcoding.is_audio_direct {
                        self.metrics
                            .stream_direct
                            .with_label_values(&[&*user, "audio"])
                            .set(if is_direct { 1.0 } else { 0.0 });
                    }
                }
            } else {
                // DirectPlay/DirectStream — both video and audio are direct
                self.metrics
                    .stream_direct
                    .with_label_values(&[&*user, "video"])
                    .set(1.0);
                self.metrics
                    .stream_direct
                    .with_label_values(&[&*user, "audio"])
                    .set(1.0);
            }
        }

        // Transcode reason aggregates
        for (reason, count) in &reason_counts {
            self.metrics
                .transcode_reason_sessions
                .with_label_values(&[reason])
                .set(*count as f64);
        }

        self.metrics
            .transcode_hw_accelerated
            .set(hw_accel_count as f64);
        self.metrics
            .play_method_sessions
            .with_label_values(&["DirectPlay"])
            .set(direct_play_count as f64);
        self.metrics
            .play_method_sessions
            .with_label_values(&["Transcode"])
            .set(transcode_count as f64);
        self.metrics
            .play_method_sessions
            .with_label_values(&["DirectStream"])
            .set(direct_stream_count as f64);
    }

    fn collect_libraries(&self, result: Result<Vec<LibraryWithCount>, CollectorError>) {
        self.metrics.library_items.reset();

        let libraries = match result {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = ?e, "failed to fetch libraries");
                return;
            }
        };

        for lib in &libraries {
            self.metrics
                .library_items
                .with_label_values(&[
                    &sanitize_label(Some(&lib.name)),
                    &sanitize_label(Some(&lib.collection_type)),
                ])
                .set(lib.item_count as f64);
        }
    }

    fn collect_item_counts(&self, result: Result<crate::client::ItemCounts, CollectorError>) {
        self.metrics.items_by_type.reset();
        self.metrics.items_count.set(0.0);

        let counts = match result {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = ?e, "failed to fetch item counts");
                return;
            }
        };

        for (media_type, count) in [
            ("Movie", counts.movie_count),
            ("Series", counts.series_count),
            ("Episode", counts.episode_count),
            ("Book", counts.book_count),
            ("Song", counts.song_count),
            ("Album", counts.album_count),
            ("Artist", counts.artist_count),
            ("Trailer", counts.trailer_count),
            ("MusicVideo", counts.music_video_count),
            ("BoxSet", counts.box_set_count),
        ] {
            self.metrics
                .items_by_type
                .with_label_values(&[media_type])
                .set(count as f64);
        }
        self.metrics.items_count.set(counts.item_count as f64);
    }

    fn collect_system_info(&self, result: Result<crate::client::SystemInfo, CollectorError>) {
        self.metrics.server_info.reset();

        let info = match result {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(error = ?e, "failed to fetch system info");
                return;
            }
        };

        self.metrics
            .server_info
            .with_label_values(&[
                &sanitize_label(Some(&info.version)),
                &sanitize_label(Some(&info.operating_system)),
                &sanitize_label(Some(&info.server_name)),
            ])
            .set(1.0);
    }
}

struct LibraryWithCount {
    name: String,
    collection_type: String,
    item_count: u64,
}

#[derive(Debug, thiserror::Error)]
enum CollectorError {
    #[error("circuit breaker open")]
    CircuitOpen,

    #[error(transparent)]
    Api(#[from] ClientError),
}

/// Sanitize a label value for Prometheus.
///
/// - Returns `"unknown"` for `None` or empty/whitespace-only input.
/// - Trims surrounding whitespace; collapses internal whitespace runs to a
///   single ASCII space.
/// - Truncates to [`MAX_LABEL_CHARS`] characters at a UTF-8 char boundary
///   (never inside a multi-byte codepoint).
/// - Returns `Cow::Borrowed` on the fast path (input already clean and short),
///   so the common case allocates nothing.
fn sanitize_label(value: Option<&str>) -> Cow<'_, str> {
    let Some(s) = value else {
        return Cow::Borrowed("unknown");
    };
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Cow::Borrowed("unknown");
    }

    // Fast path: only single ASCII spaces (no double-space runs, no \t/\n)
    // and short enough → borrow the slice as-is.
    let needs_collapse = trimmed.contains("  ")
        || trimmed
            .bytes()
            .any(|b| b.is_ascii_whitespace() && b != b' ');
    let char_count = trimmed.chars().count();

    if !needs_collapse && char_count <= MAX_LABEL_CHARS {
        return Cow::Borrowed(trimmed);
    }

    // Slow path: collapse whitespace into a fresh String.
    let mut out = String::with_capacity(trimmed.len());
    let mut iter = trimmed.split_whitespace();
    if let Some(first) = iter.next() {
        out.push_str(first);
        for word in iter {
            out.push(' ');
            out.push_str(word);
        }
    }

    // Truncate at a char boundary, not a byte boundary, so multi-byte UTF-8
    // input (e.g. CJK device names) never panics on slicing.
    if out.chars().count() > MAX_LABEL_CHARS {
        let cutoff = out
            .char_indices()
            .nth(MAX_LABEL_CHARS)
            .map_or(out.len(), |(i, _)| i);
        out.truncate(cutoff);
    }

    Cow::Owned(out)
}

/// Like [`sanitize_label`], but additionally normalizes ASCII case to
/// lowercase. Used for codec strings so that `H264`/`h264` collapse into one
/// label series instead of inflating cardinality.
fn sanitize_codec_label(value: Option<&str>) -> Cow<'_, str> {
    let sanitized = sanitize_label(value);
    if sanitized.bytes().any(|b| b.is_ascii_uppercase()) {
        let mut owned = sanitized.into_owned();
        owned.make_ascii_lowercase();
        Cow::Owned(owned)
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::*;

    // -- Mock Jellyfin API --

    struct MockJellyfinApi {
        sessions: Result<Vec<Session>, ClientError>,
        libraries: Result<Vec<Library>, ClientError>,
        item_counts: Result<ItemCounts, ClientError>,
        system_info: Result<SystemInfo, ClientError>,
        library_item_count: Result<u64, ClientError>,
        reachable: bool,
    }

    impl Default for MockJellyfinApi {
        fn default() -> Self {
            Self {
                sessions: Ok(vec![]),
                libraries: Ok(vec![]),
                item_counts: Ok(ItemCounts {
                    movie_count: 0,
                    series_count: 0,
                    episode_count: 0,
                    book_count: 0,
                    song_count: 0,
                    album_count: 0,
                    artist_count: 0,
                    trailer_count: 0,
                    music_video_count: 0,
                    box_set_count: 0,
                    item_count: 0,
                }),
                system_info: Ok(SystemInfo {
                    server_name: "test".into(),
                    version: "10.9.0".into(),
                    operating_system: "Linux".into(),
                }),
                library_item_count: Ok(0),
                reachable: true,
            }
        }
    }

    #[async_trait::async_trait]
    impl JellyfinApi for MockJellyfinApi {
        async fn get_sessions(&self) -> Result<Vec<Session>, ClientError> {
            self.sessions
                .as_ref()
                .map(Clone::clone)
                .map_err(|_| ClientError::Deserialization("mock error".into()))
        }

        async fn get_libraries(&self) -> Result<Vec<Library>, ClientError> {
            self.libraries
                .as_ref()
                .map(Clone::clone)
                .map_err(|_| ClientError::Deserialization("mock error".into()))
        }

        async fn get_item_counts(&self) -> Result<ItemCounts, ClientError> {
            self.item_counts
                .as_ref()
                .map(Clone::clone)
                .map_err(|_| ClientError::Deserialization("mock error".into()))
        }

        async fn get_system_info(&self) -> Result<SystemInfo, ClientError> {
            self.system_info
                .as_ref()
                .map(Clone::clone)
                .map_err(|_| ClientError::Deserialization("mock error".into()))
        }

        async fn get_library_item_count(&self, _parent_id: &str) -> Result<u64, ClientError> {
            self.library_item_count
                .as_ref()
                .copied()
                .map_err(|_| ClientError::Deserialization("mock error".into()))
        }

        async fn is_publicly_reachable(&self) -> bool {
            self.reachable
        }
    }

    fn make_collector(mock: MockJellyfinApi) -> Collector {
        Collector::new(
            Arc::new(mock),
            Arc::new(Metrics::new()),
            &CollectorConfig {
                scrape_interval: Duration::from_secs(10),
                failure_threshold: 5,
                reset_timeout: Duration::from_secs(60),
                retry_max_attempts: 0, // no retries in tests
                retry_base_delay: Duration::from_millis(10),
                retry_max_delay: Duration::from_millis(100),
                expose_remote_address: false,
            },
        )
    }

    fn make_active_session(user: &str, client: &str, play_method: &str) -> Session {
        Session {
            user_name: Some(user.into()),
            client: Some(client.into()),
            device_name: Some("TestDevice".into()),
            now_playing_item: Some(NowPlayingItem {
                name: Some("Test Movie".into()),
                media_type: Some("Video".into()),
                bitrate: Some(10_000_000),
                media_streams: None,
            }),
            play_state: Some(PlayState {
                play_method: Some(play_method.into()),
                is_paused: Some(false),
                position_ticks: None,
            }),
            transcoding_info: None,
            remote_end_point: None,
        }
    }

    #[tokio::test]
    async fn happy_path_all_metrics_populated() {
        let mock = MockJellyfinApi {
            sessions: Ok(vec![make_active_session("alice", "Infuse", "DirectPlay")]),
            libraries: Ok(vec![Library {
                name: "Movies".into(),
                collection_type: Some("movies".into()),
                item_id: "lib-1".into(),
            }]),
            item_counts: Ok(ItemCounts {
                movie_count: 150,
                series_count: 30,
                episode_count: 800,
                book_count: 20,
                song_count: 500,
                album_count: 40,
                artist_count: 50,
                trailer_count: 0,
                music_video_count: 0,
                box_set_count: 5,
                item_count: 1595,
            }),
            system_info: Ok(SystemInfo {
                server_name: "jellyfin".into(),
                version: "10.9.11".into(),
                operating_system: "Linux".into(),
            }),
            library_item_count: Ok(150),
            reachable: true,
        };

        let collector = make_collector(mock);
        collector.collect().await;

        let output = collector.metrics.encode();
        assert!(output.contains("jellyfin_up 1"));
        assert!(output.contains("jellyfin_metrics_stale 0"));
        assert!(output.contains("jellyfin_sessions_active_count 1"));
        assert!(output.contains(r#"jellyfin_items_by_type{type="Movie"} 150"#));
        assert!(output.contains(
            r#"jellyfin_library_items{library_name="Movies",library_type="movies"} 150"#
        ));
        assert!(output.contains(
            r#"jellyfin_server_info{os="Linux",server_name="jellyfin",version="10.9.11"} 1"#
        ));
        // Device label on sessions_active
        assert!(output.contains(r#"device="TestDevice"#));
        // DirectPlay session → stream_direct both 1
        assert!(output.contains(r#"jellyfin_stream_direct{type="video",user="alice"} 1"#));
        assert!(output.contains(r#"jellyfin_stream_direct{type="audio",user="alice"} 1"#));
        // Enriched item counts
        assert!(output.contains(r#"jellyfin_items_by_type{type="Artist"} 50"#));
        assert!(output.contains(r#"jellyfin_items_by_type{type="BoxSet"} 5"#));
        assert!(output.contains("jellyfin_items_count 1595"));
    }

    #[tokio::test]
    async fn unreachable_sets_stale() {
        let mock = MockJellyfinApi {
            reachable: false,
            ..Default::default()
        };

        let collector = make_collector(mock);
        collector.collect().await;

        let output = collector.metrics.encode();
        assert!(output.contains("jellyfin_up 0"));
        assert!(output.contains("jellyfin_metrics_stale 1"));
    }

    #[tokio::test]
    async fn session_filtering_only_active() {
        let idle_session = Session {
            user_name: Some("idle-user".into()),
            client: Some("Web".into()),
            device_name: Some("Chrome".into()),
            now_playing_item: None,
            play_state: None,
            transcoding_info: None,
            remote_end_point: None,
        };
        let active_session = make_active_session("active-user", "Infuse", "DirectPlay");

        let mock = MockJellyfinApi {
            sessions: Ok(vec![idle_session, active_session]),
            ..Default::default()
        };

        let collector = make_collector(mock);
        collector.collect().await;

        let output = collector.metrics.encode();
        assert!(output.contains("jellyfin_sessions_active_count 1"));
        assert!(output.contains("active-user"));
        assert!(!output.contains("idle-user"));
    }

    #[tokio::test]
    async fn bitrate_fallback_chain() {
        // TranscodingInfo.Bitrate takes priority
        let session_with_transcode = Session {
            user_name: Some("user1".into()),
            client: Some("Web".into()),
            device_name: Some("Chrome".into()),
            now_playing_item: Some(NowPlayingItem {
                name: Some("Movie".into()),
                media_type: Some("Video".into()),
                bitrate: Some(5_000_000),
                media_streams: Some(vec![
                    MediaStream {
                        bit_rate: Some(3_000_000),
                    },
                    MediaStream {
                        bit_rate: Some(1_000_000),
                    },
                ]),
            }),
            play_state: Some(PlayState {
                play_method: Some("Transcode".into()),
                is_paused: Some(false),
                position_ticks: None,
            }),
            transcoding_info: Some(TranscodingInfo {
                bitrate: Some(8_000_000),
                completion_percentage: Some(50.0),
                hardware_acceleration_type: None,
                video_codec: Some("h264".into()),
                audio_codec: Some("aac".into()),
                transcode_reasons: Some(vec!["ContainerBitrateExceedsLimit".into()]),
                is_video_direct: Some(false),
                is_audio_direct: Some(true),
            }),
            remote_end_point: None,
        };

        let mock = MockJellyfinApi {
            sessions: Ok(vec![session_with_transcode]),
            ..Default::default()
        };

        let collector = make_collector(mock);
        collector.collect().await;

        let output = collector.metrics.encode();
        // Should use TranscodingInfo.Bitrate (8M), not Item.Bitrate (5M) or sum (4M)
        assert!(
            output.contains(
                r#"jellyfin_stream_bitrate_bps{media_type="video",user="user1"} 8000000"#
            )
        );
    }

    #[tokio::test]
    async fn bitrate_fallback_to_media_streams() {
        let session = Session {
            user_name: Some("user2".into()),
            client: Some("Web".into()),
            device_name: Some("Firefox".into()),
            now_playing_item: Some(NowPlayingItem {
                name: Some("Movie".into()),
                media_type: Some("Video".into()),
                bitrate: None,
                media_streams: Some(vec![
                    MediaStream {
                        bit_rate: Some(3_000_000),
                    },
                    MediaStream {
                        bit_rate: Some(1_000_000),
                    },
                ]),
            }),
            play_state: Some(PlayState {
                play_method: Some("DirectPlay".into()),
                is_paused: Some(false),
                position_ticks: None,
            }),
            transcoding_info: None,
            remote_end_point: None,
        };

        let mock = MockJellyfinApi {
            sessions: Ok(vec![session]),
            ..Default::default()
        };

        let collector = make_collector(mock);
        collector.collect().await;

        let output = collector.metrics.encode();
        // Should sum MediaStreams: 3M + 1M = 4M
        assert!(
            output.contains(
                r#"jellyfin_stream_bitrate_bps{media_type="video",user="user2"} 4000000"#
            )
        );
    }

    #[tokio::test]
    async fn library_item_counts_real_values() {
        let mock = MockJellyfinApi {
            libraries: Ok(vec![
                Library {
                    name: "Movies".into(),
                    collection_type: Some("movies".into()),
                    item_id: "lib-1".into(),
                },
                Library {
                    name: "TV Shows".into(),
                    collection_type: Some("tvshows".into()),
                    item_id: "lib-2".into(),
                },
            ]),
            library_item_count: Ok(42),
            ..Default::default()
        };

        let collector = make_collector(mock);
        collector.collect().await;

        let output = collector.metrics.encode();
        // Both libraries should have count 42 (mock returns same value for all)
        assert!(
            output.contains(
                r#"jellyfin_library_items{library_name="Movies",library_type="movies"} 42"#
            )
        );
        assert!(output.contains(
            r#"jellyfin_library_items{library_name="TV Shows",library_type="tvshows"} 42"#
        ));
    }

    #[tokio::test]
    async fn partial_failure_other_metrics_still_populated() {
        let mock = MockJellyfinApi {
            sessions: Err(ClientError::Timeout),
            item_counts: Ok(ItemCounts {
                movie_count: 100,
                series_count: 0,
                episode_count: 0,
                book_count: 0,
                song_count: 0,
                album_count: 0,
                artist_count: 0,
                trailer_count: 0,
                music_video_count: 0,
                box_set_count: 0,
                item_count: 100,
            }),
            ..Default::default()
        };

        let collector = make_collector(mock);
        collector.collect().await;

        let output = collector.metrics.encode();
        assert!(output.contains("jellyfin_up 1"));
        // Items should still be populated despite sessions failing
        assert!(output.contains(r#"jellyfin_items_by_type{type="Movie"} 100"#));
    }

    #[tokio::test]
    async fn no_delay_burn_when_circuit_open() {
        // Regression guard for the breaker-wraps-retry layering: when the
        // breaker is open, fetch_with_resilience must short-circuit before
        // entering the retry loop. The previous retry-wraps-breaker order
        // would burn ~retry_max_attempts × retry_base_delay even after the
        // breaker opened.

        struct AlwaysFailingApi;
        #[async_trait::async_trait]
        impl JellyfinApi for AlwaysFailingApi {
            async fn get_sessions(&self) -> Result<Vec<Session>, ClientError> {
                Err(ClientError::Timeout)
            }
            async fn get_libraries(&self) -> Result<Vec<Library>, ClientError> {
                Err(ClientError::Timeout)
            }
            async fn get_item_counts(&self) -> Result<ItemCounts, ClientError> {
                Err(ClientError::Timeout)
            }
            async fn get_system_info(&self) -> Result<SystemInfo, ClientError> {
                Err(ClientError::Timeout)
            }
            async fn get_library_item_count(&self, _id: &str) -> Result<u64, ClientError> {
                Err(ClientError::Timeout)
            }
            async fn is_publicly_reachable(&self) -> bool {
                true
            }
        }

        let collector = Collector::new(
            Arc::new(AlwaysFailingApi),
            Arc::new(Metrics::new()),
            &CollectorConfig {
                scrape_interval: Duration::from_secs(10),
                failure_threshold: 1, // open on the very first failure
                reset_timeout: Duration::from_secs(60),
                // 2 retries × 200 ms base = a regression to retry-wraps-breaker
                // would burn ≥200 ms per fetch before seeing CB::Open.
                retry_max_attempts: 2,
                retry_base_delay: Duration::from_millis(200),
                retry_max_delay: Duration::from_millis(500),
                expose_remote_address: false,
            },
        );

        // First collect: trips the breaker.
        collector.collect().await;
        assert!(
            !collector.is_ready(),
            "breaker must be open after a failure burst"
        );

        // Second collect: every fetch must fast-fail via CircuitBreakerError::Open
        // before retry runs. Anything close to retry_base_delay would prove a
        // regression to retry-wraps-breaker.
        let start = std::time::Instant::now();
        collector.collect().await;
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(50),
            "collect with open breaker took {elapsed:?} — retry burned delay"
        );
    }

    #[tokio::test]
    async fn transcode_hw_acceleration_counted() {
        let session = Session {
            user_name: Some("user".into()),
            client: Some("Web".into()),
            device_name: Some("Chrome".into()),
            now_playing_item: Some(NowPlayingItem {
                name: Some("Movie".into()),
                media_type: Some("Video".into()),
                bitrate: Some(10_000_000),
                media_streams: None,
            }),
            play_state: Some(PlayState {
                play_method: Some("Transcode".into()),
                is_paused: Some(false),
                position_ticks: None,
            }),
            transcoding_info: Some(TranscodingInfo {
                bitrate: Some(8_000_000),
                completion_percentage: Some(75.0),
                hardware_acceleration_type: Some("vaapi".into()),
                video_codec: Some("h264".into()),
                audio_codec: Some("aac".into()),
                transcode_reasons: None,
                is_video_direct: Some(false),
                is_audio_direct: Some(false),
            }),
            remote_end_point: None,
        };

        let mock = MockJellyfinApi {
            sessions: Ok(vec![session]),
            ..Default::default()
        };

        let collector = make_collector(mock);
        collector.collect().await;

        let output = collector.metrics.encode();
        assert!(
            output
                .contains(r#"jellyfin_transcodes_active{audio_codec="aac",video_codec="h264"} 1"#)
        );
        assert!(output.contains("jellyfin_transcode_hw_accelerated 1"));
        assert!(output.contains(r#"jellyfin_transcode_completion_pct{user="user"} 75"#));
        // stream_direct should show 0 for both (transcoding)
        assert!(output.contains(r#"jellyfin_stream_direct{type="video",user="user"} 0"#));
        assert!(output.contains(r#"jellyfin_stream_direct{type="audio",user="user"} 0"#));
    }

    #[tokio::test]
    async fn enriched_transcode_metrics() {
        let session = Session {
            user_name: Some("alice".into()),
            client: Some("Infuse".into()),
            device_name: Some("Apple TV".into()),
            now_playing_item: Some(NowPlayingItem {
                name: Some("Movie".into()),
                media_type: Some("Video".into()),
                bitrate: Some(20_000_000),
                media_streams: None,
            }),
            play_state: Some(PlayState {
                play_method: Some("Transcode".into()),
                is_paused: Some(true),
                position_ticks: None,
            }),
            transcoding_info: Some(TranscodingInfo {
                bitrate: Some(15_000_000),
                completion_percentage: Some(60.0),
                hardware_acceleration_type: Some("vaapi".into()),
                video_codec: Some("hevc".into()),
                audio_codec: Some("ac3".into()),
                transcode_reasons: Some(vec![
                    "ContainerBitrateExceedsLimit".into(),
                    "VideoCodecNotSupported".into(),
                ]),
                is_video_direct: Some(false),
                is_audio_direct: Some(true),
            }),
            remote_end_point: None,
        };

        let mock = MockJellyfinApi {
            sessions: Ok(vec![session]),
            ..Default::default()
        };

        let collector = make_collector(mock);
        collector.collect().await;

        let output = collector.metrics.encode();

        // Device label on sessions_active
        assert!(output.contains(
            r#"jellyfin_sessions_active{client="Infuse",device="Apple TV",play_method="Transcode",user="alice"} 1"#
        ));
        // Paused
        assert!(output.contains(
            r#"jellyfin_session_paused{client="Infuse",device="Apple TV",user="alice"} 1"#
        ));
        // Codec labels on transcodes_active
        assert!(
            output
                .contains(r#"jellyfin_transcodes_active{audio_codec="ac3",video_codec="hevc"} 1"#)
        );
        // Transcode reasons
        assert!(output.contains(
            r#"jellyfin_transcode_reason_sessions{reason="ContainerBitrateExceedsLimit"} 1"#
        ));
        assert!(
            output.contains(
                r#"jellyfin_transcode_reason_sessions{reason="VideoCodecNotSupported"} 1"#
            )
        );
        // Stream direct: video transcoded (0), audio direct (1)
        assert!(output.contains(r#"jellyfin_stream_direct{type="video",user="alice"} 0"#));
        assert!(output.contains(r#"jellyfin_stream_direct{type="audio",user="alice"} 1"#));
    }

    // -- Label sanitization tests --

    #[test]
    fn sanitize_none_returns_unknown() {
        assert_eq!(sanitize_label(None), "unknown");
    }

    #[test]
    fn sanitize_empty_returns_unknown() {
        assert_eq!(sanitize_label(Some("")), "unknown");
        assert_eq!(sanitize_label(Some("   ")), "unknown");
    }

    #[test]
    fn sanitize_trims_and_collapses() {
        assert_eq!(sanitize_label(Some("  hello   world  ")), "hello world");
    }

    #[test]
    fn sanitize_truncates_to_64_chars() {
        let long = "a".repeat(100);
        let result = sanitize_label(Some(&long));
        assert_eq!(result.chars().count(), MAX_LABEL_CHARS);
    }

    #[test]
    fn sanitize_truncates_at_char_boundary_for_multibyte() {
        // 50 multi-byte chars (3 bytes each), then 50 ASCII chars.
        // Old byte-slice would have panicked at byte 64 (mid-codepoint);
        // char-aware truncation must split cleanly at char 64.
        let multibyte = "あ".repeat(50);
        let mixed = format!("{multibyte}{}", "a".repeat(50));
        let result = sanitize_label(Some(&mixed));
        assert_eq!(result.chars().count(), MAX_LABEL_CHARS);
        // Must still be valid UTF-8 (panic-free guarantee)
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn sanitize_codec_label_lowercases() {
        assert_eq!(sanitize_codec_label(Some("H264")), "h264");
        assert_eq!(sanitize_codec_label(Some("HEVC")), "hevc");
    }

    #[test]
    fn sanitize_codec_label_preserves_lowercase() {
        // No-op fast path when already lowercase
        assert_eq!(sanitize_codec_label(Some("h264")), "h264");
    }

    #[test]
    fn sanitize_label_borrows_clean_input() {
        // Fast path: short, single-spaced input → Cow::Borrowed (no allocation)
        let result = sanitize_label(Some("clean input"));
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn sanitize_label_owns_collapsed_input() {
        // Slow path: double space forces collapse → Cow::Owned
        let result = sanitize_label(Some("two  spaces"));
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result, "two spaces");
    }
}
