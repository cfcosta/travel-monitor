//! What changes between two snapshots, and when (DECISIONS.md 0057):
//! how long to wait after a change notice before refreshing, which rows
//! just arrived and how long they stay highlighted, and the wall clock
//! the relative times are read against.
//!
//! Everything here is plain data and arithmetic; the window drives it
//! with gpui timers.

use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use tm_domain::DealId;

use crate::data::{RowKey, Snapshot};

/// Coalesces a burst of change notices into one refresh: wait `quiet`
/// after a notice, again while notices keep coming, but never more than
/// `max_wait` in all, so a daemon writing all the time still shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Debounce {
    pub quiet: Duration,
    pub max_wait: Duration,
}

impl Default for Debounce {
    fn default() -> Self {
        Self {
            quiet: Duration::from_millis(200),
            max_wait: Duration::from_secs(1),
        }
    }
}

impl Debounce {
    /// The first wait after a notice.
    pub fn first(&self) -> Duration {
        self.quiet.min(self.max_wait)
    }

    /// After waiting `waited` in all since the first notice, with `more`
    /// telling whether another notice came during the last wait: how
    /// long to wait again, or `None` to refresh now.
    pub fn next(&self, waited: Duration, more: bool) -> Option<Duration> {
        if !more || waited >= self.max_wait {
            return None;
        }
        Some(self.quiet.min(self.max_wait - waited))
    }

    /// When refreshes happen for notices at `notices` (sorted offsets),
    /// as the window's loop would run them. For tests and reasoning.
    #[cfg(test)]
    pub fn simulate(&self, notices: &[Duration]) -> Vec<Duration> {
        let mut refreshes = Vec::new();
        let mut i = 0;
        while i < notices.len() {
            let start = notices[i];
            let mut now = start;
            let mut waited = Duration::ZERO;
            let mut wait = self.first();
            i += 1;
            loop {
                now += wait;
                waited += wait;
                let before = i;
                while i < notices.len() && notices[i] <= now {
                    i += 1;
                }
                match self.next(waited, i > before) {
                    Some(w) => wait = w,
                    None => break,
                }
            }
            refreshes.push(now);
            // Notices during the refresh's own wait were folded in.
        }
        refreshes
    }
}

/// How long a row that just arrived stays highlighted.
pub const FRESH_FOR: Duration = Duration::from_secs(2);

/// Something that just arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fresh {
    /// A query landed in the log, or its result did.
    Query(i64),
    /// A deal showed up in the list.
    Deal(DealId),
    /// New observations for the route open in the detail.
    Route(RowKey),
}

/// What is new in `new` compared with `old`. Rows only count as new
/// when both snapshots show the same thing (same origin, same route
/// open), so switching views highlights nothing.
pub fn arrivals(old: &Snapshot, new: &Snapshot) -> Vec<Fresh> {
    let mut fresh = Vec::new();
    let finished: HashSet<i64> = old
        .activity
        .iter()
        .chain(old.detail.iter().flat_map(|d| d.recent_checks.iter()))
        .filter(|a| !a.running)
        .map(|a| a.id)
        .collect();
    let mut seen = HashSet::new();
    for a in new
        .activity
        .iter()
        .chain(new.detail.iter().flat_map(|d| d.recent_checks.iter()))
    {
        // A running query shows as new when it lands, not when it starts
        // (the "Now checking" row covers that).
        if !a.running && !finished.contains(&a.id) && seen.insert(a.id) {
            fresh.push(Fresh::Query(a.id));
        }
    }
    if old.origin == new.origin {
        let deals: HashSet<DealId> =
            old.rows.iter().filter_map(|r| r.deal).collect();
        fresh.extend(
            new.rows
                .iter()
                .filter(|r| r.status.is_deal())
                .filter_map(|r| r.deal)
                .filter(|d| !deals.contains(d))
                .map(Fresh::Deal),
        );
    }
    if let (Some(o), Some(n)) = (&old.detail, &new.detail)
        && o.key == n.key
        && let (Some(was), Some(is)) = (old.row(o.key), new.row(n.key))
        && is.last_checked > was.last_checked
    {
        fresh.push(Fresh::Route(n.key));
    }
    fresh
}

/// Highlights that fade out over `ttl`; marking a key again restarts
/// its fade.
#[derive(Debug, Clone)]
pub struct Highlights<K> {
    since: HashMap<K, Instant>,
    pub ttl: Duration,
}

impl<K> Default for Highlights<K> {
    fn default() -> Self {
        Self {
            since: HashMap::new(),
            ttl: FRESH_FOR,
        }
    }
}

impl<K: Eq + Hash> Highlights<K> {
    pub fn mark(&mut self, keys: impl IntoIterator<Item = K>, now: Instant) {
        for key in keys {
            self.since.insert(key, now);
        }
    }

    /// How strong the highlight is: 1 when it starts, falling to 0 when
    /// it is over (and for keys never marked).
    pub fn strength(&self, key: &K, now: Instant) -> f32 {
        self.since.get(key).map_or(0.0, |at| {
            let t = now.saturating_duration_since(*at).as_secs_f32()
                / self.ttl.as_secs_f32();
            (1.0 - t).clamp(0.0, 1.0)
        })
    }

    /// Forgets the highlights that are over; whether any was.
    pub fn prune(&mut self, now: Instant) -> bool {
        let before = self.since.len();
        let ttl = self.ttl;
        self.since
            .retain(|_, at| now.saturating_duration_since(*at) < ttl);
        self.since.len() != before
    }

    pub fn is_empty(&self) -> bool {
        self.since.is_empty()
    }
}

/// Where the window's wall clock comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    System,
    /// Starts at this time and follows the executor's clock, so tests
    /// move it with `advance_clock`.
    #[cfg_attr(not(test), expect(dead_code, reason = "tests and screenshots"))]
    From(DateTime<Utc>),
}

impl Clock {
    /// The time now, `elapsed` after the window opened.
    pub fn read(self, elapsed: Duration) -> DateTime<Utc> {
        match self {
            Clock::System => Utc::now(),
            Clock::From(start) => {
                start + chrono::TimeDelta::from_std(elapsed).unwrap_or_default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn one_notice_refreshes_once_after_the_quiet_time() {
        let d = Debounce::default();
        assert_eq!(d.simulate(&[ms(0)]), vec![ms(200)]);
        assert_eq!(d.next(ms(200), false), None);
    }

    #[test]
    fn a_burst_is_one_refresh() {
        let d = Debounce::default();
        // A check's end: finish_query, observations, family state, the
        // heartbeat, a few milliseconds apart.
        let burst: Vec<Duration> = (0..8).map(|i| ms(i * 20)).collect();
        assert_eq!(d.simulate(&burst), vec![ms(400)]);
    }

    #[test]
    fn notices_far_apart_refresh_each() {
        let d = Debounce::default();
        assert_eq!(
            d.simulate(&[ms(0), ms(1000), ms(5000)]),
            vec![ms(200), ms(1200), ms(5200)]
        );
    }

    #[test]
    fn a_steady_stream_still_refreshes_every_max_wait() {
        let d = Debounce::default();
        let stream: Vec<Duration> = (0..300).map(|i| ms(i * 10)).collect();
        let refreshes = d.simulate(&stream);
        assert!(refreshes.len() >= 2, "{refreshes:?}");
        for pair in refreshes.windows(2) {
            assert!(pair[1] - pair[0] <= ms(1300), "{refreshes:?}");
        }
        assert_eq!(refreshes[0], ms(1000));
        assert_eq!(d.next(ms(1000), true), None);
        assert_eq!(d.next(ms(900), true), Some(ms(100)));
    }

    #[test]
    fn highlights_fade_then_expire() {
        let t0 = Instant::now();
        let mut h = Highlights::default();
        h.mark([Fresh::Query(1)], t0);
        assert_eq!(h.strength(&Fresh::Query(1), t0), 1.0);
        assert_eq!(h.strength(&Fresh::Query(2), t0), 0.0);
        let half = h.strength(&Fresh::Query(1), t0 + ms(1000));
        assert!((half - 0.5).abs() < 1e-3, "{half}");
        assert!(h.strength(&Fresh::Query(1), t0 + ms(1999)) > 0.0);
        assert_eq!(h.strength(&Fresh::Query(1), t0 + FRESH_FOR), 0.0);
        assert_eq!(h.strength(&Fresh::Query(1), t0 + ms(3000)), 0.0);
        assert!(!h.prune(t0 + ms(1500)));
        assert!(h.prune(t0 + ms(2500)));
        assert!(h.is_empty());
    }

    #[test]
    fn a_new_highlight_restarts_the_fade() {
        let t0 = Instant::now();
        let mut h = Highlights::default();
        h.mark([Fresh::Query(1)], t0);
        h.mark([Fresh::Query(1)], t0 + ms(1500));
        let left = h.strength(&Fresh::Query(1), t0 + ms(3000));
        assert!((left - 0.25).abs() < 1e-3, "{left}");
    }

    #[test]
    fn a_fixed_clock_follows_the_executor() {
        let start =
            DateTime::<Utc>::from_timestamp(1_800_000_000, 0).expect("a time");
        let clock = Clock::From(start);
        assert_eq!(clock.read(Duration::ZERO), start);
        assert_eq!(
            clock.read(Duration::from_secs(61)),
            start + chrono::TimeDelta::seconds(61)
        );
    }
}
