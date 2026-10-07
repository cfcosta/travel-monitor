//! Settings (DECISIONS.md 0035, 0036, 0038, 0039, 0043): origins and
//! destinations, points of sale, passengers, trip range, horizon, IOF,
//! card spread, budget, intervals, and the miles programs: their logins
//! and, per program, whether it runs, the value of a mile, the price it
//! sells miles at, its daily award loads and which dates it checks
//! (0083).

use gpui::{
    AnyElement,
    App,
    Context,
    Div,
    Entity,
    FontWeight,
    SharedString,
    div,
    prelude::*,
    px,
};
use tm_domain::{
    AwardDates,
    CountryCode,
    Currency,
    Money,
    PlaceKind,
    ProgramSettings,
    Settings,
    SiteLimits,
};

use super::{
    TravelMonitor,
    text_field::TextField,
    widgets::{self, mono},
};
use crate::{
    data::{Login, MilesRow, NewPlace, NewPos, Role, Snapshot},
    format,
    theme::{self, Fonts, color},
};

/// The numeric settings, in the order they are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Adults,
    Children,
    InfantsInSeat,
    InfantsOnLap,
    MinDays,
    MaxDays,
    RoundTripDays,
    Horizon,
    Iof,
    CardSpread,
    DealThreshold,
    UnusualRate,
    RandomShare,
    DailyBudget,
    MaxDailyBudget,
    MinInterval,
    MaxInterval,
}

impl Field {
    pub const GROUPS: [(&'static str, &'static [Field]); 5] = [
        (
            "Passengers",
            &[
                Field::Adults,
                Field::Children,
                Field::InfantsInSeat,
                Field::InfantsOnLap,
            ],
        ),
        (
            "Trips",
            &[
                Field::MinDays,
                Field::MaxDays,
                Field::RoundTripDays,
                Field::Horizon,
            ],
        ),
        (
            "Prices",
            &[
                Field::Iof,
                Field::CardSpread,
                Field::DealThreshold,
                Field::UnusualRate,
            ],
        ),
        (
            "Budget",
            &[
                Field::DailyBudget,
                Field::MaxDailyBudget,
                Field::RandomShare,
            ],
        ),
        ("Intervals", &[Field::MinInterval, Field::MaxInterval]),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Field::Adults => "Adults",
            Field::Children => "Children",
            Field::InfantsInSeat => "Infants (seat)",
            Field::InfantsOnLap => "Infants (lap)",
            Field::MinDays => "Shortest trip, days",
            Field::MaxDays => "Longest trip, days",
            Field::RoundTripDays => "Round trip tracked, days",
            Field::Horizon => "Horizon, days",
            Field::Iof => "IOF, %",
            Field::CardSpread => "Card spread, %",
            Field::DealThreshold => "Deal threshold, %",
            Field::UnusualRate => "Unusual mile value, %",
            Field::RandomShare => "Random checks, %",
            Field::DailyBudget => "Google Flights loads per day",
            Field::MaxDailyBudget => "Google Flights most per day",
            Field::MinInterval => "Check at most every, h",
            Field::MaxInterval => "Check at least every, h",
        }
    }

    /// The value as typed in the field (percentages as percent).
    pub fn show(self, s: &Settings) -> String {
        let pct = |f: f64| {
            let s = format::percent(f);
            s.trim_end_matches('%').to_owned()
        };
        match self {
            Field::Adults => s.passengers.adults.to_string(),
            Field::Children => s.passengers.children.to_string(),
            Field::InfantsInSeat => s.passengers.infants_in_seat.to_string(),
            Field::InfantsOnLap => s.passengers.infants_on_lap.to_string(),
            Field::MinDays => s.trips.min_days.to_string(),
            Field::MaxDays => s.trips.max_days.to_string(),
            Field::RoundTripDays => s.trips.round_trip_days.to_string(),
            Field::Horizon => s.horizon_days.to_string(),
            Field::Iof => pct(s.iof),
            Field::CardSpread => pct(s.card_spread),
            Field::DealThreshold => pct(s.deal_threshold),
            Field::UnusualRate => pct(s.unusual_rate),
            Field::RandomShare => pct(s.random_share),
            Field::DailyBudget => s.daily_budget.to_string(),
            Field::MaxDailyBudget => s.max_daily_budget.to_string(),
            Field::MinInterval => s.min_interval_hours.to_string(),
            Field::MaxInterval => s.max_interval_hours.to_string(),
        }
    }

    /// Writes `text` into `s`, or says why it cannot.
    pub fn parse_into(
        self,
        text: &str,
        s: &mut Settings,
    ) -> Result<(), String> {
        let text = text.trim().trim_end_matches('%').trim().replace(',', ".");
        let bad = || format!("{}: \"{text}\" is not a number", self.label());
        let int = |max: u64| -> Result<u64, String> {
            let n: u64 = text.parse().map_err(|_| bad())?;
            if n > max {
                return Err(format!("{}: at most {max}", self.label()));
            }
            Ok(n)
        };
        let pct = || -> Result<f64, String> {
            let v: f64 = text.parse().map_err(|_| bad())?;
            if !(0.0..=100.0).contains(&v) {
                return Err(format!("{}: between 0 and 100", self.label()));
            }
            Ok(v / 100.0)
        };
        match self {
            Field::Adults => s.passengers.adults = int(9)? as u8,
            Field::Children => s.passengers.children = int(9)? as u8,
            Field::InfantsInSeat => {
                s.passengers.infants_in_seat = int(9)? as u8
            }
            Field::InfantsOnLap => s.passengers.infants_on_lap = int(9)? as u8,
            Field::MinDays => s.trips.min_days = int(60)? as u16,
            Field::MaxDays => s.trips.max_days = int(60)? as u16,
            Field::RoundTripDays => s.trips.round_trip_days = int(60)? as u16,
            Field::Horizon => s.horizon_days = int(365)? as u16,
            Field::Iof => s.iof = pct()?,
            Field::CardSpread => s.card_spread = pct()?,
            Field::DealThreshold => s.deal_threshold = pct()?,
            Field::UnusualRate => s.unusual_rate = pct()?,
            Field::RandomShare => s.random_share = pct()?,
            Field::DailyBudget => s.daily_budget = int(100_000)? as u32,
            Field::MaxDailyBudget => s.max_daily_budget = int(100_000)? as u32,
            Field::MinInterval => s.min_interval_hours = int(24 * 60)? as u32,
            Field::MaxInterval => s.max_interval_hours = int(24 * 60)? as u32,
        }
        Ok(())
    }
}

/// Parses every field over `base` and validates the result.
pub fn parse_settings(
    base: &Settings,
    texts: &[(Field, String)],
) -> Result<Settings, String> {
    let mut s = base.clone();
    for (field, text) in texts {
        field.parse_into(text, &mut s)?;
    }
    if s.trips.round_trip_days < s.trips.min_days
        || s.trips.round_trip_days > s.trips.max_days
    {
        return Err(
            "The round-trip length must be inside the trip range".to_owned()
        );
    }
    if s.max_daily_budget < s.daily_budget {
        return Err("The most loads per day must be at least the daily budget"
            .to_owned());
    }
    s.validate().map_err(|e| {
        let text = e.to_string();
        let mut chars = text.chars();
        chars
            .next()
            .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
            .unwrap_or_default()
    })?;
    Ok(s)
}

/// One miles program's inputs (0083). The value and the buy price are
/// reais per 1,000 miles, empty for none.
#[derive(Clone)]
pub struct ProgramForm {
    pub program: String,
    pub value: Entity<TextField>,
    pub buy: Entity<TextField>,
    pub loads: Entity<TextField>,
    pub most: Entity<TextField>,
    /// What the fields were filled from.
    loaded: (ProgramSettings, SiteLimits),
}

/// `20`, `17.50`: reais per 1,000 miles, as typed.
fn per_1000_text(m: Option<&Money>) -> String {
    m.map(|m| {
        let s = format!("{:.2}", m.as_f64());
        s.strip_suffix(".00").map(str::to_owned).unwrap_or(s)
    })
    .unwrap_or_default()
}

/// Reads reais per 1,000 miles (`20`, `17,50`, `R$ 17.50`); empty is
/// none.
pub fn parse_per_1000(
    label: &str,
    text: &str,
) -> Result<Option<Money>, String> {
    let text = text
        .trim()
        .trim_start_matches("R$")
        .trim()
        .replace(',', ".");
    if text.is_empty() {
        return Ok(None);
    }
    let v: f64 = text
        .parse()
        .map_err(|_| format!("{label}: \"{text}\" is not an amount"))?;
    if !(v > 0.0 && v < 1000.0) {
        return Err(format!("{label}: between 0 and 1,000 reais"));
    }
    Ok(Some(Money::new(
        (v * 100.0).round() as i64,
        Currency::brl(),
    )))
}

/// A program's award loads: today's budget and its most (0060).
pub fn parse_loads(loads: &str, most: &str) -> Result<SiteLimits, String> {
    let int = |label: &str, text: &str| -> Result<u32, String> {
        let n: u32 = text.trim().parse().map_err(|_| {
            format!("{label}: \"{}\" is not a number", text.trim())
        })?;
        if n == 0 || n > 10_000 {
            return Err(format!("{label}: between 1 and 10,000"));
        }
        Ok(n)
    };
    let start = int("Award loads per day", loads)?;
    let max = int("Most per day", most)?;
    if max < start {
        return Err(
            "The most award loads per day must be at least the daily loads"
                .to_owned(),
        );
    }
    Ok(SiteLimits { start, max })
}

impl ProgramForm {
    fn new(
        m: &MilesRow,
        limits: SiteLimits,
        cx: &mut Context<SettingsForm>,
    ) -> Self {
        let field = |text: String,
                     placeholder: &str,
                     cx: &mut Context<SettingsForm>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| TextField::new(text, placeholder, cx).mono())
        };
        Self {
            program: m.program.clone(),
            value: field(
                per_1000_text(m.settings.value_per_1000.as_ref()),
                "none",
                cx,
            ),
            buy: field(
                per_1000_text(m.settings.buy_per_1000.as_ref()),
                "none",
                cx,
            ),
            loads: field(limits.start.to_string(), "", cx),
            most: field(limits.max.to_string(), "", cx),
            loaded: (m.settings.clone(), limits),
        }
    }

    /// The program's settings and award loads as typed, over `base`.
    pub fn parse(
        &self,
        base: &ProgramSettings,
        cx: &App,
    ) -> Result<(ProgramSettings, SiteLimits), String> {
        let text = |e: &Entity<TextField>| e.read(cx).text().to_owned();
        let limits = parse_loads(&text(&self.loads), &text(&self.most))?;
        Ok((
            ProgramSettings {
                value_per_1000: parse_per_1000(
                    "Value of 1,000 miles",
                    &text(&self.value),
                )?,
                buy_per_1000: parse_per_1000(
                    "Buy price of 1,000 miles",
                    &text(&self.buy),
                )?,
                ..base.clone()
            },
            limits,
        ))
    }

    fn dirty(&self, cx: &App) -> bool {
        let text = |e: &Entity<TextField>| e.read(cx).text().to_owned();
        let (s, l) = &self.loaded;
        text(&self.value) != per_1000_text(s.value_per_1000.as_ref())
            || text(&self.buy) != per_1000_text(s.buy_per_1000.as_ref())
            || text(&self.loads) != l.start.to_string()
            || text(&self.most) != l.max.to_string()
    }
}

/// A program's award budget as the snapshot has it, or its defaults.
fn limits_of(s: &Snapshot, m: &MilesRow) -> SiteLimits {
    let site = m.site();
    s.daemon
        .sites
        .iter()
        .find(|r| r.site == site)
        .and_then(|r| {
            Some(SiteLimits {
                start: r.budget?,
                max: r.max_budget?,
            })
        })
        .unwrap_or_else(|| s.settings.default_limits(&site))
}

/// The settings screen's inputs.
#[derive(Clone)]
pub struct SettingsForm {
    pub fields: Vec<(Field, Entity<TextField>)>,
    /// The settings the fields were filled from.
    loaded: Settings,
    pub place_role: Role,
    pub place_city: bool,
    pub code: Entity<TextField>,
    pub name: Entity<TextField>,
    pub country: Entity<TextField>,
    pub lat: Entity<TextField>,
    pub lon: Entity<TextField>,
    pub pos_country: Entity<TextField>,
    pub pos_currency: Entity<TextField>,
    /// One per miles program with a reader (0083).
    pub programs: Vec<ProgramForm>,
    pub error: Option<SharedString>,
}

impl SettingsForm {
    pub fn new(
        snapshot: Option<&Snapshot>,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let loaded = snapshot.map(|s| s.settings.clone()).unwrap_or_default();
        let field =
            |text: String, placeholder: &str, cx: &mut Context<Self>| {
                let placeholder = placeholder.to_owned();
                cx.new(|cx| TextField::new(text, placeholder, cx).mono())
            };
        let fields = Field::GROUPS
            .iter()
            .flat_map(|(_, fs)| fs.iter().copied())
            .map(|f| (f, field(f.show(&loaded), "", cx)))
            .collect();
        Self {
            fields,
            loaded,
            place_role: Role::Destination,
            place_city: false,
            code: field(String::new(), "LIS", cx),
            name: cx.new(|cx| TextField::new("", "Lisbon", cx)),
            country: field(String::new(), "PT", cx),
            lat: field(String::new(), "38.77", cx),
            lon: field(String::new(), "-9.13", cx),
            pos_country: field(String::new(), "DE", cx),
            pos_currency: field(String::new(), "EUR", cx),
            programs: snapshot
                .map(|s| {
                    s.miles
                        .iter()
                        .filter(|m| m.has_reader)
                        .map(|m| ProgramForm::new(m, limits_of(s, m), cx))
                        .collect()
                })
                .unwrap_or_default(),
            error: None,
        }
    }

    /// Takes a program's saved values from a snapshot, unless they are
    /// being edited.
    fn sync_programs(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        for m in snapshot.miles.iter().filter(|m| m.has_reader) {
            let limits = limits_of(snapshot, m);
            match self.programs.iter().position(|p| p.program == m.program) {
                None => {
                    let form = ProgramForm::new(m, limits, cx);
                    self.programs.push(form);
                }
                Some(i) if !self.programs[i].dirty(cx) => {
                    if self.programs[i].loaded != (m.settings.clone(), limits) {
                        self.programs[i] = ProgramForm::new(m, limits, cx);
                    }
                }
                Some(_) => {}
            }
        }
    }

    pub fn texts(&self, cx: &App) -> Vec<(Field, String)> {
        self.fields
            .iter()
            .map(|(f, e)| (*f, e.read(cx).text().to_owned()))
            .collect()
    }

    /// Whether any field differs from what it was filled with.
    pub fn dirty(&self, cx: &App) -> bool {
        self.fields
            .iter()
            .any(|(f, e)| e.read(cx).text() != f.show(&self.loaded))
    }

    /// Takes new settings from a snapshot, unless the person is editing.
    pub fn sync(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        self.sync_programs(snapshot, cx);
        if snapshot.settings == self.loaded || self.dirty(cx) {
            return;
        }
        self.reload(&snapshot.settings, cx);
    }

    pub fn reload(&mut self, settings: &Settings, cx: &mut Context<Self>) {
        self.loaded = settings.clone();
        for (f, e) in &self.fields {
            let text = f.show(settings);
            e.update(cx, |e, cx| e.set_text(text, cx));
        }
        cx.notify();
    }

    pub fn loaded(&self) -> &Settings {
        &self.loaded
    }

    /// The place typed in the form.
    pub fn new_place(&self, cx: &App) -> Result<NewPlace, String> {
        let text = |e: &Entity<TextField>| e.read(cx).text().trim().to_owned();
        let code = text(&self.code).to_ascii_uppercase();
        if code.len() != 3 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err("The code must be three letters (IATA)".to_owned());
        }
        let name = text(&self.name);
        if name.is_empty() {
            return Err("The place needs a name".to_owned());
        }
        let country = text(&self.country).to_ascii_uppercase();
        if country.len() != 2
            || !country.chars().all(|c| c.is_ascii_alphabetic())
        {
            return Err("The country must be two letters (ISO 3166)".to_owned());
        }
        let coord = |e: &Entity<TextField>,
                     what: &str,
                     limit: f64|
         -> Result<f64, String> {
            let v: f64 = text(e)
                .replace(',', ".")
                .parse()
                .map_err(|_| format!("The {what} must be a number"))?;
            if v.abs() > limit {
                return Err(format!("The {what} must be within ±{limit}"));
            }
            Ok(v)
        };
        Ok(NewPlace {
            code,
            name,
            country: CountryCode::new(&country),
            kind: if self.place_city {
                PlaceKind::City { freebase_id: None }
            } else {
                PlaceKind::Airport
            },
            lat: coord(&self.lat, "latitude", 90.0)?,
            lon: coord(&self.lon, "longitude", 180.0)?,
        })
    }

    pub fn new_pos(&self, cx: &App) -> Result<NewPos, String> {
        let country =
            self.pos_country.read(cx).text().trim().to_ascii_uppercase();
        if country.len() != 2 {
            return Err(
                "The point of sale is a two-letter country code".to_owned()
            );
        }
        let currency = self
            .pos_currency
            .read(cx)
            .text()
            .trim()
            .to_ascii_uppercase();
        let currency = if currency.is_empty() {
            "BRL".to_owned()
        } else {
            currency
        };
        if currency.len() != 3 {
            return Err("The currency is a three-letter code".to_owned());
        }
        Ok(NewPos {
            country: CountryCode::new(&country),
            currency: Currency::new(&currency),
        })
    }

    pub fn clear_place(&mut self, cx: &mut Context<Self>) {
        for e in [&self.code, &self.name, &self.country, &self.lat, &self.lon] {
            e.update(cx, |e, cx| e.set_text("", cx));
        }
    }
}

fn card(title: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .px(px(18.))
        .py(px(16.))
        .rounded(px(14.))
        .border_1()
        .border_color(color(theme::BORDER))
        .bg(color(theme::SECTION))
        .child(widgets::heading(title, 15.))
}

fn labelled(
    label: impl Into<SharedString>,
    field: &Entity<TextField>,
    width: f32,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .w(px(width))
        .child(widgets::label(label))
        .child(field.clone())
}

impl TravelMonitor {
    pub(super) fn render_settings(
        &mut self,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (Some(s), Some(form)) =
            (&self.snapshot, self.settings_form.clone())
        else {
            return div().into_any_element();
        };
        let f = form.read(cx).clone();
        let groups = Field::GROUPS.iter().map(|(title, fields)| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(color(theme::TEXT_MUTED))
                        .child(*title),
                )
                .child(div().flex().flex_wrap().gap(px(10.)).children(
                    fields.iter().filter_map(|field| {
                        let entity =
                            f.fields.iter().find(|(x, _)| x == field)?;
                        Some(labelled(field.label(), &entity.1, 150.))
                    }),
                ))
        });
        let save = widgets::primary_button("save-settings", "Save settings")
            .on_click(cx.listener(|this, _, _, cx| this.save_settings(cx)));
        let revert = widgets::ghost_button("revert-settings", "Revert")
            .on_click(cx.listener(|this, _, _, cx| {
                if let (Some(form), Some(s)) =
                    (this.settings_form.clone(), this.snapshot.as_ref())
                {
                    let settings = s.settings.clone();
                    form.update(cx, |form, cx| {
                        form.error = None;
                        form.reload(&settings, cx);
                    });
                }
            }));
        let general = card("Search")
            .children(groups)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child("Changing the passengers starts new price series (0036). Effective prices include IOF and the card spread (0038)."),
            )
            .children(f.error.clone().map(|e| {
                div()
                    .text_size(px(13.))
                    .text_color(color(theme::RED))
                    .debug_selector(|| "settings-error".into())
                    .child(e)
            }))
            .child(div().flex().gap(px(8.)).child(save).child(revert));

        let place_row = |role: Role,
                         p: &tm_domain::Place,
                         cx: &mut Context<Self>| {
            let id = p.id;
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(6.))
                .border_t_1()
                .border_color(color(theme::ROW_RULE))
                .child(
                    mono(fonts, p.code.clone())
                        .w(px(44.))
                        .flex_none()
                        .font_weight(FontWeight::BOLD),
                )
                .child(
                    div().flex_1().min_w_0().truncate().child(p.name.clone()),
                )
                .child(
                    mono(fonts, p.country.as_str().to_owned())
                        .w(px(28.))
                        .flex_none()
                        .text_color(color(theme::TEXT_FAINT)),
                )
                .child(
                    div()
                        .w(px(60.))
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(color(theme::TEXT_FAINT))
                        .child(match p.kind {
                            PlaceKind::Airport => "airport",
                            PlaceKind::City { .. } => "city",
                        }),
                )
                .child(
                    mono(fonts, format!("{:.2}, {:.2}", p.lat, p.lon))
                        .w(px(110.))
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(color(theme::TEXT_FAINT)),
                )
                .child(
                    widgets::small_button(
                        SharedString::from(format!("remove-{role:?}-{id}")),
                        "Remove",
                    )
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            let backend = this.env.backend.clone();
                            this.write(
                            async move { backend.remove_place(role, id).await },
                            "Removed; its history is kept",
                            cx,
                        );
                        },
                    )),
                )
        };
        let origins: Vec<_> = s
            .origins
            .iter()
            .map(|p| place_row(Role::Origin, p, cx))
            .collect();
        let destinations: Vec<_> = s
            .destinations
            .iter()
            .map(|p| place_row(Role::Destination, p, cx))
            .collect();
        let toggle = |id: &'static str, label: &'static str, on: bool| {
            widgets::segment(id, label, on).py(px(4.))
        };
        let add_place = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .pt(px(6.))
            .child(widgets::label("Add a place"))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        widgets::segments()
                            .child(toggle("role-destination", "Destination", f.place_role == Role::Destination).on_click(
                                cx.listener(|this, _, _, cx| this.edit_form(cx, |f| f.place_role = Role::Destination)),
                            ))
                            .child(toggle("role-origin", "Origin", f.place_role == Role::Origin).on_click(
                                cx.listener(|this, _, _, cx| this.edit_form(cx, |f| f.place_role = Role::Origin)),
                            )),
                    )
                    .child(
                        widgets::segments()
                            .child(
                                toggle("kind-airport", "Airport", !f.place_city)
                                    .on_click(cx.listener(|this, _, _, cx| this.edit_form(cx, |f| f.place_city = false))),
                            )
                            .child(
                                toggle("kind-city", "City", f.place_city)
                                    .on_click(cx.listener(|this, _, _, cx| this.edit_form(cx, |f| f.place_city = true))),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .items_end()
                    .child(labelled("IATA code", &f.code, 80.))
                    .child(labelled("Name", &f.name, 180.))
                    .child(labelled("Country", &f.country, 80.))
                    .child(labelled("Latitude", &f.lat, 100.))
                    .child(labelled("Longitude", &f.lon, 100.))
                    .child(
                        widgets::ghost_button("add-place", "Add")
                            .py(px(6.))
                            .on_click(cx.listener(|this, _, _, cx| this.add_place(cx))),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child("A city (SAO) covers all its airports; the daemon learns its Google ID on the first search (0041). Adding a removed place back continues its series."),
            );
        let places = card("Origins and destinations")
            .child(widgets::label("Origins"))
            .child(div().flex().flex_col().children(origins))
            .child(widgets::label(format!(
                "Destinations ({})",
                s.destinations.len()
            )))
            .child(div().flex().flex_col().children(destinations))
            .child(add_place);

        let pos_rows: Vec<_> = s
            .points_of_sale
            .iter()
            .map(|p| {
                let id = p.id;
                let active = p.active;
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(color(theme::ROW_RULE))
                    .child(
                        mono(fonts, p.country.as_str().to_owned())
                            .w(px(40.))
                            .font_weight(FontWeight::BOLD),
                    )
                    .child(
                        mono(fonts, p.currency.as_str().to_owned())
                            .w(px(50.))
                            .text_color(color(theme::TEXT_MUTED)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(color(theme::TEXT_FAINT))
                            .child(match (p.is_home, p.active) {
                                (true, _) => "home: no IOF or spread",
                                (false, true) => {
                                    "foreign: IOF and spread apply"
                                }
                                (false, false) => "paused",
                            }),
                    )
                    .when(!p.is_home, |d| {
                        d.child(
                            widgets::small_button(
                                SharedString::from(format!("pos-{id}")),
                                if active { "Pause" } else { "Resume" },
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let backend = this.env.backend.clone();
                                    this.write(
                                        async move {
                                            backend
                                                .set_pos_active(id, !active)
                                                .await
                                        },
                                        if active {
                                            "Point of sale paused"
                                        } else {
                                            "Point of sale resumed"
                                        },
                                        cx,
                                    );
                                },
                            )),
                        )
                    })
            })
            .collect();
        let pos = card("Points of sale")
            .child(div().flex().flex_col().children(pos_rows))
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .items_end()
                    .child(labelled("Country", &f.pos_country, 80.))
                    .child(labelled("Currency", &f.pos_currency, 80.))
                    .child(
                        widgets::ghost_button("add-pos", "Add")
                            .py(px(6.))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.add_pos(cx)),
                            ),
                    ),
            );

        let miles_rows: Vec<_> = s
            .miles
            .iter()
            .map(|m| self.render_program(s, &f, m, cx))
            .collect();
        let miles = card("Miles programs")
            .debug_selector(|| "miles-programs".into())
            .child(div().flex().flex_col().children(miles_rows))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child("Log in opens the program's own Chromium profile in a window, after the daemon pauses that program's site; every other site keeps running. Log in by hand (2FA included), then close the window; no password is stored (0024, 0062). Award checks run under your own account, so the defaults are small (0083)."),
            );
        let site_rows: Vec<Div> = s
            .daemon
            .sites
            .iter()
            .map(|site| {
                let budget = match (site.budget, site.max_budget) {
                    (Some(b), Some(m)) => format!(
                        "{}/{} · max {}",
                        format::thousands(site.loads_today),
                        format::thousands(b),
                        format::thousands(m)
                    ),
                    _ => "no loads yet".to_owned(),
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(color(theme::ROW_RULE_FAINT))
                    .text_size(px(13.))
                    .child(
                        div().w(px(90.)).flex_none().child(site.label.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(color(theme::TEXT_MUTED))
                            .child(if site.state.is_empty() {
                                "–".to_owned()
                            } else {
                                site.state.clone()
                            }),
                    )
                    .child(widgets::mono(fonts, budget).flex_none())
            })
            .collect();
        let no_sites = site_rows.is_empty().then(|| {
            div()
                .text_size(px(13.))
                .text_color(color(theme::TEXT_FAINT))
                .child("The daemon has not run yet.")
        });
        let sites = card("Site budgets")
            .debug_selector(|| "site-budgets".into())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(no_sites)
                    .children(site_rows),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child("Each site has its own daily budget, ramp and backoff, and its own browser (0060, 0062). Google Flights' is set above (or by the daemon's --budget); a miles program's award loads are set in its block below (40 a day, at most 80, to start); other sites start at 300 loads a day and ramp to 1,000, or follow the daemon's --site-budget."),
            );

        div()
            .id("settings-screen")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(20.))
                    .px(px(28.))
                    .py(px(22.))
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(520.))
                            .flex()
                            .flex_col()
                            .gap(px(20.))
                            .child(general)
                            .child(places),
                    )
                    .child(
                        div()
                            .w(px(420.))
                            .flex()
                            .flex_col()
                            .gap(px(20.))
                            .child(pos)
                            .child(sites)
                            .child(miles),
                    ),
            )
            .into_any_element()
    }

    /// One program's block (0083): its login, and for a program the
    /// daemon reads, whether it runs, the value of a mile (the measured
    /// one beside it), the buy price, its award loads and its dates.
    fn render_program(
        &self,
        s: &Snapshot,
        f: &SettingsForm,
        m: &MilesRow,
        cx: &mut Context<Self>,
    ) -> Div {
        let program = m.program.clone();
        let busy = self.logins.contains(&m.program);
        let (state, state_color) = match m.login {
            Login::LoggedIn => ("Logged in".to_owned(), theme::TEAL),
            Login::LoggedOut => ("Not logged in".to_owned(), theme::TEXT_FAINT),
            Login::Expired => {
                ("Session expired: log in again".to_owned(), theme::YELLOW)
            }
        };
        let mut state = vec![state];
        if m.login == Login::LoggedIn
            && let Some(until) = m.session_until
        {
            state.push(format!(
                "session until {}",
                format::short_date(until.date_naive())
            ));
        }
        if let Some(at) = m.last_checked {
            state.push(format!("checked {}", format::clock(at)));
        }
        let head = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .w(px(140.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(m.name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.))
                    .text_color(color(state_color))
                    .child(if busy {
                        "Login window open…".to_owned()
                    } else {
                        state.join(" · ")
                    }),
            )
            .child({
                let program = program.clone();
                widgets::small_button(
                    SharedString::from(format!("login-{}", m.program)),
                    "Log in",
                )
                .when(busy, |b| b.opacity(0.5))
                .on_click(cx.listener(
                    move |this, _, _, cx| this.start_login(program.clone(), cx),
                ))
            });
        let block = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .py(px(10.))
            .border_t_1()
            .border_color(color(theme::ROW_RULE))
            .child(head);
        let note = |text: String| {
            div()
                .text_size(px(12.))
                .text_color(color(theme::TEXT_FAINT))
                .child(text)
        };
        let Some(form) = f
            .programs
            .iter()
            .find(|p| p.program == m.program)
            .filter(|_| m.has_reader)
        else {
            let unit = match m.unit {
                tm_domain::MilesUnit::Points => " Prices in points: pts.",
                tm_domain::MilesUnit::Miles => "",
            };
            return block.child(note(format!(
                "No award reader yet: the site has not been probed with a logged-in, headed browser (0050, 0052).{unit}"
            )));
        };
        let active = m.active();
        let toggle = |on: bool, label: &'static str, cx: &mut Context<Self>| {
            let program = program.clone();
            let base = m.settings.clone();
            widgets::segment(
                SharedString::from(format!("active-{}-{on}", m.program)),
                label,
                active == on,
            )
            .py(px(4.))
            .on_click(cx.listener(move |this, _, _, cx| {
                let settings = ProgramSettings {
                    active: on,
                    ..base.clone()
                };
                this.save_program(program.clone(), settings, None, cx);
            }))
        };
        let measured = match m.usual_rate {
            Some(r) => format!(
                "{} pays {} against its own fares (median of {} checks, last 30 days).",
                m.name,
                format::mile_value_long(r, m.unit),
                m.rate_checks
            ),
            None => format!(
                "{}'s value of a mile is measured from its awards once it has some.",
                m.name
            ),
        };
        let used = s
            .daemon
            .sites
            .iter()
            .find(|r| r.site == m.site())
            .map(|r| format!("{} used today", r.loads_today))
            .unwrap_or_default();
        let dates =
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(AwardDates::ALL.map(|d| {
                    let program = program.clone();
                    let base = m.settings.clone();
                    widgets::segment(
                        SharedString::from(format!(
                            "dates-{}-{d:?}",
                            m.program
                        )),
                        d.label(),
                        m.settings.dates == d,
                    )
                    .justify_start()
                    .py(px(4.))
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            let settings = ProgramSettings {
                                dates: d,
                                ..base.clone()
                            };
                            this.save_program(
                                program.clone(),
                                settings,
                                None,
                                cx,
                            );
                        },
                    ))
                }));
        let save = {
            let program = program.clone();
            widgets::ghost_button(
                SharedString::from(format!("save-{}", m.program)),
                format!("Save {}", m.name),
            )
            .py(px(6.))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.save_program_form(&program, cx)
            }))
        };
        block
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(widgets::label("Award checks"))
                    .child(
                        widgets::segments()
                            .child(toggle(true, "On", cx))
                            .child(toggle(false, "Paused", cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .items_end()
                    .child(labelled("Value of 1,000 miles, R$", &form.value, 170.))
                    .child(labelled("Buy price of 1,000, R$", &form.buy, 170.)),
            )
            .child(note(measured))
            .child(note(
                "Empty values show no \u{2248}R$ figures. With a buy price, the route detail says what an award costs paid with bought miles, against the cash fare.".to_owned(),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .items_end()
                    .child(labelled("Award loads per day", &form.loads, 140.))
                    .child(labelled("Most per day", &form.most, 110.))
                    .child(div().pb(px(8.)).text_size(px(12.)).text_color(color(theme::TEXT_FAINT)).child(used)),
            )
            .child(widgets::label("Dates to check"))
            .child(dates)
            .child(note(format!(
                "Runs in a headed Chromium on an offscreen display, with this profile. Economy and business come in one load. The {} row under Site budgets is the same figure; edit it here.",
                m.name
            )))
            .child(div().flex().child(save))
    }

    /// Saves a program's settings, and its award loads when given.
    fn save_program(
        &mut self,
        program: String,
        settings: ProgramSettings,
        limits: Option<SiteLimits>,
        cx: &mut Context<Self>,
    ) {
        let backend = self.env.backend.clone();
        let site = tm_domain::Source::Miles {
            program: program.clone(),
        }
        .site();
        self.write(
            async move {
                backend.save_program(program, settings).await?;
                if let Some(limits) = limits {
                    backend.set_site_limits(site, limits).await?;
                }
                Ok(())
            },
            "Miles program saved",
            cx,
        );
    }

    /// Saves what the program's fields hold.
    pub(super) fn save_program_form(
        &mut self,
        program: &str,
        cx: &mut Context<Self>,
    ) {
        let (Some(form), Some(base)) = (
            self.settings_form.clone(),
            self.snapshot
                .as_ref()
                .and_then(|s| s.miles.iter().find(|m| m.program == program))
                .map(|m| m.settings.clone()),
        ) else {
            return;
        };
        let parsed = {
            let f = form.read(cx);
            f.programs
                .iter()
                .find(|p| p.program == program)
                .map(|p| p.parse(&base, cx))
        };
        match parsed {
            Some(Ok((settings, limits))) => {
                form.update(cx, |f, cx| {
                    f.error = None;
                    if let Some(p) =
                        f.programs.iter_mut().find(|p| p.program == program)
                    {
                        p.loaded = (settings.clone(), limits);
                    }
                    cx.notify();
                });
                self.save_program(
                    program.to_owned(),
                    settings,
                    Some(limits),
                    cx,
                );
            }
            Some(Err(error)) => form.update(cx, |f, cx| {
                f.error = Some(error.into());
                cx.notify();
            }),
            None => {}
        }
    }

    fn edit_form(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut SettingsForm),
    ) {
        if let Some(form) = &self.settings_form {
            form.update(cx, |form, cx| {
                f(form);
                cx.notify();
            });
        }
        cx.notify();
    }

    pub(super) fn save_settings(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.settings_form.clone() else {
            return;
        };
        let (base, texts) = {
            let f = form.read(cx);
            (f.loaded().clone(), f.texts(cx))
        };
        match parse_settings(&base, &texts) {
            Err(error) => form.update(cx, |f, cx| {
                f.error = Some(error.into());
                cx.notify();
            }),
            Ok(settings) => {
                form.update(cx, |f, cx| {
                    f.error = None;
                    f.reload(&settings, cx);
                });
                let backend = self.env.backend.clone();
                self.write(
                    async move { backend.save_settings(settings).await },
                    "Settings saved",
                    cx,
                );
            }
        }
    }

    fn add_place(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.settings_form.clone() else {
            return;
        };
        let (role, place) = {
            let f = form.read(cx);
            (f.place_role, f.new_place(cx))
        };
        match place {
            Err(error) => form.update(cx, |f, cx| {
                f.error = Some(error.into());
                cx.notify();
            }),
            Ok(place) => {
                form.update(cx, |f, cx| {
                    f.error = None;
                    f.clear_place(cx);
                });
                let backend = self.env.backend.clone();
                self.write(
                    async move { backend.add_place(role, place).await },
                    "Place added",
                    cx,
                );
            }
        }
    }

    fn add_pos(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.settings_form.clone() else {
            return;
        };
        let pos = form.read(cx).new_pos(cx);
        match pos {
            Err(error) => form.update(cx, |f, cx| {
                f.error = Some(error.into());
                cx.notify();
            }),
            Ok(pos) => {
                let backend = self.env.backend.clone();
                self.write(
                    async move { backend.add_pos(pos).await },
                    "Point of sale added",
                    cx,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_values_are_reais_per_1000_miles() {
        assert_eq!(parse_per_1000("x", ""), Ok(None));
        assert_eq!(parse_per_1000("x", "20"), Ok(Some(Money::brl_whole(20))));
        assert_eq!(
            parse_per_1000("x", "R$ 17,50"),
            Ok(Some(Money::new(1750, Currency::brl())))
        );
        assert!(parse_per_1000("x", "abc").is_err());
        assert!(parse_per_1000("x", "0").is_err());
        assert_eq!(
            per_1000_text(Some(&Money::new(1750, Currency::brl()))),
            "17.50"
        );
        assert_eq!(per_1000_text(Some(&Money::brl_whole(20))), "20");
        assert_eq!(
            parse_loads("40", "80"),
            Ok(SiteLimits { start: 40, max: 80 })
        );
        assert!(parse_loads("80", "40").is_err());
        assert!(parse_loads("0", "40").is_err());
    }

    fn texts(s: &Settings) -> Vec<(Field, String)> {
        Field::GROUPS
            .iter()
            .flat_map(|(_, fs)| fs.iter().map(|f| (*f, f.show(s))))
            .collect()
    }

    #[test]
    fn defaults_round_trip_through_the_fields() {
        let s = Settings::default();
        assert_eq!(parse_settings(&s, &texts(&s)), Ok(s.clone()));
        assert_eq!(Field::Iof.show(&s), "3.5");
        assert_eq!(Field::CardSpread.show(&s), "1");
    }

    #[test]
    fn edits_are_parsed_and_validated() {
        let s = Settings::default();
        let mut t = texts(&s);
        for (f, text) in &mut t {
            match f {
                Field::Adults => *text = "2".into(),
                Field::Iof => *text = "4,38%".into(),
                Field::MaxDays => *text = "12".into(),
                _ => {}
            }
        }
        let parsed = parse_settings(&s, &t).expect("valid");
        assert_eq!(parsed.passengers.adults, 2);
        assert!((parsed.iof - 0.0438).abs() < 1e-9);
        assert_eq!(parsed.trips.max_days, 12);

        let mut t = texts(&s);
        t[0].1 = "0".into();
        assert_eq!(
            parse_settings(&s, &t),
            Err("At least one adult must travel".into())
        );
        let mut t = texts(&s);
        t[0].1 = "two".into();
        assert!(
            parse_settings(&s, &t).is_err_and(|e| e.contains("not a number"))
        );
        let mut t = texts(&s);
        for (f, text) in &mut t {
            if *f == Field::RoundTripDays {
                *text = "14".into();
            }
        }
        assert!(
            parse_settings(&s, &t).is_err_and(|e| e.contains("round-trip"))
        );
    }
}
