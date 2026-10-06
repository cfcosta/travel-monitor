//! What the daemon did: recent queries with why they ran and how they
//! ended (0028), and the blocks it met (0039).

use gpui::{AnyElement, Context, Div, FontWeight, div, prelude::*, px};

use super::{
    TravelMonitor,
    widgets::{self, mono},
};
use crate::{
    data::{ActivityRow, Health, Tone},
    format,
    theme::{self, Fonts, color},
};

/// The query log as a table; `with_route` adds the route column.
pub(super) fn activity_table(
    fonts: &Fonts,
    rows: &[ActivityRow],
    with_route: bool,
) -> Div {
    let line = || div().flex().gap(px(12.)).px(px(16.)).py(px(9.));
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
    let body = rows.iter().map(|r| {
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
    });
    let empty = rows.is_empty().then(|| {
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
        .children(body)
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
        let health = match d.health(s.taken_at) {
            Health::Running => ("Running", theme::TEAL),
            Health::Stale => ("Stale", theme::YELLOW),
            Health::Missing => ("Not seen", theme::RED),
        };
        let running = s.activity.iter().filter(|a| a.running).count();
        let loads_hour: u32 = s
            .activity
            .iter()
            .filter(|a| (s.taken_at - a.at).num_minutes() < 60)
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
                        .map(|at| {
                            format!("heartbeat {}", format::ago(s.taken_at, at))
                        })
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
                    format!("{running} running"),
                    d.next_check.clone().unwrap_or_else(|| d.state.clone()),
                    theme::TEXT,
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
            .child(activity_table(fonts, &s.activity, true))
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
