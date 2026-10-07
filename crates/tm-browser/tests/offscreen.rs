//! A headed Chromium on its own offscreen compositor (0081), against
//! the machine's real GPU and a local `data:` page, with a temporary
//! profile. Skips, saying so, without `TM_CHROMIUM`, a render node or
//! `$XDG_RUNTIME_DIR`.

#![allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on: a test is the runtime's entry point"
)]

use std::time::Duration;

use serde::Deserialize;
use tm_browser::{Chromium, ChromiumConfig, Display};

const LOAD: Duration = Duration::from_secs(20);

/// A page that draws with WebGL and reports what a page can see of its
/// display.
const PAGE: &str = "data:text/html,<title>offscreen</title><canvas id=c width=64 height=64></canvas>";

/// Waits for three animation frames (which only come after the
/// compositor drew a frame), then reads the WebGL renderer and the
/// display's properties.
const PROBE_JS: &str = r#"(async () => {
  const frame = () => new Promise(r => requestAnimationFrame(r));
  for (let i = 0; i < 3; i++) await frame();
  const gl = document.getElementById('c').getContext('webgl');
  const info = gl && gl.getExtension('WEBGL_debug_renderer_info');
  gl.clearColor(0, 0.5, 1, 1);
  gl.clear(gl.COLOR_BUFFER_BIT);
  return {
    renderer: info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : null,
    vendor: info ? gl.getParameter(info.UNMASKED_VENDOR_WEBGL) : null,
    userAgent: navigator.userAgent,
    visibility: document.visibilityState,
    focused: document.hasFocus(),
    width: window.innerWidth,
    screen: [screen.width, screen.height],
    finePointer: matchMedia('(pointer: fine)').matches,
    hover: matchMedia('(hover: hover)').matches,
    webdriver: navigator.webdriver,
  };
})()"#;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code, reason = "printed in the test's output")]
struct Probe {
    renderer: Option<String>,
    vendor: Option<String>,
    user_agent: String,
    visibility: String,
    focused: bool,
    width: u32,
    screen: (u32, u32),
    fine_pointer: bool,
    hover: bool,
    webdriver: bool,
}

fn offscreen_config(profile: &std::path::Path) -> Option<ChromiumConfig> {
    let Some(binary) =
        std::env::var_os("TM_CHROMIUM").filter(|v| !v.is_empty())
    else {
        eprintln!(
            "skipping: TM_CHROMIUM is not set (run inside `nix develop`)"
        );
        return None;
    };
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

#[tokio::test(flavor = "multi_thread")]
async fn a_headed_chromium_renders_offscreen_on_the_gpu() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let Some(config) = offscreen_config(&dir.path().join("profile")) else {
        return;
    };
    let chromium = Chromium::launch(config).await.expect("Chromium starts");
    let socket = chromium.wayland_display().expect("a display").to_owned();

    let page = chromium.browser().new_page().await.expect("page");
    page.navigate(PAGE, LOAD).await.expect("navigate");
    let probe: Probe = tokio::time::timeout(LOAD, page.eval_as(PROBE_JS))
        .await
        .expect("animation frames arrive")
        .expect("probe");
    eprintln!("{probe:#?}");

    let renderer = probe
        .renderer
        .as_deref()
        .expect("WebGL debug renderer info");
    let software = ["SwiftShader", "llvmpipe", "softpipe"];
    assert!(
        !software.iter().any(|s| renderer.contains(s)),
        "software rendering: {renderer}"
    );
    assert!(
        !probe.user_agent.contains("Headless"),
        "{}",
        probe.user_agent
    );
    assert_eq!(probe.visibility, "visible");
    assert_eq!(probe.screen, (1440, 1000));
    assert!(probe.fine_pointer && probe.hover, "{probe:?}");

    let shot = page.screenshot().await.expect("screenshot");
    assert!(shot.starts_with(b"\x89PNG"), "a PNG");
    page.close().await.expect("close page");

    chromium.shutdown().await.expect("shutdown");
    let left = std::path::Path::new(
        &std::env::var_os("XDG_RUNTIME_DIR").expect("runtime dir"),
    )
    .join(&socket);
    assert!(!left.exists(), "{} is left behind", left.display());
}
