//! Desktop notifications for newly confirmed deals (DECISIONS.md 0019,
//! 0080). The daemon raises them as it confirms deals, so they arrive
//! with the app closed. Nothing leaves the machine: notices go to the
//! desktop's notification server over the session D-Bus.

use std::sync::Mutex;

use chrono::{Datelike, NaiveDate};
use tm_domain::{Deal, Route};

/// One desktop notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// Where notices go. `show` blocks; the worker calls it off the
/// runtime's threads.
pub trait Notifier: Send + Sync + std::fmt::Debug {
    fn show(&self, notice: &Notice) -> anyhow::Result<()>;
}

/// The desktop's notification server.
#[derive(Debug, Default)]
pub struct Desktop;

impl Notifier for Desktop {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn show(&self, notice: &Notice) -> anyhow::Result<()> {
        notify_rust::Notification::new()
            .appname("travel monitor")
            .summary(&notice.title)
            .body(&notice.body)
            .show()?;
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn show(&self, notice: &Notice) -> anyhow::Result<()> {
        tracing::info!("deal: {} — {}", notice.title, notice.body);
        Ok(())
    }
}

/// Keeps notices instead of showing them (tests).
#[derive(Debug, Default)]
pub struct Recorder {
    pub shown: Mutex<Vec<Notice>>,
}

impl Recorder {
    pub fn shown(&self) -> Vec<Notice> {
        self.shown.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Notifier for Recorder {
    fn show(&self, notice: &Notice) -> anyhow::Result<()> {
        self.shown
            .lock()
            .map_err(|_| anyhow::anyhow!("the recorder is poisoned"))?
            .push(notice.clone());
        Ok(())
    }
}

/// `SAO → LIS: R$3,932 (−19%)` over
/// `Lisbon · Economy · Nov 10 – 17 · TAP · via Kiwi.com`.
pub fn notice(deal: &Deal, route: &Route) -> Notice {
    let title = format!(
        "{} → {}: {}{}",
        route.origin.code,
        route.destination.code,
        deal.effective_brl,
        deal.change
            .map(|c| format!(" ({})", change(c)))
            .unwrap_or_default()
    );
    let body = [
        Some(route.destination.name.clone()),
        Some(deal.cabin.label().to_owned()),
        Some(date_range(deal.departure, deal.return_date)),
        deal.carrier.clone(),
        deal.seller.clone().map(|s| format!("via {s}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    Notice { title, body }
}

/// `−19%`, `+4%`, `0%`.
fn change(change: f64) -> String {
    let pct = (change * 100.0).round() as i64;
    match pct.signum() {
        -1 => format!("\u{2212}{}%", pct.abs()),
        1 => format!("+{pct}%"),
        _ => "0%".to_owned(),
    }
}

/// `Nov 10 – 17`, or `Nov 28 – Dec 5` across months.
fn date_range(departure: NaiveDate, return_date: NaiveDate) -> String {
    let short = |d: NaiveDate| d.format("%b %-d").to_string();
    if (departure.year(), departure.month())
        == (return_date.year(), return_date.month())
    {
        format!("{} – {}", short(departure), return_date.day())
    } else {
        format!("{} – {}", short(departure), short(return_date))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).expect("date")
    }

    #[test]
    fn changes_are_whole_percentages() {
        assert_eq!(change(-0.194), "\u{2212}19%");
        assert_eq!(change(0.04), "+4%");
        assert_eq!(change(0.001), "0%");
    }

    #[test]
    fn date_ranges_name_the_month_once_when_they_can() {
        assert_eq!(date_range(d(11, 10), d(11, 17)), "Nov 10 – 17");
        assert_eq!(date_range(d(11, 28), d(12, 5)), "Nov 28 – Dec 5");
    }
}
