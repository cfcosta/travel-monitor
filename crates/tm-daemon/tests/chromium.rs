//! End to end in a real headless Chromium, without the internet: a
//! local server plays Google Flights with a tiny "price graph" page
//! whose buttons POST to a `GetCalendarGraph` URL and get the captured
//! fixtures back. The daemon captures the page's own responses, parses
//! them and stores the cells (DECISIONS.md 0032).
//!
//! Skipped (with a message) when `TM_CHROMIUM` is not set; the dev
//! shell sets it.

#![allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on: a test is the runtime's entry point"
)]

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use tm_browser::ChromiumConfig;
use tm_daemon::{
    Config,
    Daemon,
    candidates::Only,
    jobs::JobOptions,
    sources::{
        CalendarJob,
        Sources,
        browser::{BrowserSources, Timeouts},
    },
};
use tm_domain::{
    Cabin,
    CountryCode,
    Currency,
    FamilyKey,
    FamilyKind,
    Leg,
    Passengers,
    PointOfSale,
    PosId,
    QueryOutcome,
    Source,
};
use tm_gflights::{PlaceRef, Search};
use tm_store::Store;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
};

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tm-gflights/tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

const PAGES: usize = 3;

/// The fake results page: "Price graph" fetches page 1, each "Scroll
/// forward" the next, and the control disappears after the last.
fn page() -> String {
    let bodies: Vec<String> = (1..=PAGES)
        .map(|i| {
            fixture(&format!("oneway_sao_lis_GetCalendarGraph_{i}.request.txt"))
        })
        .collect();
    let bodies = serde_json::to_string(&bodies).unwrap_or_default();
    format!(
        r#"<!doctype html><html><head><title>São Paulo to Lisbon | Google Flights</title></head>
<body>
<h1>Flights from São Paulo to Lisbon</h1>
<div role="tablist"><button id="grid">Date grid</button><button id="graph">Price graph</button></div>
<div id="chart"></div>
<button id="fwd" aria-label="Scroll forward" style="display:none">&gt;</button>
<script>
const bodies = {bodies};
let n = 0;
async function load() {{
  n += 1;
  const url = '/_/FlightsFrontendUi/data/travel.frontend.flights.FlightsFrontendService/GetCalendarGraph?n=' + n;
  const r = await fetch(url, {{method: 'POST', headers: {{'Content-Type': 'application/x-www-form-urlencoded;charset=UTF-8'}}, body: bodies[n - 1]}});
  document.getElementById('chart').innerText = 'page ' + n + ': ' + (await r.text()).length + ' bytes';
  document.getElementById('fwd').style.display = n < bodies.length ? 'inline' : 'none';
}}
document.getElementById('graph').addEventListener('click', load);
document.getElementById('fwd').addEventListener('click', load);
</script>
</body></html>"#
    )
}

async fn serve(stream: TcpStream, page: Arc<String>) {
    let mut stream = stream;
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
    let target = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let (status, kind, body) = if target.starts_with("/travel/flights/search") {
        (
            "200 OK",
            "text/html; charset=utf-8",
            page.as_str().to_owned(),
        )
    } else if target.contains("GetCalendarGraph") {
        let n = target
            .rsplit("n=")
            .next()
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(1);
        (
            "200 OK",
            "application/json; charset=utf-8",
            fixture(&format!(
                "oneway_sao_lis_GetCalendarGraph_{n}.response.txt"
            )),
        )
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
    let page = Arc::new(page());
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve(stream, Arc::clone(&page)));
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
        settle: Duration::from_millis(300),
        scroll_pause: Duration::from_millis(50),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_source_reads_a_price_graph() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let Some(config) = chromium(dir.path().join("profile")) else {
        return;
    };
    let address = server().await;
    let mut sources = BrowserSources::new(config, None).with_timeouts(fast());
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
    let date =
        chrono::NaiveDate::from_ymd_opt(2026, 11, 10).unwrap_or_default();
    let search = Search::one_way(
        PlaceRef::City("/m/022pfm".into()),
        PlaceRef::Airport("LIS".into()),
        date,
        Cabin::Economy,
        Passengers::default(),
    );
    let job = CalendarJob {
        pos: pos.id,
        url: format!(
            "http://{address}/travel/flights/search?tfs=x&hl=en&gl=BR&curr=BRL"
        ),
        search,
        horizon_end: chrono::NaiveDate::from_ymd_opt(2027, 5, 1)
            .unwrap_or_default(),
        max_pages: 7,
        display: Currency::brl(),
    };
    let fetch = sources.calendar(&job).await;
    sources.stop().await;
    assert_eq!(fetch.outcome, QueryOutcome::Ok, "{fetch:?}");
    // One navigation and three graph pages.
    assert_eq!(fetch.page_loads, 1 + PAGES as u32);
    let expected: usize = (1..=PAGES)
        .map(|i| {
            tm_gflights::rpc::parse_calendar(&fixture(&format!(
                "oneway_sao_lis_GetCalendarGraph_{i}.response.txt"
            )))
            .map(|c| c.len())
            .unwrap_or(0)
        })
        .sum();
    assert!(expected > 60, "fixtures hold {expected} cells");
    assert_eq!(fetch.value.len(), expected);
    assert!(fetch.value.iter().all(|c| c.return_date.is_none()));
    assert!(fetch.value.iter().filter(|c| c.price.is_some()).count() > 50);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_captures_parses_and_stores() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let Some(config) = chromium(dir.path().join("profile")) else {
        return;
    };
    let address = server().await;
    let store = Store::open(dir.path().join("tm.db")).await.expect("opens");
    store.seed_defaults().await.expect("seeds");
    let lis = store
        .route_between("SAO", "LIS")
        .await
        .expect("reads")
        .expect("SAO-LIS");
    let home = store
        .home_point_of_sale()
        .await
        .expect("reads")
        .expect("BR");
    let family = store
        .family_id(&FamilyKey {
            route: lis,
            cabin: Cabin::Economy,
            source: Source::GoogleFlights,
            pos: home.id,
            kind: FamilyKind::Leg { leg: Leg::Outbound },
        })
        .await
        .expect("reads")
        .expect("the family");
    let sources = BrowserSources::new(config, None).with_timeouts(fast());
    let daemon_config = Config {
        once: true,
        only: Only {
            family: Some(family),
            destination: None,
        },
        pace: (0, 0),
        seed: Some(1),
        jobs: JobOptions {
            origin_override: Some(format!("http://{address}")),
            ..JobOptions::default()
        },
        ..Config::default()
    };
    let (_stop, shutdown) = watch::channel(false);
    let mut daemon =
        Daemon::new(store.clone(), sources, daemon_config, shutdown);
    daemon.run().await.expect("runs once");

    let recent = store.recent_queries(5).await.expect("reads");
    let check = recent
        .iter()
        .find(|q| q.family == family)
        .expect("the family was checked");
    assert_eq!(check.outcome, Some(QueryOutcome::Ok), "{check:?}");
    assert_eq!(check.page_loads, Some(1 + PAGES as u32));
    assert!(check.cells > 60, "{} cells", check.cells);
    let cells = store.latest_cells(family).await.expect("reads");
    assert_eq!(cells.len() as u32, check.cells);
    assert!(cells.iter().any(|c| c.price.is_some()));
    let state = store
        .family(family)
        .await
        .expect("reads")
        .expect("exists")
        .state;
    assert_eq!(state.checks, 1);
    assert!(state.last_min_price_minor.is_some());
    let beat = store
        .heartbeat()
        .await
        .expect("reads")
        .expect("a heartbeat");
    assert_eq!(beat.state, "stopped");
    assert_eq!(beat.loads_today, 1 + PAGES as u32);
    assert!(!daemon.sources().is_running());
}
