//! Bounded exponential backoff with full jitter, and `retry-after`
//! parsing, for [`TypeSafe`](crate::TypeSafe).
//!
//! The minimum of `tau_ai::retry` that `tau-jev` used: [`RetryPolicy`],
//! [`jitter`] and [`parse_retry_after`]. `tau_ai`'s classification of
//! OpenAI errors is left out; the client decides what to retry by status
//! alone (429, 503 and 529).
//!
//! Pure and deterministic apart from [`jitter`]: callers sample jitter
//! themselves and pass it in, so every delay is a plain computation.

use std::time::Duration;

/// A uniform sample in `[0, 1)` for [`RetryPolicy::delay`]'s jitter.
/// Each call seeds a fresh `RandomState`, so callers retrying together
/// spread out.
pub fn jitter() -> f64 {
    use std::hash::BuildHasher;
    let bits = std::collections::hash_map::RandomState::new().hash_one(0u8);
    (bits >> 11) as f64 / (1u64 << 53) as f64
}

/// Bounded exponential backoff with jitter.
///
/// `max_attempts` is the total number of attempts allowed (the initial
/// attempt counts as one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    /// 3 attempts, a 2 s base and a 60 s cap, as in `tau_ai`.
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base: Duration::from_secs(2),
            max_delay: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// The deterministic upper bound for `attempt` (1-based): `base *
    /// 2^(attempt - 1)`, capped at `max_delay`. Saturates instead of
    /// overflowing, and is cheap for any `attempt` up to `u32::MAX`:
    /// doubling a nonzero duration outgrows any `max_delay` representable
    /// in a `Duration` well within 64 steps, so the loop never needs more
    /// than that.
    fn cap_for_attempt(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1);
        let mut cap = self.base;
        for _ in 0..exponent.min(64) {
            if cap >= self.max_delay {
                return self.max_delay;
            }
            cap = cap.saturating_mul(2);
        }
        cap.min(self.max_delay)
    }

    /// Delay before retry number `attempt` (1-based), given a jitter
    /// sample in `[0, 1)`.
    ///
    /// "Full jitter": the delay is drawn uniformly from `[0, cap]`, where
    /// `cap` is [`Self::cap_for_attempt`]. `jitter` is clamped to `[0,
    /// 1]`, so a value out of range (or NaN, which clamps to NaN and
    /// scales to the cap) never panics or exceeds the cap.
    pub fn delay(&self, attempt: u32, jitter: f64) -> Duration {
        let cap = self.cap_for_attempt(attempt);
        scale_duration(cap, jitter.clamp(0.0, 1.0))
    }

    /// Whether another attempt is allowed after `attempts_made` attempts
    /// have already been made.
    pub fn allows(&self, attempts_made: u32) -> bool {
        attempts_made < self.max_attempts
    }

    /// Like [`Self::delay`], but at least a server-provided `retry-after`
    /// hint when present, and never past `max_delay`.
    pub fn delay_with_hint(
        &self,
        attempt: u32,
        jitter: f64,
        retry_after: Option<Duration>,
    ) -> Duration {
        let delay = self.delay(attempt, jitter);
        match retry_after {
            Some(hint) => delay.max(hint).min(self.max_delay),
            None => delay,
        }
    }
}

/// Scale a duration by a fraction in `[0, 1]`. A product that is not a
/// valid duration (too large, or NaN) comes back as the duration itself
/// rather than panicking.
fn scale_duration(duration: Duration, fraction: f64) -> Duration {
    Duration::try_from_secs_f64(duration.as_secs_f64() * fraction)
        .map_or(duration, |scaled| scaled.min(duration))
}

/// A `retry-after` header's value as a [`Duration`]: a whole number of
/// seconds, after trimming whitespace. The HTTP-date form needs a date
/// parser this crate does not carry, so it is `None`, as is any other
/// value; the caller then backs off on its own schedule.
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Jitter samples are uniform-looking draws from `[0, 1)`: every one
    /// in range, and not all the same.
    #[test]
    fn jitter_samples_the_unit_interval() {
        let samples: Vec<f64> = (0..200).map(|_| jitter()).collect();
        assert!(
            samples.iter().all(|x| (0.0..1.0).contains(x)),
            "{samples:?}"
        );
        assert!(samples.iter().any(|x| *x != samples[0]));
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        assert!((0.3..0.7).contains(&mean), "{mean}");
    }
}
