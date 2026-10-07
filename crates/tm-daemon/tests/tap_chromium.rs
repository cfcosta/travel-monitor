//! The TAP source in a real headless Chromium, without the internet
//! (DECISIONS.md 0051): a local server plays `booking.flytap.com` with
//! a tiny booking page. On load it POSTs the captured availability
//! search; its "Dates" step shows a month calendar whose months POST to
//! `bfm/rest/booking/availability/calendar/` as they scroll into view,
//! and get the captured November back, moved to the month asked for.
//!
//! Skipped (with a message) when `TM_CHROMIUM` is not set; the dev
//! shell sets it.

#![allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on: a test is the runtime's entry point"
)]

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use chrono::{NaiveDate, TimeDelta};
use tm_browser::ChromiumConfig;
use tm_daemon::{
    sites::tap,
    sources::{
        Sources,
        TapJob,
        browser::{BrowserSources, Timeouts},
    },
};
use tm_domain::{
    Cabin,
    CountryCode,
    Currency,
    Money,
    Passengers,
    PointOfSale,
    PosId,
    QueryOutcome,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tap")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The fake booking page: results first, then the "Dates" step with
/// twelve tall months, each fetched once it is in view.
fn booking_page() -> String {
    let request: serde_json::Value =
        serde_json::from_str(&fixture("availability-search.request.json"))
            .unwrap_or_default();
    let search_body = request["body"].to_string();
    format!(
        r##"<!doctype html><html><head><title>TAP Air Portugal</title>
<style>.month {{ height: 900px; border-top: 1px solid #999; }}</style></head>
<body>
<ol class="steps"><li><a href="#" id="dates">Dates</a></li><li>Flights</li><li>Passengers</li></ol>
<div id="results">loading</div>
<div id="calendar" style="display:none"></div>
<script>
const q = new URLSearchParams(location.search);
const [, m0, y0] = q.get('depDate').split('.').map(Number);
const base = {{cabinClass: 'E', destination: q.get('destination'), market: q.get('market'),
  origin: q.get('origin'), tripType: 'O', paxType: 'ADT'}};
fetch('/bfm/rest/booking/availability/search?payWithMiles=false&starAlliance=false',
  {{method: 'POST', headers: {{'Content-Type': 'application/json'}}, body: JSON.stringify({search_body})}})
  .then(r => r.text()).then(t => {{ document.getElementById('results').innerText = t.length + ' bytes'; }});
function month(i) {{
  const d = new Date(y0, m0 - 1 + i, 1);
  return {{month: d.getMonth() + 1, year: d.getFullYear()}};
}}
function showCalendar() {{
  document.getElementById('results').style.display = 'none';
  const cal = document.getElementById('calendar');
  cal.style.display = 'block';
  const seen = new IntersectionObserver(entries => {{
    for (const e of entries) {{
      if (!e.isIntersecting || e.target.dataset.loaded) continue;
      e.target.dataset.loaded = '1';
      const body = Object.assign({{}}, base, month(Number(e.target.dataset.i)));
      fetch('/bfm/rest/booking/availability/calendar/', {{method: 'POST',
        headers: {{'Content-Type': 'application/json'}}, body: JSON.stringify(body)}})
        .then(r => r.text()).then(t => {{ e.target.innerText = body.month + '/' + body.year + ': ' + t.length; }});
    }}
  }});
  for (let i = 0; i < 12; i++) {{
    const div = document.createElement('div');
    div.className = 'month';
    div.dataset.i = i;
    div.innerText = 'month ' + i;
    cal.appendChild(div);
    seen.observe(div);
  }}
}}
document.getElementById('dates').addEventListener('click', e => {{
  e.preventDefault();
  history.pushState({{}}, '', '/booking/dates');
  showCalendar();
}});
</script>
</body></html>"##
    )
}

const CHALLENGE: &str = r#"<!doctype html><html><head><title>Just a moment...</title></head>
<body><h1>booking.flytap.com</h1><p>Verify you are human by completing the action below.</p></body></html>"#;

/// The captured November calendar, moved to the month `body` asks for.
fn calendar_for(body: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let month = v["month"].as_u64().unwrap_or(11);
    let year = v["year"].as_u64().unwrap_or(2026);
    fixture("calendar.GRU-LIS.2026-11.oneway.json").replace(
        "\"departureDate\": \"2026-11-",
        &format!("\"departureDate\": \"{year}-{month:02}-"),
    )
}

async fn serve(mut stream: TcpStream) {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    let head_end = loop {
        let Ok(n) = stream.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let length: usize = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())?
        })
        .unwrap_or(0);
    while buf.len() < head_end + length {
        let Ok(n) = stream.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
    let target = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let html = "text/html; charset=utf-8";
    let json = "application/json";
    let (status, kind, body) = if target
        .starts_with("/booking/flights/deeplink")
    {
        if target.contains("destination=BLK") {
            ("403 Forbidden", html, CHALLENGE.to_owned())
        } else {
            ("200 OK", html, booking_page())
        }
    } else if target.starts_with("/bfm/rest/booking/availability/search") {
        (
            "200 OK",
            json,
            fixture("availability-search.GRU-LIS.2026-11-10.oneway.json"),
        )
    } else if target.starts_with("/bfm/rest/booking/availability/calendar/") {
        ("200 OK", json, calendar_for(&body))
    } else {
        ("404 Not Found", "text/plain", "not found".to_owned())
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve(stream));
        }
    });
    address
}

fn chromium(profile: PathBuf) -> Option<ChromiumConfig> {
    let Some(binary) =
        std::env::var_os("TM_CHROMIUM").filter(|v| !v.is_empty())
    else {
        eprintln!(
            "skipping: TM_CHROMIUM is not set (run inside `nix develop`)"
        );
        return None;
    };
    let mut config = ChromiumConfig::new(binary, profile);
    config.window = (1120, 780);
    Some(config)
}

fn fast() -> Timeouts {
    Timeouts {
        navigation: Duration::from_secs(20),
        open_graph: Duration::from_secs(10),
        response: Duration::from_secs(10),
        settle: Duration::from_millis(400),
        scroll_pause: Duration::from_millis(50),
    }
}

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("a date")
}

fn job(address: SocketAddr, destination: &str, cabin: Cabin) -> TapJob {
    let first = day(2026, 11, 1);
    let url = tap::Deeplink {
        market: "BR".into(),
        language: "en".into(),
        origin: "GRU".into(),
        destination: destination.into(),
        departure: first,
        return_date: None,
        passengers: Passengers::default(),
    }
    .url(&format!("http://{address}"))
    .expect("a deep link");
    // Checked at noon on the capture day: the days stored on 10-06 are
    // fresh, those stored on 10-03 are not.
    let now = day(2026, 10, 6)
        .and_hms_opt(12, 0, 0)
        .expect("a time")
        .and_utc();
    TapJob {
        pos: PosId(1),
        url,
        origin: "GRU".into(),
        destination: destination.into(),
        cabin,
        departure: first,
        first,
        horizon_end: day(2027, 1, 31),
        max_months: 3,
        fresh_since: now - TimeDelta::hours(tap::FRESH_HOURS),
    }
}

fn brl(minor: i64) -> Money {
    Money::new(minor, Currency::brl())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tap_source_reads_the_calendar() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let Some(config) = chromium(dir.path().join("profile")) else {
        return;
    };
    let address = server().await;
    let sources = BrowserSources::new(config).with_timeouts(fast());
    let pos = PointOfSale {
        id: PosId(1),
        country: CountryCode::new("BR"),
        currency: Currency::brl(),
        is_home: true,
        active: true,
    };
    sources
        .start(std::slice::from_ref(&pos))
        .await
        .expect("Chromium starts");

    // Economy: the deep link, then November to January from the
    // calendar, scrolled month by month.
    let economy = sources.tap(&job(address, "LIS", Cabin::Economy)).await;
    // Business: the availability search of one date.
    let business = sources.tap(&job(address, "LIS", Cabin::Business)).await;
    // A Cloudflare challenge in place of the page.
    let blocked = sources.tap(&job(address, "BLK", Cabin::Economy)).await;
    sources.stop().await;

    assert_eq!(economy.outcome, QueryOutcome::Ok, "{economy:?}");
    // One navigation and three calendar months.
    assert_eq!(economy.page_loads, 4);
    let cells = &economy.value;
    // 22 fresh days a month (2nd to 23rd); the stale ones are left out.
    assert_eq!(cells.len(), 66, "{cells:?}");
    let departures: Vec<NaiveDate> =
        cells.iter().map(|c| c.departure).collect();
    assert_eq!(departures[0], day(2026, 11, 2));
    assert_eq!(departures[21], day(2026, 11, 23));
    assert_eq!(departures[22], day(2026, 12, 2));
    assert_eq!(departures[65], day(2027, 1, 23));
    assert!(!departures.contains(&day(2026, 11, 1)));
    assert!(!departures.contains(&day(2026, 11, 24)));
    assert!(cells.iter().all(|c| c.price == Some(brl(191_199))));
    assert!(cells.iter().all(|c| c.return_date.is_none()));
    assert!(cells.iter().all(|c| c.seller.as_deref() == Some(tap::SITE)));
    // The searched date carries the live search's flight.
    let live = cells
        .iter()
        .find(|c| c.departure == day(2026, 11, 10))
        .expect("2026-11-10");
    assert_eq!(live.carrier.as_deref(), Some("TP"));
    assert_eq!(live.stops, Some(0));
    assert_eq!(live.duration_minutes, Some(590));

    assert_eq!(business.outcome, QueryOutcome::Ok, "{business:?}");
    assert_eq!(business.page_loads, 1);
    assert_eq!(business.value.len(), 1);
    assert_eq!(business.value[0].departure, day(2026, 11, 10));
    assert_eq!(business.value[0].price, Some(brl(1_214_279)));

    assert!(
        matches!(&blocked.outcome, QueryOutcome::Blocked { detail } if detail.contains("Cloudflare")),
        "{blocked:?}"
    );
    assert_eq!(blocked.page_loads, 1);
    assert!(blocked.value.is_empty());
}
