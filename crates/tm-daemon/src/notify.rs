//! Desktop notifications for newly confirmed deals (DECISIONS.md 0019,
//! 0080). The daemon raises them as it confirms deals, so they arrive
//! with the app closed. Nothing leaves the machine: notices go to the
//! desktop's notification server over the session D-Bus.

use std::sync::Mutex;

use chrono::{Datelike, NaiveDate};
use tm_domain::{Deal, Leg, MilesUnit, Money, Route, program_name};
use tm_store::ExploreDrop;

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
/// `Lisbon · Economy · Nov 10 – 17 · TAP · via Kiwi.com`; an award deal
/// as [`award_notice`] words it, without a valuation.
pub fn notice(deal: &Deal, route: &Route) -> Notice {
    if deal.series.is_award() {
        return award_notice(deal, route, None);
    }
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
        Some(match deal.return_date {
            Some(r) => date_range(deal.departure, r),
            None => short(deal.departure),
        }),
        deal.carrier.clone(),
        deal.seller.clone().map(|s| format!("via {s}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    Notice { title, body }
}

/// A miles deal (0083), in the series' own format, its change against
/// that series' usual price, "one way" for a leg:
/// `SAO → LIS one way: 263,848 mi + R$69 (−22%)` over
/// `Lisbon · Business · Tue Nov 10 · LATAM Pass · LA8148 nonstop · 2
/// seats left · cash R$7,667`. A return leg's title runs the other
/// way. Seats only when 3 or fewer; the cash fare of the same dates
/// closes the body, for scale. With the user's value of 1,000 miles,
/// `≈R$1,346 at R$20/k` is appended.
pub fn award_notice(
    deal: &Deal,
    route: &Route,
    value_per_1000: Option<&Money>,
) -> Notice {
    let program = deal.series.program().unwrap_or_default();
    let unit = MilesUnit::of(program);
    let (from, to) = match deal.leg {
        Some(Leg::Return) => (&route.destination.code, &route.origin.code),
        _ => (&route.origin.code, &route.destination.code),
    };
    let price = deal
        .award_price()
        .map_or_else(|| deal.price.to_string(), |a| a.text(unit));
    let title = format!(
        "{from} → {to}{}: {price}{}",
        if deal.leg.is_some() { " one way" } else { "" },
        deal.change
            .map(|c| format!(" ({})", change(c)))
            .unwrap_or_default()
    );
    let when = match deal.return_date {
        Some(r) => date_range(deal.departure, r),
        None => deal.departure.format("%a %b %-d").to_string(),
    };
    let valued = value_per_1000.and_then(|v| {
        let cost = tm_scheduler::awards::cost_at(&deal.award_price()?, v)?;
        Some(format!(
            "≈{cost} at {}",
            tm_domain::format_mile_value(v.as_f64())
        ))
    });
    let body = [
        Some(route.destination.name.clone()),
        Some(deal.cabin.label().to_owned()),
        Some(when),
        Some(program_name(program).to_owned()),
        deal.carrier.clone(),
        deal.seats.filter(|s| s.few()).map(|s| s.text()),
        deal.cash.as_ref().map(|c| format!("cash {c}")),
        valued,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    Notice { title, body }
}

/// An unwatched destination that Explore priced well under its own
/// Explore history (0087):
/// `Found by Explore: SAO → CGK, R$5,207 (−24%)` over
/// `Jakarta · Economy · Nov 3 – 10 · Garuda Indonesia, 1 stop ·
/// usually R$6,850 · not on the watchlist`.
pub fn explore_notice(drop: &ExploreDrop, origin: &str) -> Notice {
    let title = format!(
        "Found by Explore: {origin} → {}, {}{}",
        drop.destination_airport,
        drop.price,
        drop.change()
            .map(|c| format!(" ({})", change(c)))
            .unwrap_or_default()
    );
    let airline = drop.carrier.clone().map(|c| match drop.stops {
        Some(0) => format!("{c}, nonstop"),
        Some(1) => format!("{c}, 1 stop"),
        Some(n) => format!("{c}, {n} stops"),
        None => c,
    });
    let body = [
        drop.destination_name.clone(),
        Some(drop.cabin.label().to_owned()),
        match (drop.departure, drop.return_date) {
            (Some(d), Some(r)) => Some(date_range(d, r)),
            (Some(d), None) => Some(short(d)),
            _ => None,
        },
        airline,
        Some(format!("usually {}", drop.usual)),
        Some("not on the watchlist".to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    Notice { title, body }
}

/// A miles program whose session ended (0024, 0083), once per expiry:
/// checks run while the app is closed, so the daemon says so itself.
pub fn login_notice(program: &str) -> Notice {
    Notice {
        title: format!("{}: log in again", program_name(program)),
        body:
            "Award checks are paused until you log in. Open the app's Settings."
                .to_owned(),
    }
}

/// `Nov 10`.
fn short(date: NaiveDate) -> String {
    date.format("%b %-d").to_string()
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

    fn route() -> Route {
        let place = |id, code: &str, name: &str| tm_domain::Place {
            id: tm_domain::PlaceId(id),
            code: code.into(),
            name: name.into(),
            country: tm_domain::CountryCode::new("BR"),
            kind: tm_domain::PlaceKind::Airport,
            lat: 0.0,
            lon: 0.0,
        };
        Route {
            id: tm_domain::RouteId(1),
            origin: place(1, "SAO", "São Paulo"),
            destination: place(2, "LIS", "Lisbon"),
            weight: 1.0,
            active: true,
        }
    }

    fn award_deal() -> Deal {
        use tm_domain::{
            Cabin,
            Currency,
            DealReason,
            DealStatus,
            PosId,
            Seats,
        };
        let taxes = Money::new(6861, Currency::brl());
        Deal {
            id: None,
            route: tm_domain::RouteId(1),
            cabin: Cabin::Business,
            pos: PosId(1),
            series: tm_domain::Series::Award {
                program: "latam_pass".into(),
            },
            leg: Some(Leg::Outbound),
            departure: d(11, 10),
            return_date: None,
            price: taxes.clone(),
            effective_brl: taxes,
            baseline: None,
            miles: Some(263_848),
            baseline_miles: Some(338_000),
            brand: Some("PREMIUM BUSINESS STANDARD".into()),
            seats: Some(Seats {
                count: 2,
                low: false,
            }),
            cash: Some(Money::brl_whole(7_667)),
            reason: DealReason::BelowMedian,
            change: Some(-0.22),
            estimated: false,
            status: DealStatus::Confirmed,
            found_at: chrono::DateTime::UNIX_EPOCH,
            confirmed_at: None,
            seen: false,
            carrier: Some("LA8148 nonstop".into()),
            seller: None,
            sellers: Vec::new(),
            url: None,
        }
    }

    /// The Status board's wording for a one-way miles deal.
    #[test]
    fn award_deals_read_in_miles() {
        let deal = award_deal();
        assert_eq!(
            notice(&deal, &route()),
            Notice {
                title: "SAO → LIS one way: 263,848 mi + R$69 (\u{2212}22%)".into(),
                body: "Lisbon · Business · Tue Nov 10 · LATAM Pass · LA8148 nonstop · 2 seats left · cash R$7,667".into(),
            }
        );
        let valued = award_notice(&deal, &route(), Some(&Money::brl_whole(20)));
        assert!(
            valued.body.ends_with(" · ≈R$5,346 at R$20/k"),
            "{}",
            valued.body
        );
        // The return leg runs the other way; plenty of seats go unsaid.
        let back = Deal {
            leg: Some(Leg::Return),
            seats: Some(tm_domain::Seats {
                count: 9,
                low: false,
            }),
            ..deal.clone()
        };
        let n = notice(&back, &route());
        assert!(n.title.starts_with("LIS → SAO one way: "), "{}", n.title);
        assert!(!n.body.contains("seats"));
        // A round-trip award names both dates.
        let trip = Deal {
            leg: None,
            return_date: Some(d(11, 17)),
            ..deal
        };
        let n = notice(&trip, &route());
        assert!(n.title.starts_with("SAO → LIS: 263,848 mi"), "{}", n.title);
        assert!(n.body.contains("Nov 10 – 17"));
    }

    #[test]
    fn an_explore_find_says_it_is_not_watched() {
        let drop = ExploreDrop {
            id: 1,
            origin: tm_domain::PlaceId(1),
            cabin: tm_domain::Cabin::Economy,
            destination_freebase_id: "/m/044rv".into(),
            destination_airport: "CGK".into(),
            destination_name: Some("Jakarta".into()),
            price: Money::brl_whole(5_207),
            usual: Money::brl_whole(6_850),
            departure: Some(d(11, 3)),
            return_date: Some(d(11, 10)),
            carrier: Some("Garuda Indonesia".into()),
            stops: Some(1),
            found_at: chrono::Utc::now(),
            notified_at: None,
        };
        let n = explore_notice(&drop, "SAO");
        assert_eq!(
            n.title,
            "Found by Explore: SAO → CGK, R$5,207 (\u{2212}24%)"
        );
        assert_eq!(
            n.body,
            "Jakarta · Economy · Nov 3 – 10 · Garuda Indonesia, 1 stop · \
             usually R$6,850 · not on the watchlist"
        );
    }

    #[test]
    fn an_ended_session_asks_for_a_login() {
        let n = login_notice("latam_pass");
        assert_eq!(n.title, "LATAM Pass: log in again");
        assert!(n.body.contains("Settings"));
    }

    #[test]
    fn date_ranges_name_the_month_once_when_they_can() {
        assert_eq!(date_range(d(11, 10), d(11, 17)), "Nov 10 – 17");
        assert_eq!(date_range(d(11, 28), d(12, 5)), "Nov 28 – Dec 5");
    }
}
