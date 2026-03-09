use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Exponential backoff with jitter for WebSocket reconnection.
///
/// Starts at `base_delay_ms` and doubles up to `max_delay_ms`, adding
/// random jitter to avoid thundering herd problems.
pub struct ReconnectPolicy {
    /// Current attempt number (0-indexed).
    attempt: AtomicU32,
    /// Base delay in milliseconds.
    base_delay_ms: u64,
    /// Maximum delay in milliseconds.
    max_delay_ms: u64,
    /// Maximum number of reconnection attempts (0 = unlimited).
    max_attempts: u32,
}

impl ReconnectPolicy {
    /// Create a new reconnection policy with sensible defaults:
    /// - Base delay: 1 second
    /// - Max delay: 60 seconds
    /// - Max attempts: unlimited
    pub fn new() -> Self {
        Self {
            attempt: AtomicU32::new(0),
            base_delay_ms: 1_000,
            max_delay_ms: 60_000,
            max_attempts: 0,
        }
    }

    /// Create a reconnection policy with custom parameters.
    pub fn with_params(base_delay_ms: u64, max_delay_ms: u64, max_attempts: u32) -> Self {
        Self {
            attempt: AtomicU32::new(0),
            base_delay_ms,
            max_delay_ms,
            max_attempts,
        }
    }

    /// Calculate the next delay duration using exponential backoff with jitter.
    ///
    /// The formula is: min(base * 2^attempt + jitter, max_delay)
    /// where jitter is a random value between 0 and base_delay.
    pub fn next_delay(&self) -> Duration {
        let attempt = self.attempt.fetch_add(1, Ordering::SeqCst);

        // Calculate exponential backoff
        let exp_delay = self.base_delay_ms.saturating_mul(1u64 << attempt.min(20));

        // Add jitter: random value between 0 and base_delay_ms
        let jitter = self.pseudo_random_jitter(attempt);

        // Cap at max_delay_ms
        let total_ms = exp_delay.saturating_add(jitter).min(self.max_delay_ms);

        tracing::debug!(
            attempt = attempt,
            delay_ms = total_ms,
            "Calculated reconnection delay"
        );

        Duration::from_millis(total_ms)
    }

    /// Reset the attempt counter after a successful connection.
    pub fn reset(&self) {
        self.attempt.store(0, Ordering::SeqCst);
        tracing::debug!("Reconnection policy reset");
    }

    /// Check if we've exceeded the maximum number of reconnection attempts.
    pub fn should_stop(&self) -> bool {
        if self.max_attempts == 0 {
            return false; // Unlimited
        }
        self.attempt.load(Ordering::SeqCst) >= self.max_attempts
    }

    /// Get the current attempt number.
    pub fn current_attempt(&self) -> u32 {
        self.attempt.load(Ordering::SeqCst)
    }

    /// Simple pseudo-random jitter using the attempt number as a seed.
    /// Not cryptographically secure, but fine for jitter purposes.
    fn pseudo_random_jitter(&self, seed: u32) -> u64 {
        // Simple LCG-style pseudo-random for jitter
        let hash = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (hash as u64 % self.base_delay_ms).min(self.base_delay_ms)
    }
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exponential_backoff() {
        let policy = ReconnectPolicy::with_params(1000, 60000, 0);

        let d1 = policy.next_delay();
        let d2 = policy.next_delay();
        let d3 = policy.next_delay();

        // Each delay should generally be larger than the previous
        // (accounting for jitter, the base doubles each time)
        assert!(d1.as_millis() >= 1000);
        assert!(d2.as_millis() >= 2000);
        assert!(d3.as_millis() >= 4000);
    }

    #[test]
    fn test_max_delay_cap() {
        let policy = ReconnectPolicy::with_params(1000, 5000, 0);

        // Advance many attempts
        for _ in 0..20 {
            let d = policy.next_delay();
            assert!(d.as_millis() <= 5000);
        }
    }

    #[test]
    fn test_reset() {
        let policy = ReconnectPolicy::new();
        policy.next_delay();
        policy.next_delay();
        assert_eq!(policy.current_attempt(), 2);

        policy.reset();
        assert_eq!(policy.current_attempt(), 0);
    }

    #[test]
    fn test_should_stop() {
        let policy = ReconnectPolicy::with_params(1000, 60000, 3);

        assert!(!policy.should_stop());
        policy.next_delay();
        policy.next_delay();
        policy.next_delay();
        assert!(policy.should_stop());
    }
}
