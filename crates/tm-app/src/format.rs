//! How numbers, prices and dates read on screen.

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use tm_domain::Money;

/// The typographic minus the designs use (`−19%`).
pub const MINUS: char = '\u{2212}';

/// `R$3,932` (whole units, as Google shows them).
pub fn money(price: &Money) -> String {
    price.to_string()
}

/// A change as a whole percentage: `−19%`, `+4%`, `0%`.
pub fn change(change: f64) -> String {
    let pct = (change * 100.0).round() as i64;
    match pct.signum() {
        -1 => format!("{MINUS}{}%", pct.abs()),
        1 => format!("+{pct}%"),
        _ => "0%".to_owned(),
    }
}

/// `3.5%` for a fraction (0.035); whole numbers lose the decimals.
pub fn percent(fraction: f64) -> String {
    let pct = fraction * 100.0;
    if (pct - pct.round()).abs() < 1e-9 {
        format!("{}%", pct.round() as i64)
    } else {
        let s = format!("{pct:.2}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        format!("{s}%")
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
    "Nov", "Dec",
];

pub fn month(date: NaiveDate) -> &'static str {
    MONTHS[date.month0() as usize]
}

/// `Nov 10`.
pub fn short_date(date: NaiveDate) -> String {
    format!("{} {}", month(date), date.day())
}

/// `Tue Nov 10`.
pub fn day_date(date: NaiveDate) -> String {
    format!("{} {}", date.format("%a"), short_date(date))
}

/// `Nov 10 – 17`, or `Oct 26 – Nov 2` across months.
pub fn date_range(departure: NaiveDate, return_date: NaiveDate) -> String {
    if departure.month() == return_date.month()
        && departure.year() == return_date.year()
    {
        format!("{} – {}", short_date(departure), return_date.day())
    } else {
        format!("{} – {}", short_date(departure), short_date(return_date))
    }
}

/// `just now`, `11 min ago`, `3 h ago`, `1 day ago`, `4 days ago`.
pub fn ago(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let secs = (now - then).num_seconds().max(0);
    match secs {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        86_400..172_800 => "1 day ago".to_owned(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// `in 4 min`, `in ~5 h`, `now`.
pub fn until(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let secs = (then - now).num_seconds();
    match secs {
        ..60 => "now".to_owned(),
        60..3600 => format!("{} min", secs / 60),
        _ => format!("~{} h", (secs + 1800) / 3600),
    }
}

/// Local wall-clock time, `12:04`.
pub fn clock(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M").to_string()
}

/// A compact price for grid cells: `3.93k`, `14.9k`, `916`.
pub fn kilo(price: &Money) -> String {
    let whole = price.whole();
    if whole.abs() >= 10_000 {
        format!("{:.1}k", whole as f64 / 1000.0)
    } else if whole.abs() >= 1000 {
        format!("{:.2}k", whole as f64 / 1000.0)
    } else {
        whole.to_string()
    }
}

/// `9 h 55 min`, `55 min`.
pub fn duration(minutes: u32) -> String {
    let (h, m) = (minutes / 60, minutes % 60);
    match (h, m) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// `nonstop`, `1 stop`, `2 stops`.
pub fn stops(stops: u8) -> String {
    match stops {
        0 => "nonstop".to_owned(),
        1 => "1 stop".to_owned(),
        n => format!("{n} stops"),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("date")
    }

    #[test]
    fn changes_use_the_typographic_minus() {
        assert_eq!(change(-0.19), "−19%");
        assert_eq!(change(0.04), "+4%");
        assert_eq!(change(0.001), "0%");
    }

    #[test]
    fn percentages() {
        assert_eq!(percent(0.035), "3.5%");
        assert_eq!(percent(0.01), "1%");
        assert_eq!(percent(0.2), "20%");
    }

    #[test]
    fn date_ranges_like_the_designs() {
        assert_eq!(date_range(d(2026, 11, 10), d(2026, 11, 17)), "Nov 10 – 17");
        assert_eq!(
            date_range(d(2026, 10, 26), d(2026, 11, 2)),
            "Oct 26 – Nov 2"
        );
        assert_eq!(day_date(d(2026, 11, 10)), "Tue Nov 10");
    }

    #[test]
    fn relative_times() {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let ago_by = |s: i64| ago(now, now - chrono::Duration::seconds(s));
        assert_eq!(ago_by(5), "just now");
        assert_eq!(ago_by(11 * 60), "11 min ago");
        assert_eq!(ago_by(3 * 3600 + 5), "3 h ago");
        assert_eq!(ago_by(90_000), "1 day ago");
        assert_eq!(ago_by(4 * 86_400), "4 days ago");
        assert_eq!(until(now, now + chrono::Duration::minutes(4)), "4 min");
        assert_eq!(until(now, now + chrono::Duration::hours(5)), "~5 h");
    }

    #[test]
    fn compact_prices() {
        assert_eq!(kilo(&Money::brl_whole(3932)), "3.93k");
        assert_eq!(kilo(&Money::brl_whole(14870)), "14.9k");
        assert_eq!(kilo(&Money::brl_whole(916)), "916");
    }

    #[test]
    fn durations_and_stops() {
        assert_eq!(duration(595), "9 h 55 min");
        assert_eq!(duration(55), "55 min");
        assert_eq!(duration(120), "2 h");
        assert_eq!(stops(0), "nonstop");
        assert_eq!(stops(2), "2 stops");
    }
}
