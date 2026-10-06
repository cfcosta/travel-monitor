//! What the daemon did: recent queries with why they ran and how they
//! ended (0028), and the blocks it met (0039). While a check runs, a
//! "Now checking" row heads the log, and queries that land are tinted
//! for a moment (0057).

use chrono::{DateTime, Utc};
use gpui::{AnyElement, Context, Div, FontWeight, div, prelude::*, px};

use super::{
    TravelMonitor,
    widgets::{self, mono},
};
use crate::{
    data::{ActivityRow, CurrentCheck, Live, Tone},
    format,
    live::Fresh,
    theme::{self, Fonts, alpha, color},
};

/// The query log as a table; `with_route` adds the route column.
/// `checking` heads it with the check in progress (its own running row
/// is left out), and `glow` tints rows that just landed.
pub(super) fn activity_table(
    fonts: &Fonts,
    rows: &[ActivityRow],
    with_route: bool,
    checking: Option<(&CurrentCheck, DateTime<Utc>)>,
    glow: impl Fn(i64) -> f32,
) -> Div {
    let line = || div().relative().flex().gap(px(12.)).px(px(16.)).py(px(9.));
    let header = line()
        .text_size(px(12.))
        .text_color(color(theme::TEXT_LABEL))
        .child(div().w(px(70.)).flex_none().child("Time"))
        .when(with_route, |l| {
            l.child(div().w(px(120.)).flex_none().child("Route"))
        })
        .child(div().flex_1().min_w_0().child("Query"))
        .child(div().flex_1().min_w_0().child("Why"))
        .child(div().flex_1().min_w_0().child("Result"))
        .child(div().w(px(60.)).flex_none().text_right().child("Loads"));
    let now_row = checking.map(|(c, now)| {
        let what = [
            Some(capitalized(&c.kind)),
            Some(c.cabin.label().to_owned()),
            Some(c.pos.clone()),
            c.site.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        line()
            .border_t_1()
            .border_color(color(theme::ROW_RULE_FAINT))
            .bg(alpha(theme::TEAL, 0.06))
            .text_size(px(13.))
            .debug_selector(|| "now-checking".into())
            .child(
                mono(fonts, format::elapsed(now, c.started_at))
                    .w(px(70.))
                    .flex_none()
                    .text_color(color(theme::TEAL)),
            )
            .when(with_route, |l| {
                l.child(
                    div()
                        .w(px(120.))
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .child(c.route.replace('→', " → ")),
                )
            })
            .child(div().flex_1().min_w_0().truncate().child(what))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color(theme::TEXT_MUTED))
                    .child(c.reason.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_color(color(theme::TEAL))
                    .child(widgets::pulse(
                        "now-checking-pulse",
                        widgets::dot(theme::TEAL, 6., true),
                    ))
                    .child("Now checking"),
            )
            .child(
                mono(fonts, "…")
                    .w(px(60.))
                    .flex_none()
                    .text_right()
                    .text_color(color(theme::TEXT_FAINT)),
            )
    });
    let hidden = checking.and_then(|(c, _)| c.query);
    let body = rows.iter().filter(|r| Some(r.id) != hidden).map(|r| {
        let result_color = match r.tone {
            Tone::Deal => theme::YELLOW,
            Tone::Bad => theme::RED,
            Tone::System if r.running => theme::TEAL,
            _ => theme::TEXT_SOFT,
        };
        line()
            .border_t_1()
            .border_color(color(theme::ROW_RULE_FAINT))
            .text_size(px(13.))
            .child(
                mono(fonts, format::clock(r.at))
                    .w(px(70.))
                    .flex_none()
                    .text_color(color(theme::TEXT_FAINT)),
            )
            .when(with_route, |l| {
                l.child(
                    div()
                        .w(px(120.))
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .child(r.route.clone()),
                )
            })
            .child(div().flex_1().min_w_0().truncate().child(r.query.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color(theme::TEXT_MUTED))
                    .child(r.reason.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color(result_color))
                    .child(r.result.clone()),
            )
            .child(
                mono(
                    fonts,
                    r.loads.map_or_else(|| "…".to_owned(), |l| l.to_string()),
                )
                .w(px(60.))
                .flex_none()
                .text_right()
                .text_color(color(theme::TEXT_FAINT)),
            )
            .children(widgets::fresh_tint(glow(r.id)))
    });
    let empty = (rows.is_empty() && now_row.is_none()).then(|| {
        line()
            .border_t_1()
            .border_color(color(theme::ROW_RULE_FAINT))
            .text_color(color(theme::TEXT_FAINT))
            .child("No checks yet.")
    });
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(color(theme::BORDER))
        .rounded(px(12.))
        .overflow_hidden()
        .child(header)
        .children(empty)
        .children(now_row)
        .children(body)
}

/// `Outbound`, `Round trip 7 d`.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

impl TravelMonitor {
    pub(super) fn render_activity(
        &mut self,
        fonts: &Fonts,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(s) = &self.snapshot else {
            return div().into_any_element();
        };
        let d = &s.daemon;
        let now = self.now;
        let live = d.live(now);
        let health = match live {
            Live::Missing => ("Not seen", theme::RED),
            Live::Stale(_) => ("Stale", theme::YELLOW),
            Live::Paused(_) => ("Paused", theme::YELLOW),
            Live::Checking(_) | Live::Idle(_) => ("Running", theme::TEAL),
        };
        let (doing, doing_sub) = match live {
            Live::Checking(c) => (
                "Checking".to_owned(),
                format!("{} · {}", c.route, format::elapsed(now, c.started_at)),
            ),
            Live::Idle(Some(at)) => (
                "Idle".to_owned(),
                format!("next in {}", format::countdown(now, at)),
            ),
            Live::Idle(None) => ("Idle".to_owned(), String::new()),
            Live::Paused(state) => ("Paused".to_owned(), state.to_owned()),
            Live::Stale(_) | Live::Missing => {
                ("–".to_owned(), d.next_check.clone().unwrap_or_default())
            }
        };
        let checking = d.checking().map(|c| (c, now));
        let loads_hour: u32 = s
            .activity
            .iter()
            .filter(|a| (now - a.at).num_minutes() < 60)
            .filter_map(|a| a.loads)
            .sum();
        let tiles = div()
            .flex()
            .gap(px(10.))
            .child(
                widgets::stat_tile(
                    fonts,
                    "Daemon",
                    div().text_color(color(health.1)).child(health.0),
                    d.heartbeat
                        .map(|at| format!("heartbeat {}", format::ago(now, at)))
                        .unwrap_or_else(|| "no heartbeat".to_owned()),
                    theme::TEXT,
                )
                .flex_1(),
            )
            .child(
                widgets::stat_tile(
                    fonts,
                    "Page loads today",
                    div().flex().child(d.loads_today.to_string()).child(
                        div()
                            .text_color(color(theme::TEXT_DIM))
                            .child(format!("/{}", d.budget)),
                    ),
                    format!("{loads_hour} in the last hour"),
                    theme::TEXT,
                )
                .flex_1(),
            )
            .child(
                widgets::stat_tile(
                    fonts,
                    "Blocks today",
                    d.blocks_today.to_string(),
                    format!("{} recorded", s.blocks.len()),
                    if d.blocks_today > 0 {
                        theme::RED
                    } else {
                        theme::TEXT
                    },
                )
                .flex_1(),
            )
            .child(
                widgets::stat_tile(
                    fonts,
                    "Now",
                    doing,
                    doing_sub,
                    if checking.is_some() {
                        theme::TEAL
                    } else {
                        theme::TEXT
                    },
                )
                .flex_1(),
            );
        let blocks = s.blocks.iter().map(|b| {
            div()
                .flex()
                .gap(px(12.))
                .px(px(16.))
                .py(px(9.))
                .border_t_1()
                .border_color(color(theme::ROW_RULE_FAINT))
                .text_size(px(13.))
                .child(
                    mono(
                        fonts,
                        format!(
                            "{} {}",
                            format::short_date(b.at.date_naive()),
                            format::clock(b.at)
                        ),
                    )
                    .w(px(110.))
                    .flex_none()
                    .text_color(color(theme::TEXT_FAINT)),
                )
                .child(div().w(px(140.)).flex_none().child(b.site.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(color(theme::RED))
                        .child(b.detail.clone()),
                )
                .child(
                    mono(
                        fonts,
                        format!("{} loads/h before", b.loads_last_hour),
                    )
                    .flex_none()
                    .text_color(color(theme::TEXT_FAINT)),
                )
        });
        let no_blocks = s.blocks.is_empty().then(|| {
            div()
                .px(px(16.))
                .py(px(9.))
                .border_t_1()
                .border_color(color(theme::ROW_RULE_FAINT))
                .text_color(color(theme::TEXT_FAINT))
                .child("No blocks so far.")
        });
        div()
            .id("activity-screen")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(px(28.))
            .py(px(22.))
            .child(widgets::heading("Activity", 22.))
            .child(tiles)
            .child(widgets::heading("Recent queries", 15.))
            .child(activity_table(fonts, &s.activity, true, checking, |id| {
                self.glow(Fresh::Query(id))
            }))
            .child(widgets::heading("Blocks", 15.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(color(theme::BORDER))
                    .rounded(px(12.))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .px(px(16.))
                            .py(px(9.))
                            .text_size(px(12.))
                            .text_color(color(theme::TEXT_LABEL))
                            .child(div().w(px(110.)).flex_none().child("When"))
                            .child(div().w(px(140.)).flex_none().child("Site"))
                            .child(div().flex_1().child("What"))
                            .child(div().child("Rate")),
                    )
                    .children(no_blocks)
                    .children(blocks),
            )
            .into_any_element()
    }
}
