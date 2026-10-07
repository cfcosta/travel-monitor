//! Renders every screen offscreen to PNG, to compare with the designs:
//!
//! ```sh
//! TM_SCREENSHOTS=/tmp/tm cargo test -p tm-app -- --ignored screenshots
//! ```

use std::{path::PathBuf, sync::Arc, time::Duration};

use chrono::{TimeZone, Utc};
use gpui::{AppContext as _, HeadlessAppContext, px, size};

use super::{AppEnv, Screen, TravelMonitor};
use crate::{data::fake::FakeBackend, login::LoginConfig};

#[test]
#[ignore = "renders on the GPU; run with --ignored to write PNGs"]
fn screenshots() -> anyhow::Result<()> {
    let dir = std::env::var_os("TM_SCREENSHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("tm-screenshots"));
    std::fs::create_dir_all(&dir)?;
    let text = Arc::new(gpui_wgpu::CosmicTextSystem::new("DejaVu Sans"));
    let mut cx = HeadlessAppContext::with_platform(
        text,
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        crate::theme::Fonts::install(cx);
        super::text_field::bind_keys(cx);
    });
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 15, 0, 0).unwrap();
    let env = AppEnv {
        backend: Arc::new(FakeBackend::at(now)),
        login: LoginConfig::resolve(None, None),
        tokio: None,
        refresh_every: Duration::from_secs(30),
        debounce: crate::live::Debounce::default(),
        clock: crate::live::Clock::From(now),
    };
    // Every screen at the designs' size, then the main screen at other
    // shapes: the map must fill tall, wide and small windows alike.
    let screens = [
        ("deals", Screen::Deals),
        ("routes", Screen::Routes),
        ("detail", Screen::Detail),
        ("activity", Screen::Activity),
        ("settings", Screen::Settings),
    ];
    type Shots<'a> = &'a [(&'a str, Screen)];
    let sizes: [(f32, f32, Shots); 4] = [
        (1440., 900., &screens),
        (1770., 2000., &screens[..1]),
        (2560., 1080., &screens[..1]),
        (1024., 768., &screens[..1]),
    ];
    for (w, h, shots) in sizes {
        let env = env.clone();
        let window = cx.open_window(size(px(w), px(h)), |_, cx| {
            cx.new(|cx| TravelMonitor::new(env, cx))
        })?;
        let view = cx.update(|cx| window.root(cx))?;
        cx.run_until_parked();
        // The designs open on LIS.
        cx.update(|cx| {
            view.update(cx, |tm, cx| {
                let lis = tm.snapshot.as_ref().and_then(|s| {
                    s.rows.iter().find(|r| r.code() == "LIS").map(|r| r.key())
                });
                if let Some(lis) = lis {
                    tm.select(lis, cx);
                }
            })
        });
        cx.run_until_parked();
        for (name, screen) in shots {
            cx.update_window(window.into(), |_, window, cx| {
                view.update(cx, |tm, cx| tm.navigate(*screen, window, cx));
                window.refresh();
            })?;
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, _| window.refresh())?;
            cx.run_until_parked();
            let image = cx.capture_screenshot(window.into())?;
            let file = if (w, h) == (1440., 900.) {
                format!("{name}.png")
            } else {
                format!("{name}-{w}x{h}.png")
            };
            let path = dir.join(file);
            image.save(&path)?;
            eprintln!("wrote {}", path.display());
        }
        if (w, h) == (1440., 900.) {
            // The Deals globe turned to Europe and zoomed in (0074): it stays
            // inside its pane.
            cx.update_window(window.into(), |_, window, cx| {
                view.update(cx, |tm, cx| {
                    tm.deals_map.set_view(crate::geo::GlobeView {
                        lon: 10.0,
                        lat: 30.0,
                        zoom: 2.5,
                    });
                    tm.navigate(Screen::Deals, window, cx);
                });
                window.refresh();
            })?;
            cx.run_until_parked();
            let image = cx.capture_screenshot(window.into())?;
            let path = dir.join("deals-zoomed.png");
            image.save(&path)?;
            eprintln!("wrote {}", path.display());
        }
    }
    Ok(())
}
