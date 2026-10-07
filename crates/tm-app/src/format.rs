//! How numbers, prices and dates read on screen.

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use tm_domain::{AwardPrice, MilesUnit, Money, Seats};

/// The typographic minus the designs use (`−19%`).
pub const MINUS: char = '\u{2212}';

/// `R$3,932` (whole units, as Google shows them).
/// `4,800`: a count with thousands separators.
pub fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

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

/// How long something has run: `12 s`, `1 min 5 s`, `14 min`, `2 h`.
pub fn elapsed(now: DateTime<Utc>, since: DateTime<Utc>) -> String {
    let secs = (now - since).num_seconds().max(0);
    match secs {
        0..60 => format!("{secs} s"),
        60..600 => match secs % 60 {
            0 => format!("{} min", secs / 60),
            s => format!("{} min {s} s", secs / 60),
        },
        600..3600 => format!("{} min", secs / 60),
        _ => format!("{} h", secs / 3600),
    }
}

/// A countdown to `then`: `45 s`, `3 min`, `~2 h`, `now` once due.
pub fn countdown(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let secs = (then - now).num_seconds();
    match secs {
        ..1 => "now".to_owned(),
        1..60 => format!("{secs} s"),
        60..3600 => format!("{} min", (secs + 30) / 60),
        _ => until(now, then),
    }
}

/// Local wall-clock time, `12:04`.
pub fn clock(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M").to_string()
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

/// A price per kilometre in BRL: `R$0.31/km`.
pub fn per_km(brl: f64) -> String {
    format!("R${brl:.2}/km")
}

// Awards (0083): miles follow the app's English formatting, not the
// site's pt-BR `63.848 milhas`.

/// `63,848`.
pub fn miles(n: i64) -> String {
    tm_domain::format_miles(n)
}

/// `264k`, for map labels and the rail.
pub fn miles_short(n: i64) -> String {
    tm_domain::format_miles_short(n)
}

/// `R$28.80/k`.
pub fn mile_value(brl_per_1000: f64) -> String {
    tm_domain::format_mile_value(brl_per_1000)
}

/// `R$28.80 per 1,000 mi`, the long form.
pub fn mile_value_long(brl_per_1000: f64, unit: MilesUnit) -> String {
    let short = mile_value(brl_per_1000);
    format!(
        "{} per 1,000 {}",
        short.trim_end_matches("/k"),
        unit.label()
    )
}

/// `≈R$1,346 at R$20/k`: an award's miles at a value per 1,000, plus
/// its taxes. The only figure that adds miles and reais, always
/// labelled.
pub fn valued(price: &AwardPrice, per_1000: &Money) -> Option<String> {
    let cost = tm_scheduler::awards::cost_at(price, per_1000)?;
    Some(format!(
        "≈{} at {}",
        money(&cost),
        mile_value(per_1000.as_f64())
    ))
}

/// `2 seats left`, only when 3 or fewer (or flagged few).
pub fn seats(seats: Option<Seats>) -> Option<String> {
    seats.filter(Seats::few).map(|s| s.text())
}

/// Miles per kilometre: `8.1 mi/km`.
pub fn miles_per_km(miles: f64, unit: MilesUnit) -> String {
    format!("{miles:.1} {}/km", unit.label())
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

    /// The one-second clock moves these without a new snapshot (0057).
    #[test]
    fn relative_times_follow_the_clock() {
        let start = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let tick = |s: i64| start + chrono::Duration::seconds(s);
        let beat = start - chrono::Duration::seconds(50);
        let next = start + chrono::Duration::seconds(75);
        let seen: Vec<(String, String, String)> = [0, 9, 10, 15, 74, 75, 80]
            .into_iter()
            .map(|s| {
                (
                    ago(tick(s), beat),
                    elapsed(tick(s), start),
                    countdown(tick(s), next),
                )
            })
            .collect();
        let expect = |a: &str, e: &str, c: &str| {
            (a.to_owned(), e.to_owned(), c.to_owned())
        };
        assert_eq!(
            seen,
            vec![
                expect("just now", "0 s", "1 min"),
                expect("just now", "9 s", "1 min"),
                expect("1 min ago", "10 s", "1 min"),
                expect("1 min ago", "15 s", "1 min"),
                expect("2 min ago", "1 min 14 s", "1 s"),
                expect("2 min ago", "1 min 15 s", "now"),
                expect("2 min ago", "1 min 20 s", "now"),
            ]
        );
        assert_eq!(
            countdown(start, start + chrono::Duration::seconds(45)),
            "45 s"
        );
        assert_eq!(
            countdown(start, start + chrono::Duration::minutes(3)),
            "3 min"
        );
        assert_eq!(
            elapsed(start + chrono::Duration::minutes(14), start),
            "14 min"
        );
    }

    #[test]
    fn counts_have_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(300), "300");
        assert_eq!(thousands(4800), "4,800");
        assert_eq!(thousands(12_000), "12,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn awards_read_like_the_designs() {
        let award = AwardPrice::new(
            63_848,
            Money::new(6861, tm_domain::Currency::brl()),
        );
        assert_eq!(award.text(MilesUnit::Miles), "63,848 mi + R$69");
        assert_eq!(miles(263_848), "263,848");
        assert_eq!(miles_short(263_848), "264k");
        assert_eq!(mile_value(28.80), "R$28.80/k");
        assert_eq!(
            mile_value_long(28.80, MilesUnit::Miles),
            "R$28.80 per 1,000 mi"
        );
        assert_eq!(
            valued(&award, &Money::brl_whole(20)).as_deref(),
            Some("≈R$1,346 at R$20/k")
        );
        assert_eq!(
            seats(Some(Seats {
                count: 2,
                low: false
            }))
            .as_deref(),
            Some("2 seats left")
        );
        assert_eq!(
            seats(Some(Seats {
                count: 9,
                low: false
            })),
            None
        );
        assert_eq!(miles_per_km(8.06, MilesUnit::Miles), "8.1 mi/km");
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
