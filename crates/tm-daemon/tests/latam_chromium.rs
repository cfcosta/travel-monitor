//! The LATAM source in a real Chromium, without LATAM (DECISIONS.md
//! 0084, 0085, 0090, 0091): a local server plays
//! `www.latamairlines.com` with a tiny results page. On load it asks
//! the cash calendar and the offers call (`offers/search`, or
//! `offers/search/redemption` for awards) for the searched date and
//! gets the captured responses back; it shows a seven-day date strip
//! whose days search again without a page load (clicked only when the
//! job's strip says so), and, for a round trip, result cards whose
//! brand choice makes the return leg's call with the chosen
//! `outOfferId`. The page sets LATAM's session cookie. Other
//! destinations play the login wall, Akamai's denial of the page, its
//! denial of the offers call, and its denial of a strip click's search
//! or a round trip's return call after the page was read.
//!
//! Every click is the browser's mouse (0091). The page counts the
//! trusted `mousemove`s before each click and tags the search a click
//! starts with them (`tmMoves`, the fewest before any click so far, and
//! `tmUntrusted`, the clicks that were not trusted), so the server sees
//! whether a hand moved before each click. Headless, the mouse sends CDP
//! input; [`the_offscreen_pointer_moves_before_each_click`] runs the
//! clicks on the offscreen display's real pointer (the machine's GPU).
//!
//! Skipped (with a message) when `TM_CHROMIUM` is not set; the dev
//! shell sets it. The offscreen test also skips without a render node
//! or `$XDG_RUNTIME_DIR`.

#![allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on: a test is the runtime's entry point"
)]

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{NaiveDate, TimeDelta, Utc};
use tm_browser::{ChromiumConfig, Display};
use tm_daemon::{
    sites::latam::{
        self,
        dates::{Pick, Strip},
    },
    sources::{
        LatamJob,
        Sources,
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
        .join("../../docs/research/sites/fixtures")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The results page. It reads its search from the URL, as LATAM's does.
const RESULTS_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<title>Seleção de voos | LATAM Airlines</title>
<style>#strip button { margin: 2px; padding: 8px; } .card { margin: 6px; padding: 6px; border: 1px solid #999; }</style>
</head><body>
<form id="searchbox"><input id="when" value="10/11/2026"></form>
<div id="strip"></div>
<div id="results">loading</div>
<script>
const q = new URLSearchParams(location.search);
const origin = q.get('origin'), destination = q.get('destination');
const award = q.get('redemption') === 'true';
const round = q.get('trip') === 'RT';
const outbound = q.get('outbound').slice(0, 10);
const inbound = round ? q.get('inbound').slice(0, 10) : 'null';
const path = award ? '/bff/air-offers/v2/offers/search/redemption' : '/bff/air-offers/v2/offers/search';
// Trusted mousemoves since the last click; the fewest before any click,
// and the clicks that were not trusted.
let moves = 0, fewest = null, untrusted = 0;
addEventListener('mousemove', e => { if (e.isTrusted) moves++; }, { capture: true, passive: true });
addEventListener('click', e => {
  fewest = fewest === null ? moves : Math.min(fewest, moves);
  if (!e.isTrusted) untrusted++;
  moves = 0;
}, { capture: true, passive: true });
const mark = () => fewest === null ? '' : '&tmMoves=' + fewest + '&tmUntrusted=' + untrusted;
const offers = (outFrom, extra) => fetch(path + '?sort=RECOMMENDED&adult=1&child=0&infant=0&cabinType=Economy'
  + '&origin=' + origin + '&destination=' + destination + '&outFrom=' + outFrom + '&inFrom=' + inbound
  + '&redemption=' + award + (extra || '&outOfferId=null&outFlightDate=null') + mark()).then(r => r.ok ? r.json() : null);
const [y, m] = outbound.split('-').map(Number);
fetch('/bff/air-offers/v2/calendar?origin=' + origin + '&destination=' + destination + '&month=' + m
  + '&year=' + y + '&isRoundTrip=' + round + '&extended=true');
const show = data => {
  const results = document.getElementById('results');
  if (!data) { results.innerText = 'error'; return; }
  results.innerText = '';
  data.content.forEach((item, i) => {
    const card = document.createElement('div');
    card.className = 'card';
    const header = document.createElement('div');
    header.dataset.testid = 'wrapper-card-header-' + i;
    header.innerText = item.summary.flightCode + ' ' + item.summary.lowestPrice.amount;
    card.appendChild(header);
    header.addEventListener('click', () => {
      item.summary.brands.forEach((brand, b) => {
        const select = document.createElement('button');
        select.id = 'bundle-detail-' + b + '-flight-select';
        select.innerText = 'Escolher ' + brand.brandText;
        select.addEventListener('click', () => {
          const upsell = document.createElement('button');
          upsell.id = 'latam-plus-continue';
          upsell.innerText = 'Continuar';
          upsell.addEventListener('click', () => {
            offers(outbound, '&outOfferId=' + brand.offerId + '&outFlightDate=' + item.summary.destination.arrival)
              .then(back => { results.innerText = back ? back.content.length + ' returns' : 'error'; });
          });
          document.body.appendChild(upsell);
        });
        card.appendChild(select);
      });
    });
    results.appendChild(card);
  });
};
offers(outbound).then(show);
if (!round) {
  const strip = document.getElementById('strip');
  const day = new Date(outbound + 'T12:00:00Z');
  for (let k = -3; k <= 3; k++) {
    const d = new Date(day.getTime() + k * 86400000);
    const iso = d.toISOString().slice(0, 10);
    const [, mm, dd] = iso.split('-');
    const button = document.createElement('button');
    button.innerText = dd + '/' + mm + ' R$ 2.001';
    button.addEventListener('click', () => {
      history.pushState({}, '', location.pathname + '?' + q.toString().replace(/outbound=[^&]*/, 'outbound=' + iso + 'T12%3A00%3A00.000Z'));
      offers(iso).then(show);
    });
    strip.appendChild(button);
  }
}
</script>
</body></html>"#;

const LOGIN_PAGE: &str = r#"<!doctype html><html><head><title>Entrar</title></head>
<body><p>Insira seu usuário</p><p>Email, CPF ou Número de cliente</p></body></html>"#;

/// The query value of `key` in `target`.
fn param<'a>(target: &'a str, key: &str) -> Option<&'a str> {
    target.split_once('?')?.1.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// What the server saw: every offers call's path and query.
type Seen = Arc<Mutex<Vec<String>>>;

async fn serve(mut stream: TcpStream, seen: Seen) {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let Ok(n) = stream.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf).to_string();
    let target = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let html = "text/html; charset=utf-8";
    let json = "application/json";
    let destination = param(&target, "destination").unwrap_or("");
    let expires = (Utc::now() + TimeDelta::days(5))
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    let mut headers = String::new();
    let (status, kind, body) = if target.starts_with("/br/pt/oferta-voos") {
        match destination {
            "WAL" => {
                headers.push_str(
                    "Location: /u/login/identifier?state=x&ui_locales=pt\r\n",
                );
                ("302 Found", html, String::new())
            }
            "BLK" => (
                "403 Forbidden",
                html,
                fixture("latam/headless-access-denied.html"),
            ),
            _ => {
                headers.push_str(&format!(
                    "Set-Cookie: _xp_session=s; Path=/; Expires={expires}; HttpOnly\r\n"
                ));
                ("200 OK", html, RESULTS_PAGE.to_owned())
            }
        }
    } else if target.starts_with("/u/login") {
        ("200 OK", html, LOGIN_PAGE.to_owned())
    } else if target.starts_with("/bff/air-offers/v2/calendar") {
        (
            "200 OK",
            json,
            fixture("latam/calendar-ow-gru-lis-2026-11-extended.json"),
        )
    } else if target.starts_with("/bff/air-offers/v2/offers/search") {
        if let Ok(mut seen) = seen.lock() {
            seen.push(target.clone());
        }
        let award = target.contains("/redemption");
        let back = latam::is_return_leg(&target);
        let name = match (destination, award, back) {
            ("DNY", ..) => "",
            ("MIA" | "RDN", true, false) => {
                "latam-pass/offers-search-redemption-rt-gru-mia-2027-03-04-outbound-br.json"
            }
            ("RDN", true, true) => "",
            // The page's own search answers; a strip day's is denied.
            ("SDN", true, _)
                if param(&target, "outFrom") != Some("2026-11-10") =>
            {
                ""
            }
            ("MIA", true, true) => {
                "latam-pass/offers-search-redemption-rt-gru-mia-2027-03-11-inbound-br.json"
            }
            (_, true, _) => {
                "latam-pass/offers-search-redemption-ow-gru-lis-2026-11-10-br.json"
            }
            (_, false, _) => {
                "latam/offers-search-ow-gru-lis-2026-11-10-br-headed-warm.json"
            }
        };
        if name.is_empty() {
            (
                "403 Forbidden",
                html,
                fixture("latam/headless-access-denied.html"),
            )
        } else {
            ("200 OK", json, fixture(name))
        }
    } else {
        ("404 Not Found", "text/plain", "not found".to_owned())
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn server() -> (SocketAddr, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let address = listener.local_addr().expect("an address");
    let seen: Seen = Arc::default();
    let shared = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve(stream, Arc::clone(&shared)));
        }
    });
    (address, seen)
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
        strip_pause: (Duration::from_millis(100), Duration::from_millis(200)),
        look: (Duration::from_millis(200), Duration::from_millis(600)),
    }
}

/// Two strip clicks: the nearest days.
fn two_days() -> Strip {
    Strip {
        max_clicks: 2,
        pick: Some(Pick::Every),
        first: day(2026, 10, 8),
        last: day(2027, 4, 4),
        cash: BTreeMap::new(),
        skip: BTreeSet::new(),
    }
}

/// Whether the search an offers call made followed clicks that each had
/// trusted mouse movement before them, as the page tagged it.
fn by_hand(call: &str) -> bool {
    param(call, "tmMoves")
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| n > 0)
        && param(call, "tmUntrusted") == Some("0")
}

fn point_of_sale() -> PointOfSale {
    PointOfSale {
        id: PosId(1),
        country: CountryCode::new("BR"),
        currency: Currency::brl(),
        is_home: true,
        active: true,
    }
}

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("a date")
}

fn job(
    address: SocketAddr,
    destination: &str,
    departure: NaiveDate,
    return_date: Option<NaiveDate>,
    award: bool,
    strip: Strip,
) -> LatamJob {
    let base = format!("http://{address}");
    let search = latam::Search {
        origin: "GRU".into(),
        destination: destination.into(),
        outbound: departure,
        inbound: return_date,
        passengers: Passengers::default(),
        cabin: Cabin::Economy,
        redemption: award,
    };
    LatamJob {
        pos: PosId(1),
        url: search.url(&base).expect("a URL"),
        base,
        origin: "GRU".into(),
        destination: destination.into(),
        cabin: Cabin::Economy,
        departure,
        return_date,
        award,
        seller: if award {
            latam::MILES_SITE.into()
        } else {
            latam::AIRLINE_SITE.into()
        },
        strip,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_latam_source_reads_offers_the_strip_and_round_trips() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let Some(config) = chromium(dir.path().join("profile")) else {
        return;
    };
    let (address, seen) = server().await;
    let sources = BrowserSources::new(config).with_timeouts(fast());
    let pos = point_of_sale();
    sources
        .start(std::slice::from_ref(&pos), 7)
        .await
        .expect("Chromium starts");

    let nov10 = day(2026, 11, 10);
    // An award leg with the strip off (the default, 0090): its page,
    // and nothing clicked.
    let single = sources
        .latam(&job(address, "LIS", nov10, None, true, Strip::none()))
        .await;
    let calls_single = seen.lock().expect("lock").clone();
    // An award leg with two strip clicks: the nearest days.
    let award = sources
        .latam(&job(address, "LIS", nov10, None, true, two_days()))
        .await;
    // The same, with the first strip day's search denied: the page's
    // cells are kept under the block (0090).
    let strip_denied = sources
        .latam(&job(address, "SDN", nov10, None, true, two_days()))
        .await;
    // Cash, one date.
    let cash = sources
        .latam(&job(address, "LIS", nov10, None, false, Strip::none()))
        .await;
    // An award round trip: the outbound's brand chosen on its card.
    let mar4 = day(2027, 3, 4);
    let mar11 = day(2027, 3, 11);
    let round = sources
        .latam(&job(address, "MIA", mar4, Some(mar11), true, Strip::none()))
        .await;
    // The same, with the return call denied after the outbound was read.
    let round_denied = sources
        .latam(&job(address, "RDN", mar4, Some(mar11), true, Strip::none()))
        .await;
    let walled = sources
        .latam(&job(address, "WAL", nov10, None, true, Strip::none()))
        .await;
    let denied = sources
        .latam(&job(address, "BLK", nov10, None, false, Strip::none()))
        .await;
    let refused = sources
        .latam(&job(address, "DNY", nov10, None, true, Strip::none()))
        .await;
    sources.stop().await;

    // Strip off: one page load, one date, the page's own search only.
    assert_eq!(single.outcome, QueryOutcome::Ok, "{single:?}");
    assert_eq!(single.page_loads, 1);
    assert_eq!(single.value.clicks, 0);
    assert!(single.value.cells.iter().all(|c| c.departure == nov10));
    let single_dates: Vec<&str> = calls_single
        .iter()
        .filter(|c| c.contains("/redemption"))
        .filter_map(|c| param(c, "outFrom"))
        .collect();
    assert_eq!(single_dates, ["2026-11-10"]);

    assert_eq!(award.outcome, QueryOutcome::Ok, "{award:?}");
    // The page and two clicks, each a load.
    assert_eq!(award.page_loads, 3);
    assert_eq!(award.value.clicks, 2);
    let dates: BTreeSet<NaiveDate> =
        award.value.cells.iter().map(|c| c.departure).collect();
    assert_eq!(dates, [day(2026, 11, 9), nov10, day(2026, 11, 11)].into());
    assert!(award.value.cells.iter().all(|c| c.miles.is_some()));
    let light = award
        .value
        .cells
        .iter()
        .find(|c| {
            c.departure == nov10
                && c.flights.as_deref() == Some("LA8146@2026-11-10T17:40")
                && c.award.as_ref().and_then(|a| a.brand.as_deref())
                    == Some("LIGHT")
        })
        .expect("LA8146 LIGHT");
    assert_eq!(light.miles, Some(63_848));
    assert_eq!(light.price, Some(Money::new(6861, Currency::brl())));
    // Each click's own search, for its own date.
    let calls = seen.lock().expect("lock").clone();
    let award_calls: Vec<&String> = calls[calls_single.len()..]
        .iter()
        .filter(|c| c.contains("/redemption") && c.contains("destination=LIS"))
        .collect();
    let award_dates: Vec<&str> = award_calls
        .iter()
        .filter_map(|c| param(c, "outFrom"))
        .collect();
    assert_eq!(award_dates, ["2026-11-10", "2026-11-09", "2026-11-11"]);
    // The page's own search came before any click; each strip day's
    // came from a mouse click with movement before it (0091).
    assert!(!award_calls[0].contains("tmMoves"), "{}", award_calls[0]);
    assert!(
        award_calls[1..].iter().all(|c| by_hand(c)),
        "{award_calls:?}"
    );
    // The calendar ranks; the session cookie is read.
    assert_eq!(award.value.calendar.get(&nov10), Some(&200_141));
    let until = award.value.session.flatten().expect("a session");
    assert!(until > Utc::now() + TimeDelta::days(4), "{until}");

    assert_eq!(cash.outcome, QueryOutcome::Ok, "{cash:?}");
    assert_eq!(cash.page_loads, 1);
    assert_eq!(cash.value.cells.len(), 1);
    assert_eq!(
        cash.value.cells[0].price,
        Some(Money::new(190_743, Currency::brl()))
    );
    assert_eq!(cash.value.clicks, 0);

    // A denied strip click is a block after two loads, and the page's
    // own cells are kept (0090).
    assert!(
        matches!(&strip_denied.outcome, QueryOutcome::Blocked { detail } if detail.contains("403")),
        "{strip_denied:?}"
    );
    assert_eq!(strip_denied.page_loads, 2);
    assert_eq!(strip_denied.value.clicks, 1);
    assert!(!strip_denied.value.cells.is_empty());
    assert!(
        strip_denied
            .value
            .cells
            .iter()
            .all(|c| c.departure == nov10 && c.miles.is_some())
    );
    assert_eq!(strip_denied.value.session, None);

    assert_eq!(round.outcome, QueryOutcome::Ok, "{round:?}");
    // The page and the outbound's choice.
    assert_eq!(round.page_loads, 2);
    assert!(
        round
            .value
            .cells
            .iter()
            .all(|c| c.return_date == Some(mar11))
    );
    let returns: Vec<String> = calls_after(&seen)
        .into_iter()
        .filter(|c| latam::is_return_leg(c) && c.contains("destination=MIA"))
        .collect();
    assert_eq!(returns.len(), 1, "{returns:?}");
    assert!(returns[0].contains("loyalty"), "{}", returns[0]);
    // The card, its brand and the upsell were clicked by the mouse.
    assert!(by_hand(&returns[0]), "{}", returns[0]);
    let best = round
        .value
        .cells
        .iter()
        .filter_map(|c| c.miles)
        .min()
        .expect("miles");
    // The cheapest outbound (LA8212/LA4400, 67,644 miles, latam-pass.md
    // §2) plus the cheapest return.
    assert!(best > 67_644 && best < 67_644 + 100_000, "{best}");

    // A denied return call is a block after two loads (0090). The
    // outbound halves price nothing alone, so no cells are kept, and
    // the session is not read on a failure.
    assert!(
        matches!(&round_denied.outcome, QueryOutcome::Blocked { detail } if detail.contains("403")),
        "{round_denied:?}"
    );
    assert_eq!(round_denied.page_loads, 2);
    assert!(round_denied.value.cells.is_empty());
    assert_eq!(round_denied.value.session, None);

    assert_eq!(walled.outcome, QueryOutcome::LoggedOut, "{walled:?}");
    assert_eq!(walled.page_loads, 1);
    assert!(
        matches!(&denied.outcome, QueryOutcome::Blocked { detail } if detail.contains("Akamai")),
        "{denied:?}"
    );
    assert_eq!(denied.page_loads, 1);
    assert!(
        matches!(&refused.outcome, QueryOutcome::Blocked { detail } if detail.contains("403")),
        "{refused:?}"
    );
}

/// The offers calls the server saw so far.
fn calls_after(seen: &Seen) -> Vec<String> {
    seen.lock().expect("lock").clone()
}

/// The same clicks on the offscreen display, where the mouse is the
/// compositor's real pointer (0091): the strip days and the round trip's
/// outbound are clicked, each after trusted movement.
#[tokio::test(flavor = "multi_thread")]
async fn the_offscreen_pointer_moves_before_each_click() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let Some(mut config) = chromium(dir.path().join("profile")) else {
        return;
    };
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR is not set");
        return;
    }
    if tm_compositor::find_render_node(None).is_err() {
        eprintln!("skipping: no GPU render node");
        return;
    }
    config.display = Display::Offscreen;
    let (address, seen) = server().await;
    let sources = BrowserSources::new(config).with_timeouts(fast());
    sources
        .start(&[point_of_sale()], 11)
        .await
        .expect("Chromium starts offscreen");
    let nov10 = day(2026, 11, 10);
    let (mar4, mar11) = (day(2027, 3, 4), day(2027, 3, 11));
    let award = sources
        .latam(&job(address, "LIS", nov10, None, true, two_days()))
        .await;
    let round = sources
        .latam(&job(address, "MIA", mar4, Some(mar11), true, Strip::none()))
        .await;
    sources.stop().await;

    assert_eq!(award.outcome, QueryOutcome::Ok, "{award:?}");
    assert_eq!((award.page_loads, award.value.clicks), (3, 2));
    assert_eq!(round.outcome, QueryOutcome::Ok, "{round:?}");
    assert_eq!(round.page_loads, 2);
    let calls = calls_after(&seen);
    let clicked: Vec<&String> = calls
        .iter()
        .filter(|c| {
            latam::is_return_leg(c)
                || (c.contains("destination=LIS")
                    && param(c, "outFrom") != Some("2026-11-10"))
        })
        .collect();
    assert_eq!(clicked.len(), 3, "{calls:?}");
    assert!(clicked.iter().all(|c| by_hand(c)), "{clicked:?}");
}
