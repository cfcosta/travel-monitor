//! The browser runtime against a real headless Chromium (`TM_CHROMIUM`)
//! and local pages. Each test skips, saying so, when it is unset.

#![allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on: a test is the runtime's entry point"
)]

mod common;

use std::time::Duration;

use common::{Reply, Server, chromium};
use serde_json::json;
use tm_browser::{ContextOptions, Error};

const LOAD: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(25);

#[tokio::test(flavor = "multi_thread")]
async fn navigation_and_evaluation() {
    let Some((chromium, _profile)) = chromium().await else {
        return;
    };
    let server = Server::start(|seen| match seen.path() {
        "/" => Reply::html(
            "<title>Home</title><p id=late></p>\
             <script>setTimeout(() => late.textContent = 'ready', 300)</script>",
        ),
        _ => Reply::not_found(),
    })
    .await;
    let version = chromium.browser().version().await.expect("version");
    assert!(version.product.contains("Chrome"), "{version:?}");

    let page = chromium.browser().new_page().await.expect("page");
    page.navigate(&server.url("/"), LOAD)
        .await
        .expect("navigate");
    assert_eq!(page.url().await.expect("url"), server.url("/"));
    assert_eq!(page.eval("1 + 1").await.expect("eval"), json!(2));
    assert_eq!(page.eval("undefined").await.expect("eval"), json!(null));
    let title: String = page.eval_as("document.title").await.expect("title");
    assert_eq!(title, "Home");
    let later = page
        .eval("new Promise(r => setTimeout(() => r({ok: [1, 'two']}), 50))")
        .await
        .expect("promise");
    assert_eq!(later, json!({"ok": [1, "two"]}));
    let error = page
        .eval("throw new Error('boom')")
        .await
        .expect_err("throws");
    assert!(
        matches!(&error, Error::Script(m) if m.contains("boom")),
        "{error}"
    );

    let ready = page
        .wait_until("document.querySelector('#late').textContent", LOAD, POLL)
        .await
        .expect("wait");
    assert_eq!(ready, "ready");
    let timeout = page
        .wait_until("window.never", Duration::from_millis(200), POLL)
        .await
        .expect_err("never true");
    assert!(matches!(timeout, Error::Timeout { .. }), "{timeout}");

    // A fragment change is a same-document navigation.
    page.navigate(&server.url("/#later"), LOAD)
        .await
        .expect("fragment");
    assert!(page.url().await.expect("url").ends_with("#later"));

    let refused = page
        .navigate("http://127.0.0.1:9/", LOAD)
        .await
        .expect_err("Chromium refuses the discard port");
    assert!(matches!(refused, Error::Navigation { .. }), "{refused}");

    // The tab stays usable after an error page.
    assert_eq!(
        page.eval("location.href").await.expect("error page"),
        "chrome-error://chromewebdata/"
    );
    page.navigate(&server.url("/"), LOAD)
        .await
        .expect("navigate back");
    let png = page.screenshot().await.expect("screenshot");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    page.close().await.expect("close");
    chromium.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn clicks_typing_and_keys() {
    let Some((chromium, _profile)) = chromium().await else {
        return;
    };
    let server = Server::start(|_| {
        Reply::html(
            r#"<title>Form</title>
            <form onsubmit="event.preventDefault(); window.submitted = this.city.value">
              <input id="city" name="city">
            </form>
            <button id="more" onclick="window.count = (window.count || 0) + 1">More</button>
            <button id="hidden" style="display:none">Hidden</button>
            <button id="covered" style="position:absolute;top:300px">Covered</button>
            <div style="position:absolute;top:290px;height:60px;width:100%;background:white"></div>
            <div style="height:3000px"></div>
            <button id="far" onclick="window.far = true">Far below</button>"#,
        )
    })
    .await;
    let page = chromium.browser().new_page().await.expect("page");
    page.navigate(&server.url("/"), LOAD)
        .await
        .expect("navigate");

    page.click("#more").await.expect("click");
    page.click("#more").await.expect("click again");
    // Input events land asynchronously; wait for both handlers to run
    // rather than reading the count once (this was flaky under load).
    page.wait_until("window.count === 2", LOAD, POLL)
        .await
        .expect("both clicks counted");

    // Below the fold: scrolled into view first.
    page.click("#far").await.expect("far click");
    assert_eq!(page.eval("window.far").await.expect("far"), true);

    page.fill("#city", "Lisboa").await.expect("fill");
    page.fill("#city", "São Paulo")
        .await
        .expect("refill replaces");
    assert_eq!(page.eval("city.value").await.expect("value"), "São Paulo");
    page.press_key("End").await.expect("end");
    page.type_text(" (SAO)").await.expect("type");
    page.press_key("Enter").await.expect("enter");
    assert_eq!(
        page.eval("window.submitted").await.expect("submitted"),
        "São Paulo (SAO)"
    );

    assert!(matches!(
        page.click("#missing").await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        page.click("#hidden").await,
        Err(Error::NotInteractable { .. })
    ));
    let covered = page.click("#covered").await.expect_err("covered");
    assert!(
        matches!(&covered, Error::NotInteractable { reason, .. } if reason.contains("covered")),
        "{covered}"
    );
    assert!(matches!(
        page.press_key("NoSuchKey").await,
        Err(Error::UnsupportedKey(_))
    ));
    chromium.shutdown().await.expect("shutdown");
}

const RPC_PAGE: &str = r#"<title>RPC</title><script>
function calendar() {
  const xhr = new XMLHttpRequest();
  xhr.open("POST", "/_/FlightsFrontendUi/data/travel.frontend.flights.FlightsFrontendService/GetCalendarGraph?rt=c");
  xhr.setRequestHeader("Content-Type", "application/x-www-form-urlencoded;charset=UTF-8");
  xhr.send("f.req=%5Bnull%2C%22%5B%5D%22%5D&at=token");
}
function other() { return fetch("/other").then(r => r.text()); }
function binary() { return fetch("/_/GetCalendarGraph.bin").then(r => r.arrayBuffer()).then(b => b.byteLength); }
function explore() {
  return fetch("/_/GetExploreDestinations", {method: "POST", body: "f.req=explore"}).then(r => r.text()).then(t => t.length);
}
</script>"#;

fn big_body() -> String {
    let mut body = String::from(")]}'\n");
    while body.len() < 2_000_000 {
        body.push_str("[\"wrb.fr\",null,\"[[\\\"LIS\\\",3825]]\"]\n");
    }
    body
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_records_post_data_and_response_bodies() {
    let Some((chromium, _profile)) = chromium().await else {
        return;
    };
    let server = Server::start(|seen| match seen.path() {
        "/" => Reply::html(RPC_PAGE),
        "/other" => Reply::text("not captured"),
        "/_/GetCalendarGraph.bin" => Reply::bytes((0..=255).collect()),
        "/_/GetExploreDestinations" => Reply::text(big_body()),
        path if path.ends_with("/GetCalendarGraph") => Reply::text(format!(
            ")]}}'\n\n42\n[[\"wrb.fr\",null,\"echo {}\"]]\n",
            String::from_utf8_lossy(&seen.body)
        )),
        _ => Reply::not_found(),
    })
    .await;
    let page = chromium.browser().new_page().await.expect("page");
    let calendar = page.capture("GetCalendarGraph").await.expect("capture");
    let explore = page
        .capture("GetExploreDestinations")
        .await
        .expect("capture");
    page.navigate(&server.url("/"), LOAD)
        .await
        .expect("navigate");

    page.eval("calendar(); other(); binary()")
        .await
        .expect("requests");
    assert_eq!(calendar.wait_for(2, LOAD).await.expect("two responses"), 2);
    let mut captured = calendar.drain();
    assert!(calendar.is_empty());
    captured.sort_by_key(|c| c.method.clone());

    let binary = &captured[0];
    assert_eq!(binary.method, "GET");
    assert_eq!(binary.body, (0..=255).collect::<Vec<u8>>());
    assert!(binary.is_ok());

    let rpc = &captured[1];
    assert_eq!(rpc.method, "POST");
    assert!(
        rpc.url
            .contains("FlightsFrontendService/GetCalendarGraph?rt=c")
    );
    assert_eq!(
        rpc.post_data.as_deref(),
        Some("f.req=%5Bnull%2C%22%5B%5D%22%5D&at=token")
    );
    assert_eq!(rpc.status, Some(200));
    assert_eq!(rpc.mime_type.as_deref(), Some("text/plain"));
    assert!(
        rpc.text()
            .contains("echo f.req=%5Bnull%2C%22%5B%5D%22%5D&at=token"),
        "{}",
        rpc.text()
    );
    assert_eq!(
        rpc.response_headers
            .get("cache-control")
            .map(String::as_str),
        Some("no-store")
    );

    let length = page.eval("explore()").await.expect("explore");
    explore.wait_for(1, LOAD).await.expect("explore response");
    let big = explore.drain().remove(0);
    assert_eq!(big.post_data.as_deref(), Some("f.req=explore"));
    assert_eq!(json!(big.body.len()), length);
    assert_eq!(big.text(), big_body());

    // Nothing else matched.
    page.eval("other()").await.expect("other");
    assert!(matches!(
        calendar.wait_for(1, Duration::from_millis(300)).await,
        Err(Error::Timeout { .. })
    ));
    chromium.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn init_scripts_run_before_the_page() {
    let Some((chromium, _profile)) = chromium().await else {
        return;
    };
    let server = Server::start(|_| {
        Reply::html("<script>window.seen = window.__tm ?? null</script>")
    })
    .await;
    let page = chromium.browser().new_page().await.expect("page");
    let id = page
        .add_init_script("window.__tm = 'early'")
        .await
        .expect("init script");
    page.navigate(&server.url("/"), LOAD)
        .await
        .expect("navigate");
    assert_eq!(page.eval("window.seen").await.expect("seen"), "early");
    page.remove_init_script(&id).await.expect("remove");
    page.navigate(&server.url("/again"), LOAD)
        .await
        .expect("navigate");
    assert_eq!(page.eval("window.seen").await.expect("seen"), json!(null));
    chromium.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn contexts_isolate_cookies() {
    let Some((chromium, _profile)) = chromium().await else {
        return;
    };
    let server = Server::start(|_| Reply::html("<title>Cookies</title>")).await;
    let browser = chromium.browser();
    let br = browser
        .new_context(ContextOptions::default())
        .await
        .expect("context");
    let us = browser
        .new_context(ContextOptions::default())
        .await
        .expect("context");
    assert_ne!(br.id(), us.id());

    let first = br.new_page().await.expect("page");
    first
        .navigate(&server.url("/"), LOAD)
        .await
        .expect("navigate");
    first
        .eval("document.cookie = 'pos=BR; max-age=600'")
        .await
        .expect("set");

    let second = br.new_page().await.expect("page");
    second
        .navigate(&server.url("/"), LOAD)
        .await
        .expect("navigate");
    assert_eq!(
        second.eval("document.cookie").await.expect("cookie"),
        "pos=BR"
    );

    let other = us.new_page().await.expect("page");
    other
        .navigate(&server.url("/"), LOAD)
        .await
        .expect("navigate");
    assert_eq!(other.eval("document.cookie").await.expect("cookie"), "");

    // The default context is the profile's own, separate from both.
    let default = browser.new_page().await.expect("page");
    default
        .navigate(&server.url("/"), LOAD)
        .await
        .expect("navigate");
    assert_eq!(default.eval("document.cookie").await.expect("cookie"), "");

    br.close().await.expect("dispose");
    drop(us);
    chromium.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn context_options_set_proxy_locale_and_timezone() {
    let Some((chromium, _profile)) = chromium().await else {
        return;
    };
    // The server doubles as an HTTP proxy: proxied requests arrive with
    // an absolute URL as their target.
    let server = Server::start(|seen| {
        Reply::html(format!("<title>Proxied</title><p>{}</p>", seen.target))
    })
    .await;
    let context = chromium
        .browser()
        .new_context(ContextOptions {
            proxy: Some(format!("http://{}", server.address)),
            locale: Some("pt-BR".into()),
            timezone: Some("America/Sao_Paulo".into()),
        })
        .await
        .expect("context");
    let page = context.new_page().await.expect("page");
    page.navigate("http://pos-br.test/search?gl=BR", LOAD)
        .await
        .expect("navigate through the proxy");
    let text: String =
        page.eval_as("document.body.innerText").await.expect("text");
    assert_eq!(text, "http://pos-br.test/search?gl=BR");
    assert!(
        server
            .seen()
            .iter()
            .any(|s| s.target == "http://pos-br.test/search?gl=BR"),
        "the proxy saw the request"
    );
    let zone: String = page
        .eval_as("Intl.DateTimeFormat().resolvedOptions().timeZone")
        .await
        .expect("time zone");
    assert_eq!(zone, "America/Sao_Paulo");
    let locale: String = page
        .eval_as("Intl.NumberFormat().resolvedOptions().locale")
        .await
        .expect("locale");
    assert_eq!(locale, "pt-BR");

    page.set_timezone("Europe/Lisbon").await.expect("time zone");
    page.set_locale("en-US").await.expect("locale");
    let zone: String = page
        .eval_as("Intl.DateTimeFormat().resolvedOptions().timeZone")
        .await
        .expect("time zone");
    assert_eq!(zone, "Europe/Lisbon");
    let price: String = page
        .eval_as("(3972).toLocaleString()")
        .await
        .expect("format");
    assert_eq!(price, "3,972");
    chromium.shutdown().await.expect("shutdown");
}

fn process_is_gone(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        // A zombie has exited; it only waits to be reaped.
        Ok(stat) => stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_and_drop_stop_the_process() {
    let Some((chromium, _profile)) = chromium().await else {
        return;
    };
    let pid = chromium.pid().expect("pid");
    let browser = chromium.browser().clone();
    chromium.shutdown().await.expect("shutdown");
    assert!(process_is_gone(pid));
    assert!(matches!(browser.version().await, Err(Error::Closed)));

    let Some((chromium, _profile)) = common::chromium().await else {
        return;
    };
    let pid = chromium.pid().expect("pid");
    drop(chromium);
    let mut gone = false;
    for _ in 0..100 {
        if process_is_gone(pid) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone, "Chromium {pid} survived its handle");
}
