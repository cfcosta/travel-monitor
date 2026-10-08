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
            // Paying in miles, on LIS business, a deal in miles only
            // (0083).
            cx.update_window(window.into(), |_, window, cx| {
                view.update(cx, |tm, cx| {
                    tm.deals_map.set_view(crate::geo::GlobeView {
                        lon: -30.0,
                        lat: 0.0,
                        zoom: 1.0,
                    });
                    tm.pay = super::PayWith::Miles;
                    tm.list_tab = super::ListTab::All;
                    let key = tm.snapshot.as_ref().and_then(|s| {
                        s.rows
                            .iter()
                            .find(|r| {
                                r.code() == "LIS"
                                    && r.cabin == tm_domain::Cabin::Business
                            })
                            .map(|r| r.key())
                    });
                    if let Some(key) = key {
                        tm.select(key, cx);
                    }
                    tm.navigate(Screen::Deals, window, cx);
                });
                window.refresh();
            })?;
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, _| window.refresh())?;
            cx.run_until_parked();
            let image = cx.capture_screenshot(window.into())?;
            let path = dir.join("deals-miles.png");
            image.save(&path)?;
            eprintln!("wrote {}", path.display());
        }
    }
    Ok(())
}

/// A backend that answers every query with one snapshot.
struct Fixed(crate::data::Snapshot);

#[async_trait::async_trait]
impl crate::data::Backend for Fixed {
    async fn snapshot(
        &self,
        _: crate::data::SnapshotQuery,
    ) -> anyhow::Result<crate::data::Snapshot> {
        Ok(self.0.clone())
    }

    fn changes(&self) -> crate::data::Changes {
        crate::data::Changes::channel().1
    }

    async fn enqueue(
        &self,
        _: crate::data::Request,
    ) -> anyhow::Result<crate::data::Ticket> {
        anyhow::bail!("read only")
    }

    async fn ticket(
        &self,
        _: crate::data::Ticket,
    ) -> anyhow::Result<crate::data::TicketState> {
        anyhow::bail!("read only")
    }

    async fn save_settings(
        &self,
        _: tm_domain::Settings,
    ) -> anyhow::Result<()> {
        anyhow::bail!("read only")
    }

    async fn add_place(
        &self,
        _: crate::data::Role,
        _: crate::data::NewPlace,
    ) -> anyhow::Result<()> {
        anyhow::bail!("read only")
    }

    async fn remove_place(
        &self,
        _: crate::data::Role,
        _: tm_domain::PlaceId,
    ) -> anyhow::Result<()> {
        anyhow::bail!("read only")
    }

    async fn set_favorite(
        &self,
        _: tm_domain::PlaceId,
        _: bool,
    ) -> anyhow::Result<()> {
        anyhow::bail!("read only")
    }

    async fn add_pos(&self, _: crate::data::NewPos) -> anyhow::Result<()> {
        anyhow::bail!("read only")
    }

    async fn set_pos_active(
        &self,
        _: tm_domain::PosId,
        _: bool,
    ) -> anyhow::Result<()> {
        anyhow::bail!("read only")
    }

    async fn save_program(
        &self,
        _: String,
        _: tm_domain::ProgramSettings,
    ) -> anyhow::Result<()> {
        anyhow::bail!("read only")
    }

    async fn set_site_limits(
        &self,
        _: String,
        _: tm_domain::SiteLimits,
    ) -> anyhow::Result<()> {
        anyhow::bail!("read only")
    }
}

/// Times frames (render, layout and paint, not the GPU) of each screen
/// over a copy of a real database:
///
/// ```sh
/// TM_BENCH_DB=<copy> cargo test --release -p tm-app --bin tm -- \
///     --ignored frame_times --nocapture
/// ```
#[test]
#[ignore = "needs TM_BENCH_DB, a copy of a real database"]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point"
)]
fn frame_times() -> anyhow::Result<()> {
    let Some(path) = std::env::var_os("TM_BENCH_DB") else {
        return Ok(());
    };
    let runtime = tokio::runtime::Runtime::new()?;
    let store =
        crate::data::store::backend(path.into(), runtime.handle().clone());
    let mut snapshot = runtime.block_on(store.snapshot(Default::default()))?;
    let first = snapshot.rows.first().map(|r| r.key());
    snapshot =
        runtime.block_on(store.snapshot(crate::data::SnapshotQuery {
            origin: snapshot.origin,
            selected: first,
            ..Default::default()
        }))?;
    let now = snapshot.taken_at;
    let text = Arc::new(gpui_wgpu::CosmicTextSystem::new("DejaVu Sans"));
    let mut cx = HeadlessAppContext::with_platform(
        text,
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        crate::theme::Fonts::install(cx);
        super::text_field::bind_keys(cx);
        cx.set_reduce_motion(true);
    });
    let env = AppEnv {
        backend: Arc::new(Fixed(snapshot)),
        login: LoginConfig::resolve(None, None),
        tokio: None,
        refresh_every: Duration::from_secs(600),
        debounce: crate::live::Debounce::default(),
        clock: crate::live::Clock::From(now),
    };
    let window = cx.open_window(size(px(1440.), px(900.)), |_, cx| {
        cx.new(|cx| TravelMonitor::new(env, cx))
    })?;
    let view = cx.update(|cx| window.root(cx))?;
    cx.run_until_parked();
    let frames = |cx: &mut HeadlessAppContext,
                  label: &str,
                  change: &dyn Fn(&mut TravelMonitor, usize)|
     -> anyhow::Result<()> {
        let mut times = Vec::new();
        for i in 0..30 {
            let took = cx.update_window(window.into(), |_, window, cx| {
                view.update(cx, |tm, cx| {
                    change(tm, i);
                    cx.notify();
                });
                let started = std::time::Instant::now();
                window.draw(cx).clear(cx);
                started.elapsed()
            })?;
            times.push(took);
        }
        times.sort();
        eprintln!(
            "{label}: median {:?}, p90 {:?}",
            times[times.len() / 2],
            times[times.len() * 9 / 10]
        );
        Ok(())
    };
    let screens = [
        ("deals, deals tab", Screen::Deals, super::ListTab::Deals),
        ("deals, all routes", Screen::Deals, super::ListTab::All),
        ("routes", Screen::Routes, super::ListTab::Deals),
        ("detail", Screen::Detail, super::ListTab::Deals),
        ("activity", Screen::Activity, super::ListTab::Deals),
    ];
    for (label, screen, tab) in screens {
        cx.update_window(window.into(), |_, window, cx| {
            view.update(cx, |tm, cx| {
                tm.list_tab = tab;
                tm.navigate(screen, window, cx);
            });
        })?;
        cx.run_until_parked();
        frames(&mut cx, label, &|_, _| {})?;
    }
    cx.update_window(window.into(), |_, window, cx| {
        view.update(cx, |tm, cx| {
            tm.list_tab = super::ListTab::All;
            tm.navigate(Screen::Deals, window, cx);
        });
    })?;
    frames(&mut cx, "deals, all routes, turning the globe", &|tm, i| {
        tm.deals_map.set_view(crate::geo::GlobeView {
            lon: -47.0 + i as f32,
            lat: -10.0,
            zoom: 1.5,
        });
    })?;
    Ok(())
}
