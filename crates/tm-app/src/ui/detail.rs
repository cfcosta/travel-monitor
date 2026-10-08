//! The route detail ("I"): six months of departures for one trip
//! length, coloured against the usual price, the selected departure's
//! sellers, and the route's recent checks.

use chrono::Duration;
use gpui::{
    AnyElement,
    Context,
    Div,
    FontWeight,
    SharedString,
    div,
    prelude::*,
    px,
};
use tm_domain::{Currency, Money};

use super::{
    PayWith,
    Screen,
    TravelMonitor,
    deals::{award_dot, sellers_table},
    map::{MapDest, MapModel, MapPlace, map},
    widgets::{self, mono},
};
use crate::{
    data::{
        ActivityRow,
        AwardQuote,
        Login,
        RouteAwards,
        RouteDetail,
        RouteRow,
        Snapshot,
    },
    format,
    geo::Region,
    grid::{CalendarDay, Tone},
    live::Fresh,
    theme::{self, Fonts, color},
};

const CELL: f32 = 22.;
const GAP: f32 = 4.;

fn tone_bg(tone: Tone) -> Option<u32> {
    match tone {
        Tone::Deal => Some(theme::YELLOW),
        Tone::Below => Some(theme::TONE_BELOW),
        Tone::Typical => Some(theme::TONE_TYPICAL),
        Tone::Above => Some(theme::TONE_ABOVE),
        Tone::Unknown => Some(theme::CARD),
        Tone::Past => None,
    }
}

impl TravelMonitor {
    pub(super) fn render_detail(
        &mut self,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(s) = &self.snapshot else {
            return div().into_any_element();
        };
        let row = self.selected.and_then(|k| s.row(k));
        let detail = s.detail.as_ref().filter(|d| Some(d.key) == self.selected);
        let main = match row {
            Some(row) => self
                .render_detail_main(fonts, s, row, detail, cx)
                .into_any_element(),
            None => div()
                .p(px(28.))
                .text_color(color(theme::TEXT_FAINT))
                .child("Pick a route.")
                .into_any_element(),
        };
        div()
            .flex()
            .size_full()
            .min_h_0()
            .child(self.render_rail(fonts, s, cx))
            .child(
                div()
                    .id("detail-main")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .child(main),
            )
            .into_any_element()
    }

    fn render_rail(
        &self,
        fonts: &Fonts,
        s: &Snapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut rows: Vec<&RouteRow> = s.rows.iter().collect();
        super::sort_rows(
            &mut rows,
            super::Sort::BiggestDrop,
            super::PayWith::Both,
        );
        div()
            .id("rail")
            .w(px(220.))
            .flex_none()
            .overflow_y_scroll()
            .py(px(10.))
            .border_r_1()
            .border_color(color(theme::BORDER))
            .bg(color(theme::PANEL))
            .children(rows.into_iter().map(|r| {
                let key = r.key();
                let on = self.selected == Some(key);
                div()
                    .id(SharedString::from(format!(
                        "rail-{}-{}",
                        key.route,
                        key.cabin.code()
                    )))
                    .flex()
                    .justify_between()
                    .items_center()
                    .px(px(16.))
                    .py(px(9.))
                    .cursor_pointer()
                    .relative()
                    .when(on, |d| {
                        d.bg(color(theme::SELECTED)).child(widgets::accent())
                    })
                    .hover(|d| d.bg(color(theme::SELECTED)))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .items_baseline()
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .child(r.code().to_owned()),
                            )
                            .when(r.cabin != tm_domain::Cabin::Economy, |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(color(theme::TEXT_FAINT))
                                        .child(r.cabin.label()),
                                )
                            }),
                    )
                    .child({
                        // A miles-led row in the short form (0083).
                        let award = r
                            .award
                            .as_ref()
                            .filter(|_| r.award_leads(PayWith::Both));
                        let price = match award {
                            Some(a) => format::miles_short(a.best.price.miles),
                            None => r.best.as_ref().map_or_else(
                                || "–".to_owned(),
                                |t| format::money(&t.price),
                            ),
                        };
                        let deal = r.is_deal(PayWith::Both);
                        div()
                            .flex()
                            .gap(px(8.))
                            .items_center()
                            .children(award.map(|_| widgets::miles_tag(fonts)))
                            .child(
                                mono(fonts, price)
                                    .text_size(px(12.))
                                    .text_color(color(theme::TEXT_SOFT)),
                            )
                            .child(
                                mono(
                                    fonts,
                                    r.lead_change(PayWith::Both).map_or_else(
                                        || "–".to_owned(),
                                        format::change,
                                    ),
                                )
                                .text_size(px(11.))
                                .text_color(
                                    color(if deal {
                                        theme::YELLOW
                                    } else {
                                        theme::TEXT_FAINT
                                    }),
                                ),
                            )
                    })
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select(key, cx)),
                    )
            }))
    }

    fn render_detail_main(
        &self,
        fonts: &Fonts,
        s: &Snapshot,
        row: &RouteRow,
        detail: Option<&RouteDetail>,
        cx: &mut Context<Self>,
    ) -> Div {
        let settings = &s.settings;
        let days = self.calendar_days.unwrap_or(settings.trips.round_trip_days);
        let adults = settings.passengers.adults;
        let pos: Vec<&str> = s
            .points_of_sale
            .iter()
            .filter(|p| p.active)
            .map(|p| p.country.as_str())
            .collect();
        let region = if row.route.is_domestic() {
            "Domestic"
        } else {
            "International"
        };
        let head = div()
            .flex_1()
            .min_w(px(380.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .justify_center()
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child(
                        div()
                            .id("crumb-routes")
                            .cursor_pointer()
                            .hover(|d| d.text_color(color(theme::TEAL)))
                            .child("Routes")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.navigate(Screen::Routes, window, cx)
                            })),
                    )
                    .child(format!("/ {region}")),
            )
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(34.))
                            .font_weight(FontWeight::BOLD)
                            .child(format!(
                                "{} → {}",
                                row.route.origin.code,
                                row.code()
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(34.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(theme::TEXT_LABEL))
                            .child(row.city().to_owned()),
                    )
                    .child(self.star(
                        "detail-star".to_owned(),
                        row.place(),
                        26.,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(widgets::pill(format!("{days}-day round trips")))
                    .child(widgets::pill(format!(
                        "{adults} adult{}",
                        if adults == 1 { "" } else { "s" }
                    )))
                    .child(widgets::pill(format!("POS {}", pos.join(" · "))))
                    .children(detail.and_then(|d| d.change_rate).map(|r| {
                        widgets::pill(format!("Changes ≈ {r:.1}/day"))
                    }))
                    .children(
                        s.miles
                            .iter()
                            .filter(|m| m.active() && m.has_reader)
                            .map(|m| {
                                if m.login == Login::LoggedIn {
                                    widgets::pill(format!(
                                        "{} · logged in",
                                        m.name
                                    ))
                                } else {
                                    widgets::pill(format!(
                                        "{} · log in again",
                                        m.name
                                    ))
                                    .border_color(color(theme::YELLOW))
                                    .text_color(color(theme::YELLOW))
                                }
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .mt(px(4.))
                    .when(self.can_check_award(), |d| {
                        d.child(
                            widgets::ghost_button(
                                "detail-check-award",
                                "Check award",
                            )
                            .py(px(8.))
                            .on_click(cx.listener(
                                |this, _, _, cx| this.check_award(cx),
                            )),
                        )
                    })
                    .child(
                        widgets::ghost_button("detail-check", "Check now")
                            .py(px(8.))
                            .on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.check_now(cx)
                                }),
                            ),
                    )
                    .child(
                        widgets::ghost_button("detail-edit", "Edit route")
                            .py(px(8.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.navigate(Screen::Settings, window, cx)
                            })),
                    ),
            );
        let cheapest = detail.and_then(|d| d.calendar.cheapest());
        let money =
            |minor: i64| format::money(&Money::new(minor, Currency::brl()));
        let tiles = div()
            .relative()
            .w(px(360.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(
                        widgets::stat_tile(
                            fonts,
                            "Cheapest now",
                            cheapest.and_then(|c| c.price).map_or_else(
                                || "–".to_owned(),
                                |p| money(p.minor),
                            ),
                            cheapest
                                .map(|c| {
                                    format::date_range(
                                        c.date,
                                        c.date
                                            + Duration::days(i64::from(days)),
                                    )
                                })
                                .unwrap_or_default(),
                            theme::YELLOW,
                        )
                        .flex_1(),
                    )
                    .child(
                        widgets::stat_tile(
                            fonts,
                            "Lowest ever",
                            detail
                                .and_then(|d| d.lowest_ever.as_ref())
                                .map_or_else(
                                    || "–".to_owned(),
                                    |(m, _, _)| format::money(m),
                                ),
                            detail
                                .and_then(|d| d.lowest_ever.as_ref())
                                .map(|(_, at, pos)| {
                                    format!(
                                        "{pos} · {}",
                                        format::short_date(*at)
                                    )
                                })
                                .unwrap_or_default(),
                            theme::TEXT,
                        )
                        .flex_1(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(
                        widgets::stat_tile(
                            fonts,
                            detail.and_then(|d| d.usual.as_ref()).map_or_else(
                                || "Usual".to_owned(),
                                |(_, b)| format!("Usual, {b}"),
                            ),
                            detail.and_then(|d| d.usual.as_ref()).map_or_else(
                                || "–".to_owned(),
                                |(m, _)| format::money(m),
                            ),
                            "60-day median",
                            theme::TEXT,
                        )
                        .flex_1(),
                    )
                    .child(
                        widgets::stat_tile(
                            fonts,
                            "Last checked",
                            row.last_checked.map_or_else(
                                || "never".to_owned(),
                                format::clock,
                            ),
                            detail
                                .and_then(|d| d.next_check)
                                .map(|n| {
                                    format!(
                                        "next in {}",
                                        format::until(self.now, n)
                                    )
                                })
                                .unwrap_or_default(),
                            theme::TEXT,
                        )
                        .flex_1(),
                    ),
            )
            // New observations for this route just arrived (0057).
            .children(widgets::fresh_tint(self.glow(Fresh::Route(row.key()))));
        let route_map = {
            let o = &row.route.origin;
            let d = &row.route.destination;
            div()
                .w(px(240.))
                .h(px(190.))
                .flex_none()
                .rounded(px(14.))
                .overflow_hidden()
                .border_1()
                .border_color(color(theme::BORDER_CARD))
                .bg(color(theme::GROUND))
                .child(map(
                    MapModel {
                        origin: Some(MapPlace {
                            code: o.code.clone().into(),
                            lon: o.lon as f32,
                            lat: o.lat as f32,
                        }),
                        destinations: vec![MapDest {
                            place: MapPlace {
                                code: d.code.clone().into(),
                                lon: d.lon as f32,
                                lat: d.lat as f32,
                            },
                            region: Region::of(
                                &d.country.0,
                                Some(o.country.0.as_str()),
                            ),
                            deal: row.status.is_deal(),
                            selected: true,
                        }],
                        labels: false,
                    },
                    fonts.clone(),
                    None,
                    |_, _, _| {},
                ))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(px(28.))
            .pt(px(20.))
            .pb(px(28.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(20.))
                    .child(head)
                    .child(tiles)
                    .child(route_map),
            )
            .children(self.render_award_tiles(fonts, s, detail, cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(20.))
                    .items_start()
                    .child(self.render_heatmap(s, row, detail, days, cx))
                    .child(
                        self.render_departure(fonts, s, row, detail, days, cx),
                    ),
            )
            .child(render_checks(
                fonts,
                detail.map(|d| d.recent_checks.as_slice()).unwrap_or(&[]),
                |id| self.glow(Fresh::Query(id)),
            ))
    }

    fn render_heatmap(
        &self,
        s: &Snapshot,
        row: &RouteRow,
        detail: Option<&RouteDetail>,
        days: u16,
        cx: &mut Context<Self>,
    ) -> Div {
        // The cabins this route is tracked in, as rows of the snapshot.
        let cabins: Vec<_> = s
            .rows
            .iter()
            .filter(|r| r.route.id == row.route.id)
            .map(|r| r.key())
            .collect();
        let cabin_tabs =
            widgets::segments().children(cabins.into_iter().map(|key| {
                widgets::segment(
                    SharedString::from(format!("cabin-{}", key.cabin.code())),
                    key.cabin.label(),
                    key == row.key(),
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.select(key, cx)),
                )
            }));
        let t = &s.settings.trips;
        let length_tabs =
            widgets::segments().children((t.min_days..=t.max_days).map(|n| {
                widgets::segment(
                    SharedString::from(format!("len-{n}")),
                    format!("{n}d"),
                    n == days,
                )
                .px(px(8.))
                .on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.calendar_days = Some(n);
                        this.refresh(cx);
                        cx.notify();
                    },
                ))
            }));
        let body: AnyElement = match detail {
            None => div()
                .text_color(color(theme::TEXT_FAINT))
                .child("Loading…")
                .into_any_element(),
            Some(d) => {
                let months = div()
                    .flex()
                    .gap(px(GAP))
                    .pl(px(40.))
                    .text_size(px(11.))
                    .text_color(color(theme::TEXT_FAINT))
                    .children(d.calendar.weeks.iter().map(|w| {
                        div()
                            .w(px(CELL))
                            .flex_none()
                            .whitespace_nowrap()
                            .child(w.month.unwrap_or(""))
                    }));
                let weekdays = div()
                    .flex()
                    .flex_col()
                    .gap(px(GAP))
                    .w(px(36.))
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(color(theme::TEXT_FAINT))
                    .children(
                        ["Mon", "", "Wed", "", "Fri", "", "Sun"]
                            .map(|l| div().h(px(CELL)).child(l)),
                    );
                let mut weeks = Vec::with_capacity(d.calendar.weeks.len());
                for (wi, w) in d.calendar.weeks.iter().enumerate() {
                    let mut column = div().flex().flex_col().gap(px(GAP));
                    for (di, day) in w.days.iter().enumerate() {
                        column = column.child(self.day_cell(wi, di, day, cx));
                    }
                    weeks.push(column);
                }
                div()
                    .flex()
                    .flex_col()
                    .gap(px(GAP))
                    .debug_selector(|| "calendar".into())
                    .child(months)
                    .child(
                        div()
                            .flex()
                            .gap(px(GAP))
                            .child(weekdays)
                            .children(weeks),
                    )
                    .into_any_element()
            }
        };
        let legend_item = |bg: Option<u32>, text: &'static str| {
            let swatch = div().size(px(12.)).rounded(px(3.));
            let swatch = match bg {
                Some(bg) => swatch.bg(color(bg)),
                None => swatch
                    .border_1()
                    .border_dashed()
                    .border_color(color(theme::BORDER_BUTTON)),
            };
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(swatch)
                .child(text)
        };
        div()
            .flex_1()
            .min_w(px(700.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(18.))
            .py(px(16.))
            .rounded(px(14.))
            .border_1()
            .border_color(color(theme::BORDER))
            .bg(color(theme::SECTION))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_between()
                    .items_center()
                    .gap(px(10.))
                    .child(widgets::heading(format!("{days}-day trips by departure date"), 15.))
                    .child(div().flex().gap(px(8.)).child(length_tabs).child(cabin_tabs)),
            )
            .child(div().id("calendar-scroll").overflow_x_scroll().child(body))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(16.))
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_LABEL))
                    .child("Compared with the usual price for the same days to departure:")
                    .child(legend_item(Some(theme::YELLOW), "deal"))
                    .child(legend_item(Some(theme::TONE_BELOW), "below"))
                    .child(legend_item(Some(theme::TONE_TYPICAL), "typical"))
                    .child(legend_item(Some(theme::TONE_ABOVE), "above"))
                    .child(legend_item(None, "past"))
                    .when(detail.is_some_and(|d| d.awards.is_some()), |l| {
                        l.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .children(award_dot(crate::grid::AwardMark::BothLegs, 8.))
                                .child("award, both legs"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .children(award_dot(crate::grid::AwardMark::OneLeg, 8.))
                                .child("one leg"),
                        )
                    }),
            )
    }

    fn day_cell(
        &self,
        wi: usize,
        di: usize,
        day: &CalendarDay,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let date = day.date;
        let cell = div()
            .id(SharedString::from(format!("day-{wi}-{di}")))
            .size(px(CELL))
            .rounded(px(5.))
            .relative()
            // A lavender dot in the corner of trips with an award (0083).
            .children(
                award_dot(day.award, 6.)
                    .map(|d| d.absolute().top(px(2.)).right(px(2.))),
            );
        let cell = match tone_bg(day.tone) {
            Some(bg) => cell.bg(color(bg)).cursor_pointer().on_click(
                cx.listener(move |this, _, _, cx| {
                    this.departure = Some(date);
                    this.refresh(cx);
                    cx.notify();
                }),
            ),
            None => cell
                .border_1()
                .border_dashed()
                .border_color(color(theme::BORDER_BUTTON)),
        };
        if day.selected {
            cell.border_2().border_color(color(0xFFFFFF))
        } else {
            cell.hover(|d| d.border_1().border_color(color(theme::TEXT_MUTED)))
        }
    }

    fn render_departure(
        &self,
        fonts: &Fonts,
        s: &Snapshot,
        row: &RouteRow,
        detail: Option<&RouteDetail>,
        days: u16,
        cx: &mut Context<Self>,
    ) -> Div {
        let panel = div()
            .flex_1()
            .min_w(px(300.))
            .max_w(px(380.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(18.))
            .py(px(16.))
            .rounded(px(14.))
            .bg(color(0x0D1220));
        let Some(d) = detail else {
            return panel.border_1().border_color(color(theme::BORDER));
        };
        let trip = d.trip.as_ref();
        let departure = self.departure.or(trip.map(|t| t.departure));
        let Some(departure) = departure else {
            return panel.border_1().border_color(color(theme::BORDER)).child(
                div()
                    .text_color(color(theme::TEXT_FAINT))
                    .child("No prices yet."),
            );
        };
        let ret = departure + Duration::days(i64::from(days));
        let day = d.calendar.day(departure);
        let is_trip =
            trip.is_some_and(|t| t.departure == departure && t.days() == days);
        let deal = day.is_some_and(|d| d.tone == Tone::Deal);
        let change = day.and_then(|d| {
            let p = d.price?;
            let b = d.baseline?;
            (b > 0).then(|| p.minor as f64 / b as f64 - 1.0)
        });
        let chip = change.map(|c| {
            let text = if deal {
                format!("Deal · {}", format::change(c))
            } else {
                format::change(c)
            };
            let chip = div()
                .px(px(10.))
                .py(px(3.))
                .rounded(px(12.))
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .child(text);
            if deal {
                chip.bg(color(theme::YELLOW))
                    .text_color(color(theme::ON_YELLOW))
            } else {
                chip.border_1()
                    .border_color(color(theme::BORDER_BUTTON))
                    .text_color(color(theme::TEXT_MUTED))
            }
        });
        let price_line = day.and_then(|d| d.price).map(|p| {
            div()
                .flex()
                .items_baseline()
                .gap(px(10.))
                .child(
                    mono(
                        fonts,
                        format::money(&Money::new(p.minor, Currency::brl())),
                    )
                    .text_size(px(24.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(if deal {
                        theme::PRICE_DEAL
                    } else {
                        theme::TEXT
                    })),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(theme::TEXT_FAINT))
                        .child(if p.confirmed {
                            "round trip, confirmed"
                        } else {
                            "estimated from the two legs"
                        }),
                )
        });
        let carrier = trip.filter(|_| is_trip).and_then(|t| {
            let label = t.carrier_label()?;
            Some(match t.duration_minutes {
                Some(m) => format!(
                    "{label} {} → {} · {}",
                    row.route.origin.code,
                    row.code(),
                    format::duration(m)
                ),
                None => label,
            })
        });
        let settings = &s.settings;
        panel
            .border_1()
            .border_color(color(if deal { theme::YELLOW } else { theme::BORDER }))
            .debug_selector(|| "departure".into())
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .gap(px(8.))
                    .child(widgets::heading(
                        format!("{} → {}", format::day_date(departure), format::day_date(ret)),
                        15.,
                    ))
                    .children(chip),
            )
            .children(carrier.map(|c| div().text_color(color(theme::TEXT_MUTED)).child(c)))
            .children(price_line)
            .when(is_trip, |p| p.child(sellers_table(fonts, d, true)))
            .when(!is_trip, |p| {
                p.child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(theme::TEXT_FAINT))
                        .child("Sellers are read for confirmed deals; Check now asks for a search of these dates."),
                )
            })
            .children(d.awards.as_ref().map(|a| {
                self.render_award_brands(fonts, row, a, departure, ret, cx)
            }))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child(format!(
                        "Effective price includes IOF {} and a {} card spread.{}",
                        format::percent(settings.iof),
                        format::percent(settings.card_spread),
                        d.awards
                            .as_ref()
                            .and_then(|a| Some((a, a.value_per_1000.as_ref()?)))
                            .map(|(a, v)| format!(
                                " ≈R$ uses your {} value, {} per 1,000.",
                                a.name,
                                format::mile_value(v.as_f64()).trim_end_matches("/k")
                            ))
                            .unwrap_or_default()
                    )),
            )
            .child(
                widgets::primary_button("detail-open-gf", "Open in Google Flights")
                    .flex()
                    .justify_center()
                    .on_click(cx.listener(|this, _, _, cx| this.open_in_google_flights(cx))),
            )
    }
}

impl TravelMonitor {
    /// The award tiles (0083): one row per active program with data,
    /// twins of the cash tiles and never merged with them: cheapest
    /// award, lowest ever in miles, the award series' usual, and when
    /// it was checked.
    fn render_award_tiles(
        &self,
        fonts: &Fonts,
        s: &Snapshot,
        detail: Option<&RouteDetail>,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let label = |text: String| {
            div()
                .text_size(px(12.))
                .text_color(color(theme::TEXT_LABEL))
                .child(text)
        };
        let Some(a) = detail.and_then(|d| d.awards.as_ref()) else {
            let program =
                s.miles.iter().find(|m| m.active() && m.has_reader)?;
            return Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(13.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child(widgets::miles_tag(fonts))
                    .child(format!("{}: no award checked yet", program.name))
                    .when(self.can_check_award(), |d| {
                        d.child(
                            widgets::small_button(
                                "tiles-check-award",
                                "Check award",
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| this.check_award(cx),
                            )),
                        )
                    }),
            );
        };
        let unit = a.unit;
        let cheapest = widgets::stat_tile(
            fonts,
            "Cheapest award",
            a.cheapest.as_ref().map_or_else(
                || div().child("–"),
                |q| widgets::miles_text(fonts, q.price.miles, unit),
            ),
            a.cheapest
                .as_ref()
                .map(|q| {
                    let when = match (q.leg, q.return_date) {
                        (_, Some(r)) => format::date_range(q.departure, r),
                        (Some(tm_domain::Leg::Return), _) => {
                            format!(
                                "return {}, one way",
                                format::short_date(q.departure)
                            )
                        }
                        _ => format!(
                            "out {}, one way",
                            format::short_date(q.departure)
                        ),
                    };
                    format!("+ {} · {when}", format::money(&q.price.taxes))
                })
                .unwrap_or_default(),
            theme::MILES,
        )
        .flex_1();
        let lowest = widgets::stat_tile(
            fonts,
            "Lowest ever",
            a.lowest_ever.map_or_else(
                || div().child("–"),
                |(m, _)| widgets::miles_text(fonts, m, unit),
            ),
            a.lowest_ever
                .map(|(_, day)| {
                    if a.checks <= 1 {
                        format!("{} · first check", format::short_date(day))
                    } else {
                        format::short_date(day)
                    }
                })
                .unwrap_or_default(),
            theme::TEXT,
        )
        .flex_1();
        let checks = match a.checks {
            1 => "1 check".to_owned(),
            n => format!("{n} checks"),
        };
        let usual = widgets::stat_tile(
            fonts,
            a.usual.as_ref().map_or_else(
                || "Usual".to_owned(),
                |(_, b)| format!("Usual, {b}"),
            ),
            a.usual.as_ref().map_or_else(
                || div().child("–"),
                |(m, _)| widgets::miles_text(fonts, *m, unit),
            ),
            match &a.usual {
                Some(_) => "60-day median, miles".to_owned(),
                None => format!("no history · {checks}"),
            },
            theme::TEXT,
        )
        .flex_1();
        let login = match a.login {
            Login::LoggedIn => "logged in",
            Login::LoggedOut => "not logged in",
            Login::Expired => "log in again",
        };
        let checked = widgets::stat_tile(
            fonts,
            "Award checked",
            div().child(
                a.checked_at
                    .map_or_else(|| "never".to_owned(), format::clock),
            ),
            format!("{} of {} dates · {login}", a.dates_known, a.dates_total),
            theme::TEXT,
        )
        .flex_1();
        let rate = a.usual_rate.map(|u| {
            format!(
                "{} pays {} against its own fares (median of {} checks)",
                a.name,
                format::mile_value_long(u.median, unit),
                u.n
            )
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .debug_selector(|| "award-tiles".into())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(widgets::miles_tag(fonts))
                        .child(label(format!("Miles · {}", a.name)))
                        .children(rate.map(label)),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(10.))
                        .child(cheapest)
                        .child(lowest)
                        .child(usual)
                        .child(checked),
                ),
        )
    }

    /// The selected dates' award brands (0083): each leg's and the
    /// round trip's, fewest miles first, with taxes and seats; other
    /// cabins of the same load under their own heading; a leg not
    /// checked says so.
    fn render_award_brands(
        &self,
        fonts: &Fonts,
        row: &RouteRow,
        a: &RouteAwards,
        departure: chrono::NaiveDate,
        ret: chrono::NaiveDate,
        cx: &mut Context<Self>,
    ) -> Div {
        let (o, d) = (&row.route.origin.code, &row.code().to_owned());
        let head = |text: String| {
            div()
                .pt(px(6.))
                .text_size(px(13.))
                .font_weight(FontWeight::BOLD)
                .child(text)
        };
        let note = |text: String| {
            div()
                .text_size(px(12.))
                .text_color(color(theme::TEXT_FAINT))
                .child(text)
        };
        let table = |quotes: &[AwardQuote], id: &str| -> Div {
            let rows = quotes.iter().take(5).enumerate().map(|(i, q)| {
                div()
                    .id(SharedString::from(format!("{id}-{i}")))
                    .flex()
                    .gap(px(8.))
                    .py(px(5.))
                    .border_t_1()
                    .border_color(color(theme::BORDER))
                    .text_size(px(12.))
                    .child(
                        div()
                            .w(px(92.))
                            .flex_none()
                            .truncate()
                            .text_color(color(theme::TEXT_SOFT))
                            .child(
                                q.flight_label()
                                    .unwrap_or_else(|| "–".to_owned()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(color(theme::TEXT_MUTED))
                            .child(q.brand.clone().unwrap_or_default()),
                    )
                    .children(
                        a.unusual(q)
                            .then(|| {
                                q.rate().map(|r| widgets::rate_chip(fonts, r))
                            })
                            .flatten(),
                    )
                    .child(
                        mono(fonts, format::miles(q.price.miles))
                            .w(px(66.))
                            .flex_none()
                            .text_right(),
                    )
                    .child(
                        mono(fonts, format::money(&q.price.taxes))
                            .w(px(48.))
                            .flex_none()
                            .text_right()
                            .text_color(color(theme::TEXT_FAINT)),
                    )
                    .child(
                        mono(
                            fonts,
                            q.seats
                                .map_or_else(|| "–".to_owned(), |s| s.short()),
                        )
                        .w(px(46.))
                        .flex_none()
                        .text_right()
                        .text_color(color(
                            if q.seats.is_some_and(|s| s.few()) {
                                theme::YELLOW
                            } else {
                                theme::TEXT_FAINT
                            },
                        )),
                    )
            });
            let more = quotes.len().saturating_sub(5);
            div()
                .flex()
                .flex_col()
                .children(rows)
                .children((more > 0).then(|| note(format!("{more} more"))))
        };
        let mut panel = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pt(px(8.))
            .border_t_1()
            .border_color(color(theme::MILES_BORDER))
            .debug_selector(|| "award-brands".into());
        let first = a.outbound.first();
        panel = panel.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(widgets::miles_tag(fonts))
                .child(head(format!(
                    "Award · out {} · {o} → {d}",
                    format::short_date(departure)
                ))),
        );
        if a.outbound.is_empty() {
            panel = panel
                .child(note(format!("{}: not checked for this date.", a.name)));
        } else {
            let mut seen = vec![a.name.clone()];
            seen.extend(
                first.map(|q| format!("seen {}", format::clock(q.observed_at))),
            );
            let itineraries = a
                .outbound
                .iter()
                .filter_map(|q| q.flights.as_deref())
                .collect::<std::collections::HashSet<_>>()
                .len();
            seen.push(format!(
                "{itineraries} itinerar{}",
                if itineraries == 1 { "y" } else { "ies" }
            ));
            seen.extend(
                first
                    .and_then(|q| q.rate())
                    .map(|r| format::mile_value_long(r, a.unit)),
            );
            panel = panel
                .child(note(seen.join(" · ")))
                .child(table(&a.outbound, "brand-out"));
            // What the cheapest outbound costs with the user's value and
            // with bought miles, against the cash fare of the same leg.
            if let Some(q) = first {
                let mut lines = Vec::new();
                if let (Some(cash), Some(r)) = (
                    &a.cash_outbound,
                    a.cash_outbound.as_ref().and_then(|c| {
                        tm_scheduler::awards::rate_against_cash(&q.price, c)
                    }),
                ) {
                    lines.push(format!(
                        "Cash for this leg {} · its miles are worth {}",
                        format::money(cash),
                        format::mile_value(r)
                    ));
                }
                if let Some(v) = &a.value_per_1000 {
                    lines.extend(format::valued(&q.price, v));
                }
                if let Some(buy) = &a.buy_per_1000
                    && let Some(b) = tm_scheduler::awards::buy_miles(
                        &q.price,
                        buy,
                        a.cash_outbound.as_ref(),
                    )
                {
                    let saving = match &b.saving {
                        Some(s) if s.minor >= 0 => {
                            format!(" · {} under cash", format::money(s))
                        }
                        Some(s) => format!(
                            " · {} over cash",
                            format::money(&tm_domain::Money::new(
                                -s.minor,
                                s.currency.clone()
                            ))
                        ),
                        None => String::new(),
                    };
                    lines.push(format!(
                        "With miles bought at {}: {}{saving}",
                        format::mile_value(buy.as_f64()),
                        format::money(&b.cost)
                    ));
                }
                panel = panel.children(lines.into_iter().map(|l| {
                    div()
                        .text_size(px(12.))
                        .text_color(color(theme::TEXT_SOFT))
                        .debug_selector(|| "award-value".into())
                        .child(l)
                }));
            }
        }
        if !a.same_load.is_empty() {
            let cabin = a.same_load[0].cabin.label();
            panel = panel
                .child(note(format!("{cabin} on the same load")))
                .child(table(&a.same_load, "brand-same"));
        }
        panel = panel.child(head(format!(
            "Return {} · {d} → {o}",
            format::short_date(ret)
        )));
        if a.inbound.is_empty() {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(note("not checked".to_owned()))
                    .when(self.can_check_award(), |d| {
                        d.child(
                            widgets::small_button(
                                "brands-check-award",
                                "Check award",
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| this.check_award(cx),
                            )),
                        )
                    }),
            );
        } else {
            panel = panel.child(table(&a.inbound, "brand-ret"));
        }
        if !a.round_trip.is_empty() {
            panel = panel
                .child(head(format!(
                    "Round trip {}",
                    format::date_range(departure, ret)
                )))
                .child(table(&a.round_trip, "brand-rt"));
        }
        panel
    }
}

fn render_checks(
    fonts: &Fonts,
    checks: &[ActivityRow],
    glow: impl Fn(i64) -> f32,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(widgets::heading("Recent checks", 15.))
        .child(super::activity::activity_table(
            fonts, checks, false, None, glow,
        ))
}
