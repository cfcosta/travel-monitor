//! The main screen, "G · Map + list" (DECISIONS.md 0044): deals and
//! routes on the left, the night map on the right, and a card for the
//! selected row over the map.

use std::rc::Rc;

use chrono::{Datelike, NaiveDate};
use gpui::{
    AnyElement,
    Context,
    Div,
    FontWeight,
    SharedString,
    Stateful,
    Window,
    div,
    list,
    prelude::*,
    px,
    relative,
    rgba,
};
use tm_domain::{Cabin, Currency, Money, PlaceId};

use super::{
    ListTab,
    Menu,
    PayWith,
    Screen,
    Sort,
    TravelMonitor,
    map::{MapDest, MapModel, MapPlace, map},
    widgets::{self, mono},
};
use crate::{
    data::{AwardSummary, Login, RouteDetail, RouteRow, RowKey, Trip},
    format,
    geo::Region,
    grid::{AwardMark, CalendarDay, Tone as CalendarTone},
    live::Fresh,
    theme::{self, Fonts, color},
};

const CABINS: [Option<Cabin>; 4] = [
    None,
    Some(Cabin::Economy),
    Some(Cabin::PremiumEconomy),
    Some(Cabin::Business),
];

fn cabin_label(cabin: Option<Cabin>) -> &'static str {
    cabin.map_or("All cabins", Cabin::label)
}

impl TravelMonitor {
    pub(super) fn render_deals(
        &mut self,
        fonts: &Fonts,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .size_full()
            .min_h_0()
            .child(self.render_list(fonts, cx))
            .child(self.render_map_pane(fonts, cx))
            .into_any_element()
    }

    /// The star of `place` (0094); a click stars or unstars it without
    /// selecting the row under it.
    pub(super) fn star(
        &self,
        id: String,
        place: PlaceId,
        size: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let on = self.snapshot.as_ref().is_some_and(|s| s.is_favorite(place));
        widgets::star(id, on, size).on_click(cx.listener(
            move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_favorite(place, cx);
            },
        ))
    }

    fn render_list(&self, fonts: &Fonts, cx: &mut Context<Self>) -> Div {
        let Some(s) = &self.snapshot else {
            return div();
        };
        let pay = self.pay;
        let favorites_only = self.favorites_only;
        let listed =
            |r: &&RouteRow| !favorites_only || s.is_favorite(r.place());
        let deal_count = s
            .rows
            .iter()
            .filter(listed)
            .filter(|r| r.is_deal(pay))
            .count();
        let all_count = s
            .rows
            .iter()
            .filter(listed)
            .filter(|r| pay != PayWith::Miles || r.award.is_some())
            .count();
        let tab = |id: &'static str,
                   label: &'static str,
                   count: usize,
                   which: ListTab| {
            widgets::segment(
                id,
                div()
                    .flex()
                    .gap(px(6.))
                    .child(label)
                    .child(mono(fonts, count.to_string()).text_size(px(12.))),
                self.list_tab == which,
            )
            .flex_1()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.list_tab = which;
                cx.notify();
            }))
        };
        let tabs = widgets::segments()
            .p(px(4.))
            .rounded(px(10.))
            .child(tab("tab-deals", "Deals", deal_count, ListTab::Deals))
            .child(tab("tab-all", "All routes", all_count, ListTab::All));
        // Pay with (0083): drives the list, the map and the card.
        let pay_with = widgets::segments().children(PayWith::ALL.map(|p| {
            widgets::segment(
                SharedString::from(format!("pay-{}", p.label().to_lowercase())),
                p.label(),
                pay == p,
            )
            .flex_1()
            .py(px(4.))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.pay = p;
                cx.notify();
            }))
        }));
        // Every place, or only the starred ones (0094).
        let places = widgets::segments().children(
            [
                (false, "All", "places-all"),
                (true, "★ Favorites", "places-favorites"),
            ]
            .map(|(only, label, id)| {
                widgets::segment(id, label, favorites_only == only)
                    .debug_selector(move || id.to_owned())
                    .flex_1()
                    .py(px(4.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.favorites_only = only;
                        cx.notify();
                    }))
            }),
        );
        let cabin_menu = self.dropdown(
            Menu::Cabin,
            "cabin",
            cabin_label(self.cabin).into(),
            CABINS
                .iter()
                .map(|c| (cabin_label(*c).into(), *c == self.cabin))
                .collect(),
            |this, i, cx| {
                this.cabin = CABINS[i];
                cx.notify();
            },
            cx,
        );
        let sort_menu = self.dropdown(
            Menu::Sort,
            "sort",
            self.sort.label(pay).into(),
            Sort::ALL
                .iter()
                .map(|s| (s.label(pay).into(), *s == self.sort))
                .collect(),
            |this, i, cx| {
                this.sort = Sort::ALL[i];
                cx.notify();
            },
            cx,
        );
        let field = |label: &'static str, control: Div| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(4.))
                .text_size(px(11.))
                .text_color(color(theme::TEXT_LABEL))
                .child(label)
                .child(control)
        };
        let rows = self.visible_rows();
        let logged_in = s
            .miles
            .iter()
            .any(|m| m.active() && m.has_reader && m.login == Login::LoggedIn);
        let empty = rows.is_empty().then(|| {
            div()
                .p(px(24.))
                .text_color(color(theme::TEXT_FAINT))
                .child(match (pay, self.list_tab) {
                    _ if favorites_only && s.favorites.is_empty() => {
                        "No favorites yet. Star a place with ☆ on its row to follow it closely.".to_owned()
                    }
                    (_, ListTab::Deals) if favorites_only => {
                        "No deals on your favorites right now. They are checked about twice as often.".to_owned()
                    }
                    (PayWith::Miles, _) if !logged_in => {
                        "No miles program is logged in. Log in under Settings.".to_owned()
                    }
                    (PayWith::Miles, ListTab::All) => {
                        let budget = s
                            .daemon
                            .sites
                            .iter()
                            .find(|r| r.site.starts_with("miles:"))
                            .and_then(|r| r.budget)
                            .unwrap_or(tm_domain::SiteLimits::MILES.start);
                        format!("No award prices yet: the program checks about {budget} dates a day.")
                    }
                    (_, ListTab::Deals) => "No deals right now. Every watched route is listed under All routes.".to_owned(),
                    (_, ListTab::All) => "No routes match. Add destinations in Settings.".to_owned(),
                })
        });
        let keys: Rc<Vec<RowKey>> =
            Rc::new(rows.iter().map(|r| r.key()).collect());
        let content = super::fingerprint((
            s.taken_at,
            pay,
            self.list_tab,
            self.cabin,
            self.sort,
            self.favorites_only,
            &*keys,
        ));
        let state = self.deals_rows.sync(keys.len(), content);
        let items = list(
            state,
            cx.processor(move |this, ix: usize, _, cx| {
                let fonts = theme::fonts(cx);
                let row =
                    keys.get(ix).and_then(|k| this.snapshot.as_ref()?.row(*k));
                match row {
                    Some(row) => {
                        this.render_row(&fonts, row, cx).into_any_element()
                    }
                    None => div().into_any_element(),
                }
            }),
        )
        .flex_1()
        .min_h_0();
        div()
            .flex()
            .flex_col()
            .w(px(420.))
            .flex_none()
            .min_h_0()
            .border_r_1()
            .border_color(color(theme::BORDER))
            .bg(color(theme::PANEL))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .px(px(18.))
                    .pt(px(16.))
                    .pb(px(12.))
                    .border_b_1()
                    .border_color(color(theme::BORDER))
                    .child(tabs)
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(field("Pay with", pay_with))
                            .child(field("Places", places)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(field("Cabin", cabin_menu))
                            .child(field("Sort", sort_menu)),
                    ),
            )
            .child(
                div()
                    .id("rows")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .children(empty)
                    .child(items),
            )
    }

    fn render_row(
        &self,
        fonts: &Fonts,
        row: &RouteRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = row.key();
        let selected = self.selected == Some(key);
        let pay = self.pay;
        let award = row.award_for(pay);
        let leads = row.award_leads(pay);
        let deal = if leads {
            award.is_some_and(|a| a.status.is_deal())
        } else {
            row.status.is_deal()
        };
        let dates = row
            .best
            .as_ref()
            .map(|t| format::date_range(t.departure, t.return_date))
            .unwrap_or_else(|| "no prices yet".to_owned());
        let sub = match (award.filter(|_| leads), row.price_per_km()) {
            (Some(a), _) => {
                let mut parts = vec![
                    row.cabin.label().to_owned(),
                    a.name.clone(),
                    award_dates(a),
                ];
                parts.extend(a.best.route_label());
                let km = row.distance_km();
                if self.sort == Sort::LowestPricePerKm && km >= 1.0 {
                    parts.push(format::miles_per_km(
                        a.best.price.miles as f64 / km,
                        a.best.unit,
                    ));
                }
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(widgets::miles_tag(fonts))
                    .child(div().truncate().child(parts.join(" · ")))
            }
            (None, Some(per_km)) if self.sort == Sort::LowestPricePerKm => {
                div().child(format!(
                    "{} · {dates} · {}",
                    row.cabin.label(),
                    format::per_km(per_km)
                ))
            }
            _ => div().child(format!("{} · {dates}", row.cabin.label())),
        };
        // The third line: under a miles-led row the cash price for scale;
        // under a cash row the award, when the program priced one.
        let third = award.map(|a| {
            let line = div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(color(theme::TEXT_SOFT));
            if leads {
                let mut parts = Vec::new();
                // The cash fare of the award's own leg, else the row's trip.
                let cash =
                    a.cash.as_ref().or(row.best.as_ref().map(|t| &t.price));
                parts
                    .extend(cash.map(|c| format!("cash {}", format::money(c))));
                parts.extend(format::seats(a.best.seats));
                line.child(parts.join(" · "))
            } else {
                line.child(widgets::miles_tag(fonts))
                    .child(award_line(fonts, a))
                    .children(
                        a.unusual
                            .then(|| {
                                a.rate.map(|r| widgets::rate_chip(fonts, r))
                            })
                            .flatten(),
                    )
                    .children(
                        a.status.is_deal().then(|| {
                            widgets::change_chip(fonts, a.change, true)
                        }),
                    )
            }
        });
        let price: AnyElement = match award.filter(|_| leads) {
            Some(a) => div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(2.))
                .child(
                    widgets::miles_text(fonts, a.best.price.miles, a.best.unit)
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(15.)),
                )
                .child(
                    mono(
                        fonts,
                        format!("+{}", format::money(&a.best.price.taxes)),
                    )
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_FAINT)),
                )
                .into_any_element(),
            None => mono(
                fonts,
                row.best
                    .as_ref()
                    .map(|t| format::money(&t.price))
                    .unwrap_or_else(|| "–".to_owned()),
            )
            .font_weight(FontWeight::BOLD)
            .text_size(px(15.))
            .into_any_element(),
        };
        let change = row.lead_change(pay);
        let chip = if leads
            && award.is_some_and(|a| a.history_days < 28 && !a.status.is_deal())
        {
            no_history_chip(fonts)
        } else {
            widgets::change_chip(fonts, change, deal)
        };
        let history = match award.filter(|_| leads) {
            Some(a) => a.history.clone(),
            None => row.history.clone(),
        };
        let item = div()
            .w_full()
            .id(SharedString::from(format!(
                "row-{}-{}",
                key.route,
                key.cabin.code()
            )))
            .debug_selector(move || {
                format!("row-{}-{}", key.route, key.cabin.code())
            })
            .flex()
            .when(third.is_some(), |d| d.items_start())
            .when(third.is_none(), |d| d.items_center())
            .gap(px(12.))
            .px(px(18.))
            .py(px(12.))
            .border_b_1()
            .border_color(color(theme::ROW_RULE))
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(self.star(
                                format!(
                                    "star-{}-{}",
                                    key.route,
                                    key.cabin.code()
                                ),
                                row.place(),
                                15.,
                                cx,
                            ))
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::BOLD)
                                    .child(row.code().to_owned()),
                            )
                            .child(
                                div()
                                    .text_color(color(theme::TEXT_MUTED))
                                    .truncate()
                                    .child(row.city().to_owned()),
                            ),
                    )
                    .child(
                        sub.text_size(px(12.))
                            .text_color(color(theme::TEXT_FAINT)),
                    )
                    .children(third.map(|t| {
                        t.debug_selector(move || {
                            format!(
                                "award-line-{}-{}",
                                key.route,
                                key.cabin.code()
                            )
                        })
                    })),
            )
            .child(widgets::sparkline(
                history,
                80.,
                26.,
                if deal { theme::YELLOW } else { theme::SPARK },
            ))
            .child(
                div()
                    .w(px(118.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(4.))
                    .child(price)
                    .child(chip),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select(key, cx)))
            .relative()
            // A deal that just showed up (0057).
            .children(widgets::fresh_tint(
                row.deal.map_or(0.0, |d| self.glow(Fresh::Deal(d))),
            ));
        if selected {
            item.relative()
                .bg(color(theme::SELECTED))
                .child(widgets::accent())
        } else {
            item.hover(|s| s.bg(color(0x0B1324)))
        }
    }

    /// The globe's dots: one per destination of the rows the list shows,
    /// so the sidebar's filters (tab, Pay with, cabin, Places) apply to
    /// the map too. A deal in any listed cabin makes the dot yellow, in
    /// the series Pay with shows (0083). Each dot's key is the row a
    /// click selects.
    pub(crate) fn map_destinations(
        &self,
        home: Option<&str>,
    ) -> (Vec<MapDest>, Vec<RowKey>) {
        let mut dests: Vec<MapDest> = Vec::new();
        let mut keys = Vec::new();
        for row in self.visible_rows() {
            let d = &row.route.destination;
            let selected = self.selected == Some(row.key());
            if let Some(i) =
                dests.iter().position(|m| m.place.code.as_ref() == d.code)
            {
                dests[i].deal |= row.is_deal(self.pay);
                if selected {
                    dests[i].selected = true;
                    keys[i] = row.key();
                }
                continue;
            }
            dests.push(MapDest {
                place: MapPlace {
                    code: d.code.clone().into(),
                    lon: d.lon as f32,
                    lat: d.lat as f32,
                },
                region: Region::of(&d.country.0, home),
                deal: row.is_deal(self.pay),
                selected,
            });
            keys.push(row.key());
        }
        (dests, keys)
    }

    fn render_map_pane(
        &self,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Some(s) = &self.snapshot else {
            return div().id("deals-map-pane");
        };
        let home = s.origin_place().map(|p| p.country.0.as_str());
        let origin = s.origin_place().map(|p| MapPlace {
            code: p.code.clone().into(),
            lon: p.lon as f32,
            lat: p.lat as f32,
        });
        let (dests, keys) = self.map_destinations(home);
        let model = MapModel {
            origin,
            destinations: dests,
            labels: true,
        };
        let entity = cx.entity().downgrade();
        let picker = move |i: usize, _: &mut Window, cx: &mut gpui::App| {
            if let (Some(key), Some(view)) =
                (keys.get(i).copied(), entity.upgrade())
            {
                view.update(cx, |this, cx| this.select(key, cx));
            }
        };
        // A column: the map takes what the card leaves, at least a usable
        // minimum, and the pane scrolls when a small window cannot hold
        // both. The card used to float over the map and covered all of
        // it (and the top bar) in small windows.
        div()
            .id("deals-map-pane")
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .child(div().relative().flex_1().min_h(px(280.)).child(
                div().absolute().inset_0().child(map(
                    model,
                    fonts.clone(),
                    Some(self.deals_map.clone()),
                    picker,
                )),
            ))
            .children(self.render_card(fonts, cx))
    }

    /// The selected route's card (0076): the trip and its price on one
    /// line, then every departure's price for the chosen trip length,
    /// against the usual price, with the cheapest of each month.
    fn render_card(
        &self,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let s = self.snapshot.as_ref()?;
        let key = self.selected?;
        let row = s.row(key)?;
        let detail = s.detail.as_ref().filter(|d| d.key == key);
        let pay = self.pay;
        let award = row.award_for(pay);
        let leads = row.award_leads(pay);
        let deal = if leads {
            award.is_some_and(|a| a.status.is_deal())
        } else {
            row.status.is_deal()
        };
        let trip = detail.and_then(|d| d.trip.as_ref()).or(row.best.as_ref());
        let origin = &row.route.origin.code;
        let usual = detail
            .and_then(|d| d.usual.as_ref().map(|(m, _)| m.clone()))
            .or_else(|| row.baseline.clone());
        let days = self
            .calendar_days
            .unwrap_or(s.settings.trips.round_trip_days);
        let priced: Vec<&CalendarDay> = detail
            .map(|d| {
                d.calendar
                    .weeks
                    .iter()
                    .flat_map(|w| &w.days)
                    .filter(|d| d.tone != CalendarTone::Past)
                    .collect()
            })
            .unwrap_or_default();
        let cheapest =
            priced.iter().filter_map(|d| d.price.map(|p| p.minor)).min();
        let dated = priced.iter().filter(|d| d.price.is_some()).count();

        let route = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(FontWeight::BOLD)
                            .child(format!("{origin} → {}", row.code())),
                    )
                    .child(self.star(
                        "card-star".to_owned(),
                        row.place(),
                        20.,
                        cx,
                    ))
                    .child(widgets::series_chip(
                        row.status,
                        award.map(|a| a.status),
                    )),
            )
            .child(
                div().text_color(color(theme::TEXT_MUTED)).child(
                    [
                        Some(row.city().to_owned()),
                        Some(row.cabin.label().to_owned()),
                        Some(
                            match (
                                award.filter(|_| leads),
                                trip.and_then(|t| t.carrier_label()),
                            ) {
                                (Some(a), _) => a.name.clone(),
                                (None, Some(c)) => c,
                                (None, None) => "round trip".to_owned(),
                            },
                        ),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · "),
                ),
            );
        let vs = match (row.change, &usual) {
            (Some(c), Some(u)) => Some(format!(
                "{} vs usual {}",
                format::change(c),
                format::money(u)
            )),
            (Some(c), None) => Some(format!("{} vs usual", format::change(c))),
            _ => None,
        };
        let when = match award.filter(|_| leads) {
            Some(a) => Some(award_leg_text(a)),
            None => trip.map(|t| {
                let range = format::date_range(t.departure, t.return_date);
                match cheapest {
                    Some(min) if t.price.minor <= min && dated > 1 => {
                        format!("{range} · cheapest of {dated} dates")
                    }
                    _ => range,
                }
            }),
        };
        let route = route.children(
            when.map(|w| div().text_color(color(theme::TEXT_SOFT)).child(w)),
        );
        let price = match award.filter(|_| leads) {
            Some(a) => award_price_block(fonts, a, trip),
            None => div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(4.))
                .ml_auto()
                .flex_none()
                .child(
                    mono(
                        fonts,
                        trip.map_or_else(
                            || "–".to_owned(),
                            |t| format::money(&t.price),
                        ),
                    )
                    .text_size(px(30.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(if deal {
                        theme::PRICE_DEAL
                    } else {
                        theme::TEXT
                    })),
                )
                .children(vs.map(|v| {
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(if deal {
                            theme::YELLOW
                        } else {
                            theme::DROP
                        }))
                        .child(v)
                })),
        };
        let can_award = self.can_check_award();
        let actions = div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .ml_auto()
            .flex_none()
            .child(
                widgets::primary_button("open-gf", "Open in Google Flights")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_in_google_flights(cx)
                    })),
            )
            .child(
                widgets::ghost_button("check-now", "Check now")
                    .on_click(cx.listener(|this, _, _, cx| this.check_now(cx))),
            )
            .child(widgets::ghost_button("price-history", "History").on_click(
                cx.listener(|this, _, window, cx| {
                    this.navigate(Screen::Detail, window, cx)
                }),
            ))
            .when(can_award && pay != PayWith::Cash, |a| {
                a.child(
                    widgets::ghost_button("check-award", "Check award")
                        .on_click(
                            cx.listener(|this, _, _, cx| this.check_award(cx)),
                        ),
                )
            });
        let header = div()
            .flex()
            .items_start()
            .gap(px(24.))
            .child(route)
            .child(price);

        let t = &s.settings.trips;
        let lengths =
            widgets::segments().children((t.min_days..=t.max_days).map(|n| {
                widgets::segment(
                    SharedString::from(format!("card-len-{n}")),
                    format!("{n}d"),
                    n == days,
                )
                .px(px(8.))
                .on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.calendar_days = Some(n);
                        this.departure = None;
                        this.refresh(cx);
                        cx.notify();
                    },
                ))
            }));
        let strip_head = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(widgets::label(format!(
                "Price by departure date · {days}-day trips"
            )))
            .child(div().ml_auto().child(lengths));
        let strip: AnyElement = match detail {
            None => div()
                .h(px(STRIP_H))
                .text_color(color(theme::TEXT_FAINT))
                .child("Loading…")
                .into_any_element(),
            Some(_) if dated == 0 => div()
                .h(px(STRIP_H))
                .text_color(color(theme::TEXT_FAINT))
                .child(format!("No {days}-day trips priced yet."))
                .into_any_element(),
            Some(_) => price_strip(
                fonts,
                &priced,
                trip.map(|t| t.departure),
                usual.as_ref().map(|u| u.minor),
                award.is_some(),
                cx,
            )
            .into_any_element(),
        };

        let legend_item = |hex: u32, text: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().size(px(10.)).rounded(px(2.)).bg(color(hex)))
                .child(text)
        };
        let history_days = row.history.len();
        let mut notes: Vec<String> = Vec::new();
        if let Some(at) = row.last_checked {
            notes.push(format!("Seen {}", format::clock(at)));
        }
        notes.push(match history_days {
            0 => "no price history yet".to_owned(),
            1 => "1 day of prices, so “usual” is still rough".to_owned(),
            n if n < 7 => {
                format!("{n} days of prices, so “usual” is still rough")
            }
            n => format!("{n} days of prices"),
        });
        match detail
            .and_then(|d| d.sellers.iter().min_by_key(|s| s.effective.minor))
        {
            Some(best) => notes.push(format!(
                "cheapest seller {} ({}) {}",
                best.seller,
                best.pos.as_str(),
                format::money(&best.effective)
            )),
            None if trip.is_some_and(|t| t.carrier.is_none()) => {
                notes.push("airline shown once confirmed".to_owned())
            }
            None => {}
        }
        if let Some(a) = award {
            notes.push(format!(
                "award seen {}",
                format::clock(a.best.observed_at)
            ));
        }
        let footer = div()
            .flex()
            .items_center()
            .gap(px(20.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_LABEL))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(16.))
                            .child(legend_item(theme::YELLOW, "this trip"))
                            .child(legend_item(STRIP_BELOW, "below usual"))
                            .child(legend_item(STRIP_TYPICAL, "near usual"))
                            .child(legend_item(STRIP_ABOVE, "above usual"))
                            .when(award.is_some(), |l| {
                                l.child(award_legend(
                                    AwardMark::BothLegs,
                                    "award, both legs",
                                ))
                                .child(
                                    award_legend(AwardMark::OneLeg, "one leg"),
                                )
                            }),
                    )
                    .child(notes.join(" · ")),
            )
            .child(actions);

        Some(
            div()
                .flex_none()
                .mx(px(24.))
                .mb(px(24.))
                .flex()
                .flex_col()
                .gap(px(16.))
                .px(px(24.))
                .py(px(20.))
                .rounded(px(16.))
                .bg(rgba(0x0C1222EB))
                .border_1()
                .border_color(color(theme::BORDER_INPUT))
                .shadow_lg()
                .debug_selector(|| "detail-card".into())
                .relative()
                .child(header)
                .children(
                    award
                        .filter(|_| !leads)
                        .map(|a| card_award_block(fonts, a)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(strip_head)
                        .child(strip),
                )
                .child(footer)
                // New observations for this route just arrived (0057).
                .children(
                    widgets::fresh_tint(self.glow(Fresh::Route(key)))
                        .map(|t| t.rounded(px(16.))),
                ),
        )
    }
}

/// The price strip's bar area, in pixels.
const STRIP_H: f32 = 116.0;
/// Bar colours: under, near and over the usual price for that far out.
const STRIP_BELOW: u32 = 0x3B8FA0;
const STRIP_TYPICAL: u32 = 0x36506F;
const STRIP_ABOVE: u32 = 0x263247;

/// Every departure's price as a bar (taller is dearer), coloured
/// against the usual price for that far out, the selected trip in
/// yellow, the usual price as a dashed line, and each month's cheapest
/// under it (0076). A bar picks its departure.
fn price_strip(
    fonts: &Fonts,
    days: &[&CalendarDay],
    selected: Option<NaiveDate>,
    usual: Option<i64>,
    marks: bool,
    cx: &mut Context<TravelMonitor>,
) -> Div {
    let prices: Vec<i64> = days
        .iter()
        .filter_map(|d| d.price.map(|p| p.minor))
        .collect();
    let lo = prices.iter().copied().min().unwrap_or(0) as f32;
    let hi = prices.iter().copied().max().unwrap_or(1) as f32;
    let span = (hi - lo).max(1.0);
    // The cheapest bar stays visible; the dearest fills the strip.
    let height =
        |minor: i64| 12.0 + (minor as f32 - lo) / span * (STRIP_H - 12.0);
    let bars = days.iter().enumerate().map(|(i, day)| {
        let slot = div()
            .id(SharedString::from(format!("strip-{i}")))
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(1.))
            .flex()
            .items_end();
        let Some(price) = day.price else {
            return slot
                .child(div().w_full().h(px(3.)).bg(color(theme::BORDER)));
        };
        let fill = if selected == Some(day.date) {
            theme::YELLOW
        } else {
            match day.tone {
                CalendarTone::Deal | CalendarTone::Below => STRIP_BELOW,
                CalendarTone::Typical => STRIP_TYPICAL,
                _ => STRIP_ABOVE,
            }
        };
        let date = day.date;
        slot.cursor_pointer()
            .hover(|s| s.opacity(0.75))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.departure = Some(date);
                this.refresh(cx);
                cx.notify();
            }))
            .child(
                div()
                    .w_full()
                    .h(px(height(price.minor)))
                    .rounded_t(px(2.))
                    .bg(color(fill)),
            )
    });
    let usual_line = usual
        .filter(|u| (*u as f32) >= lo && (*u as f32) <= hi)
        .map(|u| {
            let bottom = height(u);
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(bottom))
                .border_t_1()
                .border_dashed()
                .border_color(color(theme::TEXT_DIM))
                .child(
                    mono(
                        fonts,
                        format!(
                            "usual {}",
                            format::money(&Money::new(u, Currency::brl()))
                        ),
                    )
                    .absolute()
                    .right_0()
                    .bottom(px(3.))
                    .pl(px(6.))
                    .bg(rgba(0x0C1222EB))
                    .text_size(px(11.))
                    .text_color(color(theme::TEXT_MUTED)),
                )
        });
    // Each month's share of the strip, and its cheapest departure.
    let mut months: Vec<(NaiveDate, usize, Option<i64>)> = Vec::new();
    for day in days {
        let price = day.price.map(|p| p.minor);
        match months.last_mut() {
            Some((first, n, min)) if first.month() == day.date.month() => {
                *n += 1;
                *min = match (*min, price) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
            }
            _ => months.push((day.date, 1, price)),
        }
    }
    let total = days.len().max(1) as f32;
    let best_month = months.iter().filter_map(|m| m.2).min();
    let month_row = div()
        .flex()
        .border_t_1()
        .border_color(color(theme::BORDER))
        .pt(px(6.))
        .children(months.iter().map(|(first, n, min)| {
            let best = min.is_some() && *min == best_month;
            // A sliver of a month (the strip's last days) keeps its
            // divider but not its label.
            let labelled = *n >= 12;
            div()
                .w(relative(*n as f32 / total))
                .flex_none()
                .min_w_0()
                .overflow_hidden()
                .pl(px(4.))
                .border_l_1()
                .border_color(color(theme::BORDER))
                .flex()
                .flex_col()
                .gap(px(1.))
                .when(labelled, |m| {
                    m.child(
                        div()
                            .text_size(px(12.))
                            .whitespace_nowrap()
                            .text_color(color(theme::TEXT_MUTED))
                            .child(format::month(*first)),
                    )
                })
                .children(min.filter(|_| labelled).map(|m| {
                    mono(fonts, format::money(&Money::new(m, Currency::brl())))
                        .text_size(px(12.))
                        .whitespace_nowrap()
                        .text_color(color(if best {
                            theme::YELLOW
                        } else {
                            theme::TEXT_SOFT
                        }))
                }))
        }));
    // Award marks under the bars (0083): filled for both legs, hollow
    // for one.
    let mark_row = marks.then(|| {
        div()
            .flex()
            .h(px(8.))
            .debug_selector(|| "award-marks".into())
            .children(days.iter().map(|d| {
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .justify_center()
                    .children(award_dot(d.award, 5.))
            }))
    });
    div()
        .flex()
        .flex_col()
        .debug_selector(|| "price-strip".into())
        .child(
            div()
                .relative()
                .h(px(STRIP_H))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_end()
                        .children(bars),
                )
                .children(usual_line),
        )
        .children(mark_row)
        .child(month_row)
}

/// A lavender dot for a trip with an award: filled for both legs,
/// hollow for one.
pub(super) fn award_dot(mark: AwardMark, size: f32) -> Option<Div> {
    let dot = div().size(px(size)).flex_none().rounded_full();
    match mark {
        AwardMark::None => None,
        AwardMark::BothLegs => Some(dot.bg(color(theme::MILES))),
        AwardMark::OneLeg => {
            Some(dot.border_1().border_color(color(theme::MILES)))
        }
    }
}

fn award_legend(mark: AwardMark, text: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .children(award_dot(mark, 8.))
        .child(text)
}

/// `Out Nov 10, LA8148 nonstop · PREMIUM BUSINESS STANDARD`.
fn award_leg_text(a: &AwardSummary) -> String {
    let q = &a.best;
    let when = match (q.leg, q.return_date) {
        (_, Some(r)) => format::date_range(q.departure, r),
        (Some(tm_domain::Leg::Return), None) => {
            format!("Return {}", format::short_date(q.departure))
        }
        _ => format!("Out {}", format::short_date(q.departure)),
    };
    let mut parts = vec![match q.route_label() {
        Some(r) => format!("{when}, {r}"),
        None => when,
    }];
    parts.extend(q.brand.clone());
    parts.join(" · ")
}

/// A miles-led card's price: the award big, its taxes, its change
/// against the award series' usual, and the cash fare for scale.
fn award_price_block(
    fonts: &Fonts,
    a: &AwardSummary,
    trip: Option<&Trip>,
) -> Div {
    let deal = a.status.is_deal();
    let unit = a.best.unit;
    let vs = match (a.change, a.baseline_miles) {
        (Some(c), Some(b)) => Some(format!(
            "{} vs usual {} {}",
            format::change(c),
            format::miles(b),
            unit.label()
        )),
        _ if a.history_days < 28 => Some("no history yet".to_owned()),
        _ => None,
    };
    let mut scale = Vec::new();
    scale.extend(
        a.cash
            .as_ref()
            .or(trip.map(|t| &t.price))
            .map(|c| format!("cash {}", format::money(c))),
    );
    scale.extend(format::seats(a.best.seats));
    div()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(4.))
        .ml_auto()
        .flex_none()
        .child(
            widgets::miles_text(fonts, a.best.price.miles, unit)
                .text_size(px(30.))
                .font_weight(FontWeight::BOLD)
                .text_color(color(if deal {
                    theme::PRICE_DEAL
                } else {
                    theme::TEXT
                })),
        )
        .child(
            mono(
                fonts,
                format!("+ {} taxes", format::money(&a.best.price.taxes)),
            )
            .text_color(color(theme::TEXT_MUTED)),
        )
        .children(vs.map(|v| {
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(if deal {
                    theme::YELLOW
                } else {
                    theme::TEXT_FAINT
                }))
                .child(v)
        }))
        .children((!scale.is_empty()).then(|| {
            div()
                .text_size(px(12.))
                .text_color(color(theme::TEXT_FAINT))
                .child(scale.join(" · "))
        }))
}

/// The value line of an award (0083): its measured value of a mile
/// against the cheapest cash fare of its leg and date, the user's
/// valuation, and the cost with bought miles. Display only: miles and
/// cash never share a baseline (0023).
pub(super) fn value_line(a: &AwardSummary) -> Vec<String> {
    let q = &a.best;
    let mut parts = Vec::new();
    match (a.rate_against_cash(), &a.cash, a.rate) {
        (Some(r), Some(cash), _) => parts.push(format!(
            "{} vs cash {}",
            format::mile_value(r),
            format::money(cash)
        )),
        (_, _, Some(r)) => parts
            .push(format!("{} against its own fare", format::mile_value(r))),
        _ => {}
    }
    if let Some(u) = a.usual_rate.filter(|_| a.unusual) {
        parts.push(format!("usual {}", format::mile_value(u)));
    }
    if let Some(v) = &a.value_per_1000
        && let Some(text) = format::valued(&q.price, v)
    {
        parts.push(text.replacen(" at ", " at your ", 1));
    }
    if let Some(buy) = &a.buy_per_1000
        && let Some(b) =
            tm_scheduler::awards::buy_miles(&q.price, buy, a.cash.as_ref())
    {
        let saving = match &b.saving {
            Some(s) if s.minor >= 0 => {
                format!(", {} under cash", format::money(s))
            }
            Some(s) => format!(
                ", {} over cash",
                format::money(&tm_domain::Money::new(
                    -s.minor,
                    s.currency.clone()
                ))
            ),
            None => String::new(),
        };
        parts.push(format!(
            "with miles bought at {}: {}{saving}",
            format::mile_value(buy.as_f64()),
            format::money(&b.cost)
        ));
    }
    parts
}

/// The card's award block under the cash price (0083): miles and taxes,
/// the program, leg, flight and brand, then the value line.
fn card_award_block(fonts: &Fonts, a: &AwardSummary) -> Div {
    let unit = a.best.unit;
    let (price, what) = match &a.trip {
        Some(t) if t.complete() => (
            t.price().unwrap_or_else(|| a.best.price.clone()),
            format!(
                "{} · {}",
                a.name,
                format::date_range(t.departure, t.return_date)
            ),
        ),
        Some(t) if t.outbound.is_some() || t.inbound.is_some() => {
            let (q, missing) = match (&t.outbound, &t.inbound) {
                (Some(o), _) => (o, "return not checked"),
                (None, Some(i)) => (i, "out not checked"),
                _ => (&a.best, ""),
            };
            let leg = if q.leg == Some(tm_domain::Leg::Return) {
                "return"
            } else {
                "out"
            };
            let mut parts = vec![a.name.clone()];
            parts.push(
                [
                    Some(format!("{leg} {}", format::short_date(q.departure))),
                    q.route_label(),
                    q.brand.clone(),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", "),
            );
            parts.push(missing.to_owned());
            (q.price.clone(), parts.join(" · "))
        }
        _ => (
            a.best.price.clone(),
            format!("{} · {}", a.name, award_leg_text(a)),
        ),
    };
    let value = value_line(a);
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .pl(px(12.))
        .border_l_2()
        .border_color(color(theme::MILES_BORDER))
        .debug_selector(|| "card-award".into())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(widgets::miles_tag(fonts))
                .child(
                    widgets::award_text(fonts, &price, unit)
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD),
                )
                .children(
                    a.status
                        .is_deal()
                        .then(|| widgets::change_chip(fonts, a.change, true)),
                )
                .children(
                    a.unusual
                        .then(|| a.rate.map(|r| widgets::rate_chip(fonts, r)))
                        .flatten(),
                ),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(color(theme::TEXT_MUTED))
                .child(what),
        )
        .children((!value.is_empty()).then(|| {
            div()
                .text_size(px(12.))
                .text_color(color(theme::TEXT_SOFT))
                .child(value.join(" · "))
        }))
}

/// Sellers per point of sale, the cheapest in yellow, then the
/// airline's own price. `quoted` adds the seller's own-currency column
/// (the route detail).
pub(super) fn sellers_table(
    fonts: &Fonts,
    d: &RouteDetail,
    quoted: bool,
) -> Div {
    let cheapest = d.sellers.iter().map(|s| s.effective.minor).min();
    let line = || {
        div()
            .flex()
            .gap(px(10.))
            .py(px(7.))
            .border_t_1()
            .border_color(color(theme::BORDER))
            .text_size(px(13.))
    };
    let rows = d.sellers.iter().map(|s| {
        let best = Some(s.effective.minor) == cheapest;
        line()
            .child(
                mono(fonts, s.pos.as_str().to_owned())
                    .w(px(36.))
                    .flex_none()
                    .text_color(color(theme::TEXT_MUTED)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color(theme::TEXT_SOFT))
                    .child(s.seller.clone()),
            )
            .when(quoted, |l| {
                l.child(
                    mono(
                        fonts,
                        s.quoted.as_ref().map_or_else(
                            || format::money(&s.effective),
                            format::money,
                        ),
                    )
                    .w(px(84.))
                    .flex_none()
                    .text_right()
                    .text_color(color(theme::TEXT_FAINT)),
                )
            })
            .child(
                mono(fonts, format::money(&s.effective))
                    .w(px(96.))
                    .flex_none()
                    .text_right()
                    .when(best, |p| {
                        p.text_color(color(theme::YELLOW))
                            .font_weight(FontWeight::BOLD)
                    }),
            )
    });
    let direct = d.airline_direct.as_ref().map(|(name, price)| {
        line()
            .text_color(color(theme::TEXT_LABEL))
            .child(div().w(px(36.)).flex_none())
            .child(div().flex_1().child(if quoted {
                format!("{name} direct")
            } else {
                "Airline direct".to_owned()
            }))
            .when(quoted, |l| l.child(div().w(px(84.)).flex_none()))
            .child(
                mono(fonts, format::money(price))
                    .w(px(96.))
                    .flex_none()
                    .text_right(),
            )
    });
    let empty = d.sellers.is_empty().then(|| {
        div()
            .text_size(px(12.))
            .text_color(color(theme::TEXT_FAINT))
            .child("Sellers are read when a deal is confirmed.")
    });
    // The program as a seller (0083): its award in the price column,
    // the valued ≈R$ in the quoted one. Never the cheapest seller: it is
    // not a price in reais.
    let award = d.awards.as_ref().filter(|_| quoted).and_then(|a| {
        let out = a.outbound.first();
        let rt = a.round_trip.first();
        let back = a.inbound.first();
        let (price, what) = match (rt, out, back) {
            (Some(q), _, _) => (q.price.clone(), "round trip"),
            (None, Some(o), Some(b)) => (o.price.plus(&b.price)?, "both legs"),
            (None, Some(o), None) => {
                (o.price.clone(), "out only · return not checked")
            }
            (None, None, Some(b)) => {
                (b.price.clone(), "return only · out not checked")
            }
            (None, None, None) => return None,
        };
        let valued = a
            .value_per_1000
            .as_ref()
            .and_then(|v| tm_scheduler::awards::cost_at(&price, v))
            .map(|m| format!("≈{}", format::money(&m)));
        Some(
            line()
                .debug_selector(|| "award-seller".into())
                .child(
                    mono(fonts, "BR")
                        .w(px(36.))
                        .flex_none()
                        .text_color(color(theme::TEXT_MUTED)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_color(color(theme::TEXT_SOFT))
                                .child(widgets::miles_tag(fonts))
                                .child(a.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(color(theme::TEXT_FAINT))
                                .child(what),
                        ),
                )
                .child(
                    mono(fonts, valued.unwrap_or_default())
                        .w(px(84.))
                        .flex_none()
                        .text_right()
                        .text_color(color(theme::TEXT_FAINT)),
                )
                .child(
                    div()
                        .w(px(96.))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_end()
                        .child(widgets::miles_text(fonts, price.miles, a.unit))
                        .child(
                            mono(
                                fonts,
                                format!("+{}", format::money(&price.taxes)),
                            )
                            .text_size(px(11.))
                            .text_color(color(theme::TEXT_FAINT)),
                        ),
                ),
        )
    });
    div()
        .flex()
        .flex_col()
        .debug_selector(|| "sellers".into())
        .children(empty)
        .children(rows)
        .children(direct)
        .children(award)
}

/// `Nov 10 one way`, `Nov 17 return`, `Nov 10 – 17`: what an award
/// prices.
fn award_dates(a: &AwardSummary) -> String {
    let q = &a.best;
    match (q.return_date, q.leg) {
        (Some(r), _) => format::date_range(q.departure, r),
        (None, Some(tm_domain::Leg::Return)) => {
            format!("{} return", format::short_date(q.departure))
        }
        (None, _) => format!("{} one way", format::short_date(q.departure)),
    }
}

/// A cash row's award line: the row's own dates when known (`63,848 mi
/// + R$69 out · return not checked`, or both ways), else the cheapest
/// award on sale (`63,848 mi + R$69 · Nov 12 one way`).
pub(super) fn award_line(fonts: &Fonts, a: &AwardSummary) -> Div {
    let unit = a.best.unit;
    let line = div().flex().items_baseline().gap(px(4.)).min_w_0();
    match &a.trip {
        Some(t) if t.complete() => match t.price() {
            Some(p) => line.child(widgets::award_text(fonts, &p, unit)).child(
                div().truncate().child(format!(
                    "· {}",
                    format::date_range(t.departure, t.return_date)
                )),
            ),
            None => line,
        },
        Some(t) => {
            let (quote, text) = match (&t.outbound, &t.inbound) {
                (Some(o), _) => (o, "out · return not checked"),
                (None, Some(i)) => (i, "return · out not checked"),
                _ => (&a.best, ""),
            };
            line.child(widgets::award_text(fonts, &quote.price, unit))
                .child(div().truncate().child(text))
        }
        None => line
            .child(widgets::award_text(fonts, &a.best.price, unit))
            .child(div().truncate().child(format!("· {}", award_dates(a)))),
    }
}

/// `no history`: an award series under four weeks old cannot be a deal
/// yet (0030).
fn no_history_chip(fonts: &Fonts) -> Div {
    mono(fonts, "no history")
        .px(px(8.))
        .py(px(2.))
        .rounded(px(10.))
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .bg(color(theme::FLAT_BG))
        .text_color(color(theme::TEXT_LABEL))
}
