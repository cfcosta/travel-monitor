//! The window against the fake backend: loading, selection, filters,
//! notifications, requests, links and settings.

use std::{rc::Rc, sync::Arc, time::Duration};

use chrono::{DateTime, TimeZone, Utc};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use tm_domain::Cabin;

use super::{AppEnv, ListTab, Screen, TravelMonitor, settings::Field};
use crate::{
    data::{Request, fake::FakeBackend},
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
        refresh_every: Duration::from_secs(5),
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
