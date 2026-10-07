//! The mouse on a local page that logs every pointer event: on the
//! offscreen display (real Wayland pointer input, against the
//! machine's GPU) and, headless, through CDP. Temporary profiles only.
//! Skips, saying so, without `TM_CHROMIUM`, a render node or
//! `$XDG_RUNTIME_DIR`.

#![allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on: a test is the runtime's entry point"
)]

use std::time::Duration;

use serde::Deserialize;
use tm_browser::{Chromium, ChromiumConfig, Display, Mouse, Page};

const LOAD: Duration = Duration::from_secs(20);

/// A tall page with a button in view and one far below, which logs
/// every mouse and wheel event with where it landed.
const PAGE: &str = r#"data:text/html,<!doctype html><title>mouse</title>
<style>
  body { margin: 0; height: 3000px; font: 16px sans-serif }
  button { position: absolute; width: 160px; height: 48px; border: 0; background: rgb(200, 200, 200) }
  button:hover { background: rgb(255, 0, 0) }
  %23near { left: 420px; top: 300px }
  %23far { left: 700px; top: 2300px }
</style>
<button id=near>Search</button><button id=far>Far</button>
<script>
  window.events = [];
  for (const type of ["mousemove", "mouseover", "mousedown", "mouseup", "click", "wheel"])
    addEventListener(type, (e) => events.push({
      type: e.type, x: e.clientX, y: e.clientY, trusted: e.isTrusted,
      target: e.target.id || e.target.tagName.toLowerCase(), time: e.timeStamp,
    }), { capture: true, passive: true });
</script>"#;

#[derive(Debug, Clone, Deserialize)]
struct Logged {
    #[serde(rename = "type")]
    kind: String,
    x: f64,
    y: f64,
    trusted: bool,
    target: String,
    time: f64,
}

#[derive(Debug, Deserialize)]
struct Box {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn chromium_binary() -> Option<std::ffi::OsString> {
    let binary = std::env::var_os("TM_CHROMIUM").filter(|v| !v.is_empty());
    if binary.is_none() {
        eprintln!(
            "skipping: TM_CHROMIUM is not set (run inside `nix develop`)"
        );
    }
    binary
}

fn offscreen_config(profile: &std::path::Path) -> Option<ChromiumConfig> {
    let binary = chromium_binary()?;
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR is not set");
        return None;
    }
    if tm_compositor::find_render_node(None).is_err() {
        eprintln!("skipping: no GPU render node");
        return None;
    }
    let mut config = ChromiumConfig::new(binary, profile);
    config.display = Display::Offscreen;
    Some(config)
}

async fn events(page: &Page) -> Vec<Logged> {
    page.eval_as("events.splice(0)").await.expect("events")
}

/// The events so far, once one of `kind` is among them: a real
/// pointer's events reach the page a moment after they are sent.
async fn events_through(page: &Page, kind: &str) -> Vec<Logged> {
    page.wait_until(
        &format!("events.some((e) => e.type === '{kind}')"),
        Duration::from_secs(5),
        Duration::from_millis(20),
    )
    .await
    .unwrap_or_else(|e| panic!("no {kind} event: {e}"));
    tokio::time::sleep(Duration::from_millis(50)).await;
    events(page).await
}

async fn rect(page: &Page, selector: &str) -> Box {
    page.eval_as(&format!(
        "(() => {{ const r = document.querySelector('{selector}').getBoundingClientRect(); \
         return {{ x: r.x, y: r.y, width: r.width, height: r.height }}; }})()"
    ))
    .await
    .expect("rect")
}

fn inside(event: &Logged, r: &Box) -> bool {
    event.x >= r.x
        && event.x <= r.x + r.width
        && event.y >= r.y
        && event.y <= r.y + r.height
}

/// Moves, hovers, clicks and scrolls, then checks what the page saw.
async fn exercise(page: &Page, mouse: &Mouse) {
    page.navigate(PAGE, LOAD).await.expect("navigate");
    page.wait_until(
        "document.readyState === 'complete' && window.events",
        LOAD,
        Duration::from_millis(50),
    )
    .await
    .expect("page ready");

    // A move to a viewport point arrives at that point: the mapping
    // from CSS pixels to the output is right.
    for (x, y) in [(123.0, 234.0), (1010.0, 655.0)] {
        mouse.move_to(page, x, y).await.expect("move");
        // Wait for the last motion to arrive (a miss shows below).
        let _ = page
            .wait_until(
                &format!(
                    "events.some((e) => e.type === 'mousemove' \
                     && Math.abs(e.x - {x}) <= 1 && Math.abs(e.y - {y}) <= 1)"
                ),
                Duration::from_secs(3),
                Duration::from_millis(20),
            )
            .await;
        let moved = events(page).await;
        let last = moved
            .iter()
            .rev()
            .find(|e| e.kind == "mousemove")
            .expect("mousemove events");
        eprintln!("{} events moving to ({x}, {y}); last {last:?}", moved.len());
        assert!(
            (last.x - x).abs() <= 1.0 && (last.y - y).abs() <= 1.0,
            "the move to ({x}, {y}) landed at ({}, {})",
            last.x,
            last.y
        );
    }
    let window: serde_json::Value = page
        .eval("({ screenX, screenY, outerWidth, outerHeight, innerWidth, innerHeight, devicePixelRatio })")
        .await
        .expect("window");
    eprintln!("window: {window}");

    // Hovering applies :hover.
    mouse.hover(page, "#near").await.expect("hover");
    let _ = page
        .wait_until(
            "document.querySelector('#near').matches(':hover')",
            Duration::from_secs(3),
            Duration::from_millis(20),
        )
        .await;
    let hovered: (bool, String) = page
        .eval_as(
            "(() => { const b = document.querySelector('#near'); \
             return [b.matches(':hover'), getComputedStyle(b).backgroundColor]; })()",
        )
        .await
        .expect("hover state");
    assert_eq!(hovered, (true, "rgb(255, 0, 0)".to_owned()));
    events(page).await;

    // A click: many moves, then down, up and click inside the button,
    // all trusted.
    mouse.move_to(page, 60.0, 600.0).await.expect("move away");
    events(page).await;
    mouse.click(page, "#near").await.expect("click");
    let clicked = events_through(page, "click").await;
    let near = rect(page, "#near").await;
    let click = clicked
        .iter()
        .position(|e| e.kind == "click")
        .expect("a click");
    let moves = clicked[..click]
        .iter()
        .filter(|e| e.kind == "mousemove")
        .count();
    let down = clicked
        .iter()
        .find(|e| e.kind == "mousedown")
        .expect("down");
    let up = clicked.iter().find(|e| e.kind == "mouseup").expect("up");
    eprintln!(
        "click: {moves} moves first, at ({}, {}), held {:.0} ms, button {near:?}",
        clicked[click].x,
        clicked[click].y,
        up.time - down.time
    );
    assert!(moves >= 10, "only {moves} mousemoves before the click");
    assert!(clicked.iter().all(|e| e.trusted), "an untrusted event");
    assert_eq!(clicked[click].target, "near");
    assert!(inside(&clicked[click], &near), "{:?}", clicked[click]);
    let held = up.time - down.time;
    assert!((40.0..=200.0).contains(&held), "held {held} ms");

    // A click far below scrolls there with the wheel first.
    mouse.click(page, "#far").await.expect("click far");
    let far_events = events_through(page, "click").await;
    let scrolled: f64 = page.eval_as("scrollY").await.expect("scrollY");
    let wheels = far_events.iter().filter(|e| e.kind == "wheel").count();
    let far_click = far_events
        .iter()
        .find(|e| e.kind == "click")
        .expect("a click on #far");
    eprintln!("far: {wheels} wheel events, scrollY {scrolled}");
    assert!(wheels > 0, "no wheel events");
    assert!(scrolled > 1000.0, "the page scrolled to {scrolled}");
    assert_eq!(far_click.target, "far");
    assert!(inside(far_click, &rect(page, "#far").await));
    assert!(far_events.iter().all(|e| e.trusted), "an untrusted event");

    // Wandering moves a little, sometimes.
    mouse.wander_for(page, Duration::from_secs(4)).await;
    let wandered = events(page).await;
    eprintln!("wandering: {} events in 4 s", wandered.len());

    let webdriver: bool = page
        .eval_as("navigator.webdriver")
        .await
        .expect("webdriver");
    assert!(!webdriver, "navigator.webdriver is set");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_offscreen_mouse_is_a_real_pointer() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let Some(config) = offscreen_config(&dir.path().join("profile")) else {
        return;
    };
    let chromium = Chromium::launch(config).await.expect("Chromium starts");
    let mouse = Mouse::new(chromium.pointer(), 7);
    assert!(mouse.is_real());
    let page = chromium.browser().new_page().await.expect("page");
    exercise(&page, &mouse).await;
    page.close().await.expect("close page");
    chromium.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_headless_mouse_moves_the_same_way_through_devtools() {
    let Some(binary) = chromium_binary() else {
        return;
    };
    let dir = tempfile::tempdir().expect("temporary directory");
    let chromium =
        Chromium::launch(ChromiumConfig::new(binary, dir.path().join("p")))
            .await
            .expect("Chromium starts");
    let mouse = chromium.mouse();
    assert!(!mouse.is_real());
    let page = chromium.browser().new_page().await.expect("page");
    exercise(&page, &mouse).await;
    page.close().await.expect("close page");
    chromium.shutdown().await.expect("shutdown");
}
