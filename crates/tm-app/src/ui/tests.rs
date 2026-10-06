//! The window against the fake backend: loading, selection, filters,
//! notifications, requests, links and settings.

use std::{rc::Rc, sync::Arc, time::Duration};

use chrono::{DateTime, TimeZone, Utc};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use tm_domain::{Cabin, DealId};

use super::{
    AppEnv,
    ListTab,
    Screen,
    TravelMonitor,
    settings::Field,
    topbar::status_line,
};
use crate::{
    data::{
        ActivityRow,
        CurrentCheck,
        DaemonStatus,
        Request,
        RowStatus,
        Tone,
        fake::FakeBackend,
    },
    live::{self, Clock, Debounce, Fresh},
    login::LoginConfig,
    notify::Recorder,
};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 6, 15, 0, 0).unwrap()
}

struct Opened {
    view: Entity<TravelMonitor>,
    cx: VisualTestContext,
    fake: Arc<FakeBackend>,
    notices: Rc<Recorder>,
}

fn open(cx: &mut TestAppContext, fake: FakeBackend) -> Opened {
    cx.update(|cx| {
        crate::theme::Fonts::install(cx);
        super::text_field::bind_keys(cx);
        // The pulse would draw 15 frames a second of fake time.
        cx.set_reduce_motion(true);
    });
    let fake = Arc::new(fake);
    let notices = Rc::new(Recorder::default());
    let env = AppEnv {
        backend: fake.clone(),
        notifier: notices.clone(),
        login: LoginConfig::resolve(
            Some("/tmp/tm-profile".into()),
            Some("true".into()),
        ),
        tokio: None,
        // Long enough that no test sees the fallback refresh.
        refresh_every: Duration::from_secs(600),
        debounce: Debounce::default(),
        clock: Clock::From(now()),
    };
    let window = cx.add_window(|_, cx| TravelMonitor::new(env, cx));
    let view = window.root(cx).expect("root");
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    Opened {
        view,
        cx,
        fake,
        notices,
    }
}

#[gpui::test]
fn opens_on_the_biggest_deal(cx: &mut TestAppContext) {
    let Opened { view, mut cx, .. } = open(cx, FakeBackend::at(now()));
    view.read_with(&cx, |tm, _| {
        let s = tm.snapshot.as_ref().expect("snapshot");
        assert_eq!(s.rows.len(), 16);
        let key = tm.selected.expect("a row is selected");
        // Sorted by biggest drop: SSA, −27%.
        assert_eq!(s.row(key).map(|r| r.code()), Some("SSA"));
        assert_eq!(s.detail.as_ref().map(|d| d.key), Some(key));
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("detail-card").is_some());
    assert!(cx.debug_bounds("trip-grid").is_some());
    assert!(cx.debug_bounds("sellers").is_some());
}

#[gpui::test]
fn tabs_and_filters_choose_the_rows(cx: &mut TestAppContext) {
    let Opened { view, mut cx, .. } = open(cx, FakeBackend::at(now()));
    let count = |cx: &mut VisualTestContext| {
        view.read_with(cx, |tm, _| tm.visible_rows().len())
    };
    assert_eq!(count(&mut cx), 6);
    view.update(&mut cx, |tm, _| tm.list_tab = ListTab::All);
    assert_eq!(count(&mut cx), 16);
    view.update(&mut cx, |tm, _| tm.cabin = Some(Cabin::Business));
    assert_eq!(count(&mut cx), 1);
}

#[gpui::test]
fn clicking_a_row_selects_it(cx: &mut TestAppContext) {
    let Opened { view, mut cx, .. } = open(cx, FakeBackend::at(now()));
    let lis = view.read_with(&cx, |tm, _| {
        let s = tm.snapshot.as_ref().expect("snapshot");
        s.rows
            .iter()
            .find(|r| r.code() == "LIS")
            .expect("LIS")
            .key()
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let selector = format!("row-{}-{}", lis.route, lis.cabin.code());
    let bounds = cx
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .expect("LIS row drawn");
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.run_until_parked();
    view.read_with(&cx, |tm, _| {
        assert_eq!(tm.selected, Some(lis));
        let detail = tm
            .snapshot
            .as_ref()
            .and_then(|s| s.detail.as_ref())
            .expect("detail");
        assert_eq!(detail.key, lis);
        assert_eq!(detail.sellers.len(), 4);
    });
}

#[gpui::test]
fn a_new_deal_is_notified_once(cx: &mut TestAppContext) {
    let Opened {
        view,
        mut cx,
        notices,
        ..
    } = open(cx, FakeBackend::at(now()).with_unseen("LIS"));
    assert_eq!(notices.shown.borrow().len(), 1);
    assert!(notices.shown.borrow()[0].title.contains("LIS"));
    view.update(&mut cx, |tm, cx| tm.refresh(cx));
    cx.run_until_parked();
    view.update(&mut cx, |tm, cx| tm.refresh(cx));
    cx.run_until_parked();
    assert_eq!(notices.shown.borrow().len(), 1);
    view.read_with(&cx, |tm, _| {
        assert!(
            tm.snapshot.as_ref().is_some_and(|s| s.notices.is_empty()),
            "marked seen"
        );
    });
}

#[gpui::test]
fn buttons_open_the_link_and_ask_the_daemon(cx: &mut TestAppContext) {
    let Opened {
        view, mut cx, fake, ..
    } = open(cx, FakeBackend::at(now()));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("detail-card").is_some());
    view.update(&mut cx, |tm, cx| tm.open_in_google_flights(cx));
    let url = cx.cx.opened_url();
    assert!(url.is_some_and(|u| {
        u.starts_with("https://www.google.com/travel/flights")
    }));
    view.update(&mut cx, |tm, cx| tm.check_now(cx));
    cx.run_until_parked();
    let route = view.read_with(&cx, |tm, _| {
        let s = tm.snapshot.as_ref().expect("snapshot");
        s.row(tm.selected.expect("selected")).expect("row").route.id
    });
    assert_eq!(fake.requests(), vec![Request::CheckRoute(route)]);
    view.read_with(&cx, |tm, _| {
        assert!(
            tm.toast
                .as_ref()
                .is_some_and(|t| t.contains("Check requested"))
        );
    });
}

#[gpui::test]
fn every_screen_draws(cx: &mut TestAppContext) {
    let Opened { view, mut cx, .. } = open(cx, FakeBackend::at(now()));
    for screen in [
        Screen::Routes,
        Screen::Detail,
        Screen::Activity,
        Screen::Settings,
        Screen::Deals,
    ] {
        cx.update(|window, cx| {
            view.update(cx, |tm, cx| tm.navigate(screen, window, cx))
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.read_with(&cx, |tm, _| assert_eq!(tm.screen, screen));
    }
    cx.update(|window, cx| {
        view.update(cx, |tm, cx| tm.navigate(Screen::Detail, window, cx))
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("calendar").is_some());
    assert!(cx.debug_bounds("departure").is_some());
}

#[gpui::test]
fn settings_are_validated_then_saved(cx: &mut TestAppContext) {
    let Opened {
        view, mut cx, fake, ..
    } = open(cx, FakeBackend::at(now()));
    cx.update(|window, cx| {
        view.update(cx, |tm, cx| tm.navigate(Screen::Settings, window, cx))
    });
    cx.run_until_parked();
    let form =
        view.read_with(&cx, |tm, _| tm.settings_form.clone().expect("form"));
    let adults = form.read_with(&cx, |f, _| {
        f.fields
            .iter()
            .find(|(f, _)| *f == Field::Adults)
            .map(|(_, e)| e.clone())
            .expect("adults")
    });
    adults.update(&mut cx, |e, cx| e.set_text("0", cx));
    view.update(&mut cx, |tm, cx| tm.save_settings(cx));
    cx.run_until_parked();
    form.read_with(&cx, |f, _| {
        assert_eq!(f.error.as_deref(), Some("At least one adult must travel"));
    });
    adults.update(&mut cx, |e, cx| e.set_text("2", cx));
    view.update(&mut cx, |tm, cx| tm.save_settings(cx));
    cx.run_until_parked();
    form.read_with(&cx, |f, _| assert!(f.error.is_none()));
    view.read_with(&cx, |tm, _| {
        assert_eq!(
            tm.snapshot.as_ref().map(|s| s.settings.passengers.adults),
            Some(2)
        );
    });
    assert!(fake.requests().contains(&Request::ReloadConfig));
}

// ---------------------------------------------------------------------
// Live updates (0057)
// ---------------------------------------------------------------------

fn go(view: &Entity<TravelMonitor>, cx: &mut VisualTestContext, to: Screen) {
    cx.update(|window, cx| {
        view.update(cx, |tm, cx| tm.navigate(to, window, cx))
    });
    cx.run_until_parked();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn advance(cx: &mut VisualTestContext, by: Duration) {
    cx.executor().advance_clock(by);
    cx.run_until_parked();
}

fn status(view: &Entity<TravelMonitor>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |tm, _| {
        let s = tm.snapshot.as_ref().expect("snapshot");
        status_line(&s.daemon, tm.now).1
    })
}

/// A burst of change notices is one refresh, after the debounce; the
/// query that landed is on the activity screen, tinted, and the tint
/// wears off.
#[gpui::test]
fn a_change_notice_refreshes_the_view(cx: &mut TestAppContext) {
    let Opened {
        view, mut cx, fake, ..
    } = open(cx, FakeBackend::at(now()));
    go(&view, &mut cx, Screen::Activity);
    let before = fake.snapshots();

    let id = fake.land_query("SAO → GIG", "14 prices");
    fake.touch();
    fake.touch();
    cx.run_until_parked();
    assert_eq!(fake.snapshots(), before, "waits for the burst to settle");

    advance(&mut cx, Duration::from_millis(250));
    assert_eq!(fake.snapshots(), before + 1, "one refresh for the burst");
    let at = cx.executor().now();
    view.read_with(&cx, |tm, _| {
        let s = tm.snapshot.as_ref().expect("snapshot");
        assert_eq!(s.activity[0].id, id);
        assert_eq!(s.activity[0].result, "14 prices");
        assert!(tm.fresh.strength(&Fresh::Query(id), at) > 0.8);
        // Rows that were there already are not tinted.
        assert_eq!(tm.fresh.strength(&Fresh::Query(s.activity[1].id), at), 0.0);
    });

    advance(&mut cx, Duration::from_secs(1));
    let at = cx.executor().now();
    view.read_with(&cx, |tm, _| {
        let left = tm.fresh.strength(&Fresh::Query(id), at);
        assert!(left > 0.0 && left < 0.6, "{left}");
    });
    advance(&mut cx, Duration::from_secs(2));
    view.read_with(&cx, |tm, _| assert!(tm.fresh.is_empty(), "expired"));
    assert_eq!(fake.snapshots(), before + 1, "no reads while fading");
}

/// While a check runs the activity screen heads its log with it; when it
/// ends the row goes, the query's result lands tinted, and the chip
/// goes idle.
#[gpui::test]
fn the_check_in_progress_comes_and_goes(cx: &mut TestAppContext) {
    let Opened {
        view, mut cx, fake, ..
    } = open(cx, FakeBackend::at(now()));
    go(&view, &mut cx, Screen::Activity);
    assert!(cx.debug_bounds("now-checking").is_some());
    assert!(
        status(&view, &mut cx)
            .starts_with("checking SAO→MIA · business · outbound · BR · 12 s")
    );
    let running = view.read_with(&cx, |tm, _| {
        let s = tm.snapshot.as_ref().expect("snapshot");
        s.daemon
            .check
            .as_ref()
            .and_then(|c| c.query)
            .expect("its query")
    });

    fake.set_checking(None);
    advance(&mut cx, Duration::from_millis(250));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("now-checking").is_none());
    assert_eq!(
        status(&view, &mut cx),
        "idle · next in 4 min · 212/2400 loads"
    );
    let at = cx.executor().now();
    view.read_with(&cx, |tm, _| {
        assert!(tm.fresh.strength(&Fresh::Query(running), at) > 0.0);
    });
}

/// The one-second clock moves the chip's elapsed time and the countdown
/// without reading anything.
#[gpui::test]
fn the_clock_moves_relative_times(cx: &mut TestAppContext) {
    let Opened {
        view, mut cx, fake, ..
    } = open(cx, FakeBackend::at(now()));
    let before = fake.snapshots();
    assert!(status(&view, &mut cx).ends_with("· 12 s"));
    advance(&mut cx, Duration::from_secs(1));
    assert!(status(&view, &mut cx).ends_with("· 13 s"));
    advance(&mut cx, Duration::from_secs(60));
    assert!(status(&view, &mut cx).ends_with("· 1 min 13 s"));
    assert_eq!(fake.snapshots(), before, "the clock reads nothing");

    // Idle: the countdown runs down, then the heartbeat goes stale.
    fake.set_checking(None);
    advance(&mut cx, Duration::from_millis(250));
    // The fake's next check is 15:04; the clock reads 15:01:01.
    assert_eq!(
        status(&view, &mut cx),
        "idle · next in 3 min · 212/2400 loads"
    );
    advance(&mut cx, Duration::from_secs(30));
    assert_eq!(
        status(&view, &mut cx),
        "idle · next in 2 min · 212/2400 loads"
    );
    advance(&mut cx, Duration::from_secs(30));
    assert_eq!(fake.snapshots(), before + 1);
    // Its heartbeat is from 14:59:56; at 15:02:01 that is stale.
    assert_eq!(
        status(&view, &mut cx),
        "stale · seen 2 min ago · 212/2400 loads"
    );
}

/// New observations for the open route tint its card.
#[gpui::test]
fn new_observations_tint_the_open_route(cx: &mut TestAppContext) {
    let Opened {
        view, mut cx, fake, ..
    } = open(cx, FakeBackend::at(now()));
    let lis = view.read_with(&cx, |tm, _| {
        let s = tm.snapshot.as_ref().expect("snapshot");
        s.rows
            .iter()
            .find(|r| r.code() == "LIS")
            .expect("LIS")
            .key()
    });
    view.update(&mut cx, |tm, cx| tm.select(lis, cx));
    cx.run_until_parked();
    let selected = view.read_with(&cx, |tm, _| tm.selected);

    fake.check_route("LIS");
    advance(&mut cx, Duration::from_millis(250));
    let at = cx.executor().now();
    view.read_with(&cx, |tm, _| {
        assert!(tm.fresh.strength(&Fresh::Route(lis), at) > 0.0);
        assert_eq!(tm.selected, selected, "the selection stays");
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("detail-card").is_some());
}

/// The chip in each state.
#[test]
fn status_lines() {
    let t = now();
    let mut d = DaemonStatus {
        loads_today: 212,
        budget: 1200,
        ..DaemonStatus::default()
    };
    assert_eq!(status_line(&d, t).1, "daemon not seen");
    d.heartbeat = Some(t - chrono::Duration::minutes(5));
    d.state = "running".into();
    assert_eq!(
        status_line(&d, t).1,
        "stale · seen 5 min ago · 212/1200 loads"
    );
    d.heartbeat = Some(t - chrono::Duration::seconds(3));
    d.next_check_at = Some(t + chrono::Duration::seconds(45));
    assert_eq!(status_line(&d, t).1, "idle · next in 45 s · 212/1200 loads");
    d.blocks_today = 2;
    assert!(status_line(&d, t).1.ends_with("· 2 blocks"));
    d.blocks_today = 0;
    d.state = "paused for login: smiles".into();
    assert_eq!(
        status_line(&d, t).1,
        "paused for login: smiles · 212/1200 loads"
    );
    d.state = "checking".into();
    d.check = Some(CurrentCheck {
        query: Some(7),
        route: "SAO→LIS".into(),
        cabin: Cabin::Economy,
        kind: "outbound".into(),
        pos: "BR".into(),
        site: None,
        reason: "Scheduled · score 0.41".into(),
        started_at: t - chrono::Duration::seconds(12),
    });
    let (_, text, checking) = status_line(&d, t);
    assert_eq!(text, "checking SAO→LIS · economy · outbound · BR · 12 s");
    assert!(checking);
    // A long check is not stale while it is younger than ten minutes.
    d.heartbeat = Some(t - chrono::Duration::minutes(4));
    assert!(status_line(&d, t).2);
    // Check columns left by a daemon that no longer says "checking".
    d.state = "running".into();
    d.heartbeat = Some(t);
    assert!(status_line(&d, t).1.starts_with("idle"));
}

/// What counts as new between two snapshots of the same view.
#[gpui::test]
fn arrivals_between_snapshots(cx: &mut TestAppContext) {
    let Opened { view, cx, .. } = open(cx, FakeBackend::at(now()));
    let old =
        view.read_with(&cx, |tm, _| tm.snapshot.clone().expect("snapshot"));
    assert!(live::arrivals(&old, &old).is_empty());

    let mut new = old.clone();
    new.activity.insert(
        0,
        ActivityRow {
            id: 5000,
            at: now(),
            route: "SAO → GIG".into(),
            query: "Return leg · Economy · BR".into(),
            reason: "Random sample".into(),
            result: "running".into(),
            tone: Tone::System,
            loads: None,
            running: true,
        },
    );
    let watching = new
        .rows
        .iter()
        .position(|r| r.status == RowStatus::Watching)
        .expect("a watched row");
    new.rows[watching].status = RowStatus::Deal;
    new.rows[watching].deal = Some(DealId(999));
    let found = live::arrivals(&old, &new);
    assert_eq!(found, vec![Fresh::Deal(DealId(999))], "running is not new");

    let mut landed = new.clone();
    landed.activity[0].running = false;
    landed.activity[0].result = "12 prices".into();
    assert_eq!(live::arrivals(&new, &landed), vec![Fresh::Query(5000)]);

    // Another origin: its deals are not news.
    let mut elsewhere = new.clone();
    elsewhere.origin = None;
    assert!(live::arrivals(&old, &elsewhere).is_empty());
}
