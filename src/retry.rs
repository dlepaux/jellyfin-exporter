use std::time::Duration;

/// Retry an operation with exponential backoff plus jitter.
///
/// Layering: this is the *inner* layer. The collector wraps it inside the
/// circuit breaker (`cb.execute(|| with_retry(http_call))`) so retries only
/// run when the breaker is closed/half-open.
///
/// Backoff schedule for attempt N: `base_delay × 2^N + jitter`, capped at
/// `max_delay`. Jitter is uniform in `[0, base_delay)` so two callers
/// retrying in lockstep desynchronize within one base interval.
pub async fn with_retry<F, Fut, T, E>(
    f: F,
    max_attempts: u32,
    base_delay: Duration,
    max_delay: Duration,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let total_attempts = max_attempts + 1; // initial + retries

    let mut last_err = None;

    for attempt in 0..total_attempts {
        match f().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                last_err = Some(e);

                // Don't sleep after the last attempt
                if attempt + 1 < total_attempts {
                    let delay = backoff_delay(attempt, base_delay, max_delay);
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    // All attempts exhausted — return the last error.
    // invariant: total_attempts ≥ 1, so the loop body always executes at
    // least once and last_err is always Some at this point.
    Err(last_err.expect("loop always runs at least once"))
}

/// Compute the backoff delay for attempt `attempt` (zero-indexed).
///
/// Jitter is scaled to `base_delay` rather than a hardcoded constant: with
/// a tiny `base_delay` (tests), large hardcoded jitter would overwhelm the
/// real schedule; with a large `base_delay` (production), small hardcoded
/// jitter is too narrow to meaningfully desynchronize.
fn backoff_delay(attempt: u32, base_delay: Duration, max_delay: Duration) -> Duration {
    let exponential = base_delay.saturating_mul(2u32.saturating_pow(attempt));
    // Clamp the millisecond count into u64 for fastrand. base_delay is bounded
    // by config validation well below u64::MAX; saturating to u64::MAX is
    // unreachable in practice but keeps the cast total.
    let jitter_ceiling =
        u64::try_from(base_delay.as_millis().min(u128::from(u64::MAX))).unwrap_or(u64::MAX);
    let jitter = if jitter_ceiling == 0 {
        Duration::ZERO
    } else {
        Duration::from_millis(fastrand::u64(0..jitter_ceiling))
    };
    let total = exponential.saturating_add(jitter);
    total.min(max_delay)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::Cell;

    #[tokio::test]
    async fn succeeds_on_first_try() {
        let result = with_retry(
            || async { Ok::<_, &str>(42) },
            3,
            Duration::from_millis(10),
            Duration::from_millis(100),
        )
        .await;

        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn succeeds_after_retries() {
        let attempts = Cell::new(0u32);

        let result = with_retry(
            || {
                let count = attempts.get() + 1;
                attempts.set(count);
                async move {
                    if count < 3 {
                        Err::<u32, &str>("not yet")
                    } else {
                        Ok(99)
                    }
                }
            },
            3,
            Duration::from_millis(1),
            Duration::from_millis(10),
        )
        .await;

        assert_eq!(result.unwrap(), 99);
        assert_eq!(attempts.get(), 3);
    }

    #[tokio::test]
    async fn exhausts_all_attempts() {
        let attempts = Cell::new(0u32);

        let result = with_retry(
            || {
                attempts.set(attempts.get() + 1);
                async { Err::<(), &str>("always fails") }
            },
            2,
            Duration::from_millis(1),
            Duration::from_millis(10),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.get(), 3); // 1 initial + 2 retries
    }

    #[tokio::test]
    async fn zero_retries_means_single_attempt() {
        let attempts = Cell::new(0u32);

        let result = with_retry(
            || {
                attempts.set(attempts.get() + 1);
                async { Err::<(), &str>("fail") }
            },
            0,
            Duration::from_millis(1),
            Duration::from_millis(10),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn backoff_delay_grows_exponentially() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(10);

        // Attempt 0: 100ms * 2^0 = 100ms + jitter
        let d0 = backoff_delay(0, base, max);
        assert!(d0 >= Duration::from_millis(100));
        assert!(d0 <= Duration::from_millis(300)); // 100 + max 200 jitter

        // Attempt 2: 100ms * 2^2 = 400ms + jitter
        let d2 = backoff_delay(2, base, max);
        assert!(d2 >= Duration::from_millis(400));
        assert!(d2 <= Duration::from_millis(600));
    }

    #[test]
    fn backoff_delay_capped_at_max() {
        let base = Duration::from_millis(100);
        let max = Duration::from_millis(500);

        // Attempt 10: 100ms * 2^10 = 102400ms, should be capped to 500ms
        let d = backoff_delay(10, base, max);
        assert_eq!(d, max);
    }
}
