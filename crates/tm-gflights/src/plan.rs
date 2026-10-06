//! How many price-graph pages cover a horizon. The first
//! `GetCalendarGraph` covers about 60 days, then each "Scroll forward"
//! adds 21 (research, 2026-10-05: 2026-11-03..2027-01-01, then
//! 2027-01-02..01-22, 01-23..02-12, …). Six months of departures is 7
//! pages: about 7 calls for a round trip and 14 for both one-way legs
//! (DECISIONS.md 0042).

use chrono::NaiveDate;

use crate::rpc::CalendarRequest;

pub const GRAPH_FIRST_PAGE_DAYS: u32 = 60;
pub const GRAPH_SCROLL_DAYS: u32 = 21;

/// Pages (the first load plus scrolls) whose departures cover `days`
/// days. Zero days need no page.
pub fn graph_pages(days: u32) -> u32 {
    match days {
        0 => 0,
        d if d <= GRAPH_FIRST_PAGE_DAYS => 1,
        d => 1 + (d - GRAPH_FIRST_PAGE_DAYS).div_ceil(GRAPH_SCROLL_DAYS),
    }
}

/// "Scroll forward" presses after the first page.
pub fn graph_scrolls(days: u32) -> u32 {
    graph_pages(days).saturating_sub(1)
}

/// Days of departures that `pages` pages cover.
pub fn graph_days(pages: u32) -> u32 {
    match pages {
        0 => 0,
        p => GRAPH_FIRST_PAGE_DAYS + (p - 1) * GRAPH_SCROLL_DAYS,
    }
}

/// The last departure date the captured requests cover, so the daemon
/// can stop scrolling once it reaches its horizon.
pub fn covered_until(requests: &[CalendarRequest]) -> Option<NaiveDate> {
    requests.iter().map(|r| r.departures.last).max()
}

/// Whether the captured requests reach `horizon_end`.
pub fn reaches(requests: &[CalendarRequest], horizon_end: NaiveDate) -> bool {
    covered_until(requests).is_some_and(|last| last >= horizon_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_months_is_seven_pages() {
        assert_eq!(graph_pages(183), 7);
        assert_eq!(graph_scrolls(183), 6);
        assert_eq!(graph_pages(0), 0);
        assert_eq!(graph_pages(1), 1);
        assert_eq!(graph_pages(60), 1);
        assert_eq!(graph_pages(61), 2);
        assert_eq!(graph_pages(81), 2);
        assert_eq!(graph_pages(82), 3);
    }

    #[test]
    fn pages_cover_their_days() {
        for days in 0..=1000 {
            let p = graph_pages(days);
            assert!(graph_days(p) >= days, "{days} days, {p} pages");
            if p > 0 {
                assert!(
                    graph_days(p - 1) < days,
                    "{days} days, {p} pages is not minimal"
                );
            }
        }
    }
}
