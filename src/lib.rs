//! Prometheus exporter for the [Jellyfin] media server.
//!
//! This crate ships as a binary (`jellyfin-exporter`); the library form
//! exists purely to keep the binary thin and to give tests access to the
//! pieces that compose it.
//!
//! # Architecture
//!
//! The exporter runs a background collection loop on a configurable
//! interval. The HTTP `/metrics` endpoint serves the snapshot from the most
//! recent successful collection — it does not trigger a live API call, so
//! Prometheus scrapes are cheap and decoupled from Jellyfin's availability.
//!
//! ```text
//!     env vars  ──► [config]
//!                       │
//!                       ▼
//!     [client] ◄── [collector] ──► [metrics] ──► /metrics
//!         │           │
//!         │           ├── retry (exponential backoff with jitter)
//!         │           └── circuit_breaker (sustained-failure cutoff)
//!         ▼
//!     Jellyfin HTTP API
//! ```
//!
//! Layering of the resilience pipeline is deliberate: the circuit breaker
//! wraps retry. Each breaker invocation corresponds to a complete retry
//! sequence, so transient blips that retry covers up never trip the
//! breaker, and an open breaker fast-fails without burning retry delay.
//!
//! When Jellyfin is unreachable, the collector keeps serving the last
//! successful snapshot, sets `jellyfin_up=0` and `jellyfin_metrics_stale=1`,
//! and resumes cleanly when the server returns. Dashboards do not blank.
//!
//! # Public surface
//!
//! Only the items re-exported at the crate root are intended for external
//! use. Internal modules (`circuit_breaker`, `retry`, `metrics`,
//! `collector`, `server`) are `pub(crate)` — their existence is an
//! implementation detail.
//!
//! [Jellyfin]: https://jellyfin.org

// Prometheus's Gauge::set takes f64 by API design. The values we feed in
// (item counts, session counts, byte counts, percentages) all fit losslessly
// in an f64 mantissa (≤ 2^53 ≈ 9e15) — we never observe values approaching
// that ceiling on a Jellyfin instance. Allowing the cast at crate level
// avoids per-call attribute noise on every Gauge.set(x as f64) site.
#![allow(clippy::cast_precision_loss)]

pub mod client;
pub mod config;

pub(crate) mod circuit_breaker;
pub(crate) mod collector;
pub(crate) mod metrics;
pub(crate) mod retry;
pub(crate) mod server;

pub use crate::collector::{Collector, CollectorConfig};
pub use crate::metrics::Metrics;
pub use crate::server::{AppState, build_router};
