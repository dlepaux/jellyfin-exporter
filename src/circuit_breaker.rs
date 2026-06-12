use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Source of "what time is it now", so the breaker's reset-timeout logic can
/// be driven by a deterministic clock in tests instead of real wall-clock
/// sleeps.
///
/// Production uses [`SystemClock`] — the default type parameter — which is a
/// zero-sized wrapper over [`Instant::now`]. The generic monomorphizes to a
/// direct call with no runtime cost. `tokio::time::pause` cannot substitute
/// here: the breaker measures elapsed time with [`std::time::Instant`], which
/// tokio's time mocking does not affect.
pub trait Clock {
    fn now(&self) -> Instant;
}

/// Production [`Clock`] backed by the real monotonic clock.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Circuit breaker state machine.
///
/// - `Closed` → `Open` (after `failure_threshold` consecutive failures)
/// - `Open` → `HalfOpen` (after `reset_timeout` elapses)
/// - `HalfOpen` → `Closed` (on success) or `Open` (on failure)
pub struct CircuitBreaker<C = SystemClock> {
    state: Mutex<CircuitState>,
    failure_threshold: u32,
    reset_timeout: Duration,
    clock: C,
}

#[derive(Debug, Clone, Copy)]
enum CircuitState {
    Closed { consecutive_failures: u32 },
    Open { since: Instant },
    HalfOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitStatus {
    Closed,
    Open,
    HalfOpen,
}

impl fmt::Display for CircuitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => write!(f, "closed"),
            Self::Open => write!(f, "open"),
            Self::HalfOpen => write!(f, "half-open"),
        }
    }
}

/// Outcome of a [`CircuitBreaker::execute`] call.
///
/// Generic over the inner error type `E` so callers preserve the original
/// error variant — the breaker no longer boxes-and-stringifies, which means
/// downstream code can `match` on specific failure modes (timeout vs auth vs
/// deserialization) instead of inspecting a `String`.
#[derive(Debug, thiserror::Error)]
pub enum CircuitBreakerError<E>
where
    E: std::error::Error + 'static,
{
    /// The breaker is currently open and refused to invoke the operation.
    /// `retry_after` is the remaining time before the next half-open probe.
    #[error("circuit breaker is open, retry after {retry_after:?}")]
    Open { retry_after: Duration },

    /// The operation was invoked and returned an error. The breaker has
    /// already accounted for this failure.
    #[error(transparent)]
    Inner(E),
}

impl CircuitBreaker<SystemClock> {
    /// Construct a breaker that opens after `failure_threshold` consecutive
    /// failures and tries to recover after `reset_timeout` has elapsed.
    #[must_use]
    pub const fn new(failure_threshold: u32, reset_timeout: Duration) -> Self {
        Self {
            state: Mutex::new(CircuitState::Closed {
                consecutive_failures: 0,
            }),
            failure_threshold,
            reset_timeout,
            clock: SystemClock,
        }
    }
}

impl<C: Clock> CircuitBreaker<C> {
    /// Current breaker status — read once and snapshot, the lock is released
    /// before this returns.
    #[must_use]
    pub fn status(&self) -> CircuitStatus {
        let snapshot = *lock_state(&self.state);
        match snapshot {
            CircuitState::Closed { .. } => CircuitStatus::Closed,
            CircuitState::Open { .. } => CircuitStatus::Open,
            CircuitState::HalfOpen => CircuitStatus::HalfOpen,
        }
    }

    /// Execute an operation through the circuit breaker.
    ///
    /// - If closed: execute and track failures.
    /// - If open and timeout elapsed: transition to half-open, execute one probe.
    /// - If open and timeout not elapsed: return `CircuitBreakerError::Open`
    ///   without invoking the operation.
    /// - If half-open: execute one probe; success closes, failure re-opens.
    ///
    /// When the breaker wraps a retry layer (the layering used by the
    /// collector), each invocation here corresponds to a complete retry
    /// sequence — the breaker only sees sustained failures, not transient
    /// blips that retry covers up.
    pub async fn execute<F, Fut, T, E>(&self, f: F) -> Result<T, CircuitBreakerError<E>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: std::error::Error + 'static,
    {
        // Check state and decide whether to proceed. The lock is released
        // (`drop(guard)`) before `f().await`; the guard is never held across
        // an await point.
        let decision = {
            let mut guard = lock_state(&self.state);
            let snapshot = *guard;
            let outcome = match snapshot {
                CircuitState::Open { since } => {
                    let elapsed = self.clock.now().saturating_duration_since(since);
                    if elapsed >= self.reset_timeout {
                        *guard = CircuitState::HalfOpen;
                        ProceedDecision::Run
                    } else {
                        ProceedDecision::Reject(self.reset_timeout.saturating_sub(elapsed))
                    }
                }
                CircuitState::Closed { .. } | CircuitState::HalfOpen => ProceedDecision::Run,
            };
            drop(guard);
            outcome
        };
        if let ProceedDecision::Reject(retry_after) = decision {
            return Err(CircuitBreakerError::Open { retry_after });
        }

        // Execute the operation outside the lock
        match f().await {
            Ok(value) => {
                self.record_success();
                Ok(value)
            }
            Err(e) => {
                self.record_failure();
                Err(CircuitBreakerError::Inner(e))
            }
        }
    }

    fn record_success(&self) {
        let mut state = lock_state(&self.state);
        *state = CircuitState::Closed {
            consecutive_failures: 0,
        };
    }

    fn record_failure(&self) {
        let mut guard = lock_state(&self.state);
        let snapshot = *guard;
        match snapshot {
            CircuitState::Closed {
                consecutive_failures,
            } => {
                let new_count = consecutive_failures + 1;
                *guard = if new_count >= self.failure_threshold {
                    CircuitState::Open {
                        since: self.clock.now(),
                    }
                } else {
                    CircuitState::Closed {
                        consecutive_failures: new_count,
                    }
                };
            }
            CircuitState::HalfOpen => {
                *guard = CircuitState::Open {
                    since: self.clock.now(),
                };
            }
            CircuitState::Open { .. } => {
                // Already open, no change
            }
        }
    }
}

enum ProceedDecision {
    Run,
    Reject(Duration),
}

#[cfg(test)]
impl<C: Clock> CircuitBreaker<C> {
    /// Construct a breaker with an explicit [`Clock`]. Test-only — production
    /// goes through [`CircuitBreaker::new`], which pins the clock to
    /// [`SystemClock`].
    const fn with_clock(failure_threshold: u32, reset_timeout: Duration, clock: C) -> Self {
        Self {
            state: Mutex::new(CircuitState::Closed {
                consecutive_failures: 0,
            }),
            failure_threshold,
            reset_timeout,
            clock,
        }
    }
}

/// Acquire the state lock, recovering from a poisoned mutex.
///
/// A poisoned mutex means a prior holder panicked while holding the lock —
/// the inner state is still well-formed because every mutator in this module
/// performs a single assignment, so we recover the inner value rather than
/// propagate the panic and take down the collector loop.
fn lock_state(mutex: &Mutex<CircuitState>) -> MutexGuard<'_, CircuitState> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Debug, thiserror::Error)]
    #[error("test error")]
    struct TestError;

    /// Deterministic [`Clock`] for time-sensitive breaker tests. Holds a fixed
    /// base [`Instant`] plus an advanceable offset, so `advance` moves the
    /// breaker's reset window forward with zero real elapsed time and no
    /// sleeps. `std::time::Instant` cannot be constructed from a raw value, so
    /// `now` returns `base + offset` rather than a synthetic instant.
    struct MockClock {
        base: Instant,
        offset_nanos: AtomicU64,
    }

    impl MockClock {
        fn new() -> Self {
            Self {
                base: Instant::now(),
                offset_nanos: AtomicU64::new(0),
            }
        }

        fn advance(&self, by: Duration) {
            self.offset_nanos
                .fetch_add(by.as_nanos() as u64, Ordering::Relaxed);
        }
    }

    impl Clock for MockClock {
        fn now(&self) -> Instant {
            self.base + Duration::from_nanos(self.offset_nanos.load(Ordering::Relaxed))
        }
    }

    // Lets a test keep ownership of the clock (to call `advance`) while the
    // breaker borrows it. No `Arc` needed: both live in the same test scope.
    impl Clock for &MockClock {
        fn now(&self) -> Instant {
            (**self).now()
        }
    }

    #[tokio::test]
    async fn starts_closed() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(60));
        assert_eq!(cb.status(), CircuitStatus::Closed);
    }

    #[tokio::test]
    async fn stays_closed_below_threshold() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(60));

        // Two failures — below threshold of 3
        for _ in 0..2 {
            let _ = cb.execute(|| async { Err::<(), _>(TestError) }).await;
        }

        assert_eq!(cb.status(), CircuitStatus::Closed);
    }

    #[tokio::test]
    async fn opens_at_threshold() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(60));

        for _ in 0..3 {
            let _ = cb.execute(|| async { Err::<(), _>(TestError) }).await;
        }

        assert_eq!(cb.status(), CircuitStatus::Open);
    }

    #[tokio::test]
    async fn open_rejects_immediately() {
        let cb = CircuitBreaker::new(1, Duration::from_secs(60));

        let _ = cb.execute(|| async { Err::<(), _>(TestError) }).await;
        assert_eq!(cb.status(), CircuitStatus::Open);

        let result = cb.execute(|| async { Ok::<_, TestError>(()) }).await;
        assert!(matches!(result, Err(CircuitBreakerError::Open { .. })));
    }

    #[tokio::test]
    async fn transitions_to_half_open_after_timeout() {
        let clock = MockClock::new();
        let cb = CircuitBreaker::with_clock(1, Duration::from_millis(10), &clock);

        let _ = cb.execute(|| async { Err::<(), _>(TestError) }).await;
        assert_eq!(cb.status(), CircuitStatus::Open);

        // Advance past the reset window deterministically — no real sleep.
        clock.advance(Duration::from_millis(11));

        // Next execute should transition to half-open and run the probe
        let result = cb.execute(|| async { Ok::<_, TestError>(42) }).await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(cb.status(), CircuitStatus::Closed);
    }

    #[tokio::test]
    async fn half_open_failure_reopens() {
        let clock = MockClock::new();
        let cb = CircuitBreaker::with_clock(1, Duration::from_millis(10), &clock);

        let _ = cb.execute(|| async { Err::<(), _>(TestError) }).await;

        // Advance past the reset window deterministically — no real sleep.
        clock.advance(Duration::from_millis(11));

        // Probe fails — should re-open
        let _ = cb.execute(|| async { Err::<(), _>(TestError) }).await;
        assert_eq!(cb.status(), CircuitStatus::Open);
    }

    #[tokio::test]
    async fn open_rejects_until_clock_advances() {
        let clock = MockClock::new();
        let cb = CircuitBreaker::with_clock(1, Duration::from_millis(10), &clock);

        let _ = cb.execute(|| async { Err::<(), _>(TestError) }).await;
        assert_eq!(cb.status(), CircuitStatus::Open);

        // Just shy of the reset window: still rejected without invoking the op.
        clock.advance(Duration::from_millis(9));
        let probed = std::cell::Cell::new(false);
        let result = cb
            .execute(|| async {
                probed.set(true);
                Ok::<_, TestError>(())
            })
            .await;
        assert!(matches!(result, Err(CircuitBreakerError::Open { .. })));
        assert!(!probed.get(), "operation must not run while still open");
        assert_eq!(cb.status(), CircuitStatus::Open);
    }

    #[tokio::test]
    async fn success_resets_failure_count() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(60));

        // Two failures
        for _ in 0..2 {
            let _ = cb.execute(|| async { Err::<(), _>(TestError) }).await;
        }

        // One success resets the count
        let _ = cb.execute(|| async { Ok::<_, TestError>(()) }).await;

        // Two more failures — should still be closed (count was reset)
        for _ in 0..2 {
            let _ = cb.execute(|| async { Err::<(), _>(TestError) }).await;
        }

        assert_eq!(cb.status(), CircuitStatus::Closed);
    }
}
