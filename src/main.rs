use std::process;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use jellyfin_exporter::client::JellyfinClient;
use jellyfin_exporter::config::Config;
use jellyfin_exporter::{AppState, Collector, CollectorConfig, Metrics, build_router};

#[tokio::main]
async fn main() {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("configuration error: {err}");
            process::exit(1);
        }
    };

    // Init tracing after config so we can use the configured log level
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("jellyfin_exporter={}", config.log_level)));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!(
        url = %config.jellyfin_url,
        port = config.port,
        scrape_interval_ms = config.scrape_interval_ms,
        "starting jellyfin-exporter"
    );

    if config.jellyfin_url.starts_with("http://") {
        tracing::warn!(
            url = %config.jellyfin_url,
            "JELLYFIN_URL uses plaintext http://; the API key will travel \
             in cleartext over this hop. Use https:// for any path that \
             leaves a trusted Docker/LAN network."
        );
    }

    let client = match JellyfinClient::new(
        &config.jellyfin_url,
        &config.jellyfin_api_key,
        Duration::from_millis(config.request_timeout_ms),
    ) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("failed to create HTTP client: {err}");
            process::exit(1);
        }
    };

    let metrics = Arc::new(Metrics::new());

    let collector = Arc::new(Collector::new(
        Arc::new(client),
        Arc::clone(&metrics),
        &CollectorConfig {
            scrape_interval: Duration::from_millis(config.scrape_interval_ms),
            failure_threshold: config.circuit_breaker_threshold,
            reset_timeout: Duration::from_millis(config.circuit_breaker_reset_ms),
            retry_max_attempts: config.retry_max_attempts,
            retry_base_delay: Duration::from_millis(config.retry_base_delay_ms),
            retry_max_delay: Duration::from_millis(config.retry_max_delay_ms),
            expose_remote_address: config.expose_remote_address,
        },
    ));

    let state = Arc::new(AppState {
        metrics,
        collector: Arc::clone(&collector),
        metrics_token: config.metrics_token,
    });

    let app = build_router(state);
    let cancel = CancellationToken::new();

    // Spawn the collector background loop
    let collector_cancel = cancel.clone();
    tokio::spawn(async move {
        collector.run(collector_cancel).await;
    });

    // Bind the HTTP server
    let listener = match tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("failed to bind to port {}: {err}", config.port);
            process::exit(1);
        }
    };

    tracing::info!(port = config.port, "HTTP server listening");

    // Serve with graceful shutdown
    let shutdown_cancel = cancel.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            tracing::info!("shutdown signal received");
            shutdown_cancel.cancel();
        })
        .await
        .unwrap_or_else(|err| {
            eprintln!("server error: {err}");
            process::exit(1);
        });
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }
}
