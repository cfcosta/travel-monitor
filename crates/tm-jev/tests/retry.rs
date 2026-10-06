//! Backoff and `retry-after` parsing, as properties. Adapted from
//! `tau-ai`'s retry tests, without the OpenAI error classification.

use std::time::Duration;

use hegel::{Generator, PrintableGenerator, TestCase, generators as gs};
use tm_jev::retry::{RetryPolicy, parse_retry_after};

/// A [`RetryPolicy`] with small, valid bounds: `base` in `1ms..=10s`,
/// `max_delay >= base` (within ten seconds of it, so shrunk failures stay
/// short), and `max_attempts` in `0..=10`.
#[hegel::composite]
fn retry_policy(tc: &TestCase) -> RetryPolicy {
    let base_ms = tc.draw(gs::integers::<u64>().min_value(1).max_value(10_000));
    let extra_ms =
        tc.draw(gs::integers::<u64>().min_value(0).max_value(10_000));
    RetryPolicy {
        max_attempts: tc.draw(gs::integers::<u32>().max_value(10)),
        base: Duration::from_millis(base_ms),
        max_delay: Duration::from_millis(base_ms + extra_ms),
    }
}

fn unit_jitter() -> impl PrintableGenerator<f64> {
    gs::floats::<f64>()
        .min_value(0.0)
        .max_value(1.0)
        .allow_nan(false)
        .allow_infinity(false)
}

/// Full jitter, against a reference in integer nanoseconds: the delay is
/// `jitter × min(max_delay, base × 2^(attempt−1))`, to within the
/// rounding of one `f64` multiplication.
#[hegel::test(test_cases = 300)]
fn delay_matches_full_jitter_reference(tc: TestCase) {
    let policy = tc.draw(retry_policy().print_as_debug());
    let attempt = tc.draw(gs::integers::<u32>().min_value(1).max_value(80));
    let jitter =
        tc.draw(gs::floats::<f64>().min_value(0.0).max_value_exclusive(1.0));
    let base = policy.base.as_nanos();
    let cap = (0..attempt - 1)
        .fold(base, |d, _| d.saturating_mul(2).min(u128::MAX / 4))
        .min(policy.max_delay.as_nanos());
    let expected = cap as f64 * jitter;
    let got = policy.delay(attempt, jitter).as_nanos() as f64;
    tc.note(&format!("cap = {cap} ns, expected = {expected} ns"));
    assert!((got - expected).abs() <= 1_000.0, "{got} vs {expected}");
}

/// The delay never exceeds `max_delay`, and never panics however large
/// `attempt` is or whatever the jitter, out of range or NaN included.
#[hegel::test(test_cases = 300)]
fn delay_is_bounded(tc: TestCase) {
    let policy = tc.draw(retry_policy().print_as_debug());
    let attempt =
        tc.draw(gs::integers::<u32>().min_value(1).max_value(u32::MAX));
    let jitter = tc.draw(gs::floats::<f64>());
    assert!(policy.delay(attempt, jitter) <= policy.max_delay);
}

/// The upper bound (full jitter at `jitter = 1.0`) never shrinks as
/// `attempt` grows.
#[hegel::test(test_cases = 300)]
fn upper_bound_is_monotone_in_attempt(tc: TestCase) {
    let policy = tc.draw(retry_policy().print_as_debug());
    let attempt =
        tc.draw(gs::integers::<u32>().min_value(1).max_value(u32::MAX - 1));
    assert!(policy.delay(attempt + 1, 1.0) >= policy.delay(attempt, 1.0));
}

/// A loop that keeps asking `allows` and counting attempts stops after
/// exactly `max_attempts` of them.
#[hegel::test(test_cases = 300)]
fn allows_permits_exactly_max_attempts(tc: TestCase) {
    let policy = tc.draw(retry_policy().print_as_debug());
    // Bounded, so a policy that allows everything fails instead of hanging.
    let count = (0..=policy.max_attempts + 1)
        .take_while(|&attempts_made| policy.allows(attempts_made))
        .count();
    assert_eq!(count, policy.max_attempts as usize);
}

/// A server's `retry-after` hint is the least the backoff waits, and
/// `max_delay` the most; with no hint, it is the plain delay.
#[hegel::test(test_cases = 300)]
fn a_hint_bounds_the_backoff_from_below(tc: TestCase) {
    let policy = tc.draw(retry_policy().print_as_debug());
    let attempt = tc.draw(gs::integers::<u32>().min_value(1).max_value(1000));
    let jitter = tc.draw(unit_jitter());
    let hint = Duration::from_millis(
        tc.draw(gs::integers::<u64>().max_value(120_000)),
    );
    let plain = policy.delay(attempt, jitter);
    assert_eq!(
        policy.delay_with_hint(attempt, jitter, Some(hint)),
        plain.max(hint).min(policy.max_delay)
    );
    assert_eq!(policy.delay_with_hint(attempt, jitter, None), plain);
}

/// A `retry-after` value: a near miss of a plain integer (a sign, a
/// fraction, too many digits, whitespace around it) or arbitrary text.
#[hegel::composite]
fn retry_after_value(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::from_regex(r"[ \t]{0,2}[+-]?[0-9]{0,21}(\.[0-9]{0,2})?[ \t]{0,2}")
            .fullmatch(true),
        gs::text().max_size(16),
    ))
}

/// A value is a whole number of seconds after trimming, or nothing: a
/// negative or fractional number, one past `u64::MAX`, blank or other
/// text (an HTTP-date included) is not supported.
#[hegel::test(test_cases = 300)]
fn parse_retry_after_is_a_trimmed_whole_number(tc: TestCase) {
    let value = tc.draw(retry_after_value());
    let expected = value.trim().parse::<u64>().ok().map(Duration::from_secs);
    assert_eq!(parse_retry_after(&value), expected);
}

/// The values a server is likely to send, by hand.
#[test]
fn parse_retry_after_examples() {
    let cases = [
        ("0", Some(Duration::ZERO)),
        (" 5\t", Some(Duration::from_secs(5))),
        ("+5", Some(Duration::from_secs(5))),
        ("-1", None),
        ("1.5", None),
        ("", None),
        ("18446744073709551616", None),
        ("Wed, 21 Oct 2026 07:28:00 GMT", None),
    ];
    for (value, expected) in cases {
        assert_eq!(parse_retry_after(value), expected, "{value:?}");
    }
}

/// The default policy is `tau_ai`'s: 3 attempts, 2 s base, 60 s cap.
#[test]
fn the_default_policy() {
    assert_eq!(
        RetryPolicy::default(),
        RetryPolicy {
            max_attempts: 3,
            base: Duration::from_secs(2),
            max_delay: Duration::from_secs(60),
        }
    );
}
