# Architecture

How the crates fit together. Decisions are numbered in
[DECISIONS.md](../DECISIONS.md); research is in [research/](research/).

```
           ┌────────────┐   requests table   ┌──────────────┐
           │  tm (app)  │ ─────────────────▶ │ tmd (daemon) │
           │   gpui     │ ◀───────────────── │              │
           └─────┬──────┘   reads SQLite     └──────┬───────┘
                 │                                   │ drives
                 ▼                                   ▼
            tm-store  ◀──────── writes ──────── tm-browser ── CDP ──▶ headless Chromium
                                                                     (one per site, 0062)
                 ▲
   tm-scheduler (pure) ◀── daemon asks ──
   tm-gflights  (pure) ◀── URLs + parsers ──
   tm-domain    (types, everyone)
```

## Crates and their public surface

Every crate depends on `tm-domain`. Only `tm-store` touches SQLite, and
only `tm-browser` touches Chromium. `tm-gflights` and `tm-scheduler` do no I/O, so they are tested with
fixtures and properties alone.

### tm-domain (done)

Places, routes, cabins, points of sale, money (integer minor units),
query families (`FamilyKey` = route × cabin × source × pos × kind,
`FamilyKind::{Leg, RoundTrip, Explore}`), observations, Explore hints,
deals, seller quotes, query reasons and outcomes, settings and their
validation. Read the source: it is the shared vocabulary.

Awards (`award`, 0083): `AwardPrice` (miles + taxes, `63,848 mi +
R$69`), `MilesUnit` (`mi`, `pts`), `Seats`, `AwardDetail` on an
observation (fare before taxes in BRL, brand, fare basis, seats),
`Observation::flights` (the itinerary key, every segment's flight and
departure), and `Series` (`Cash` or `Award { program }`) on a deal. A
deal is a round trip, or for an award one leg (`leg`, no
`return_date`) or its round trip; award deals carry miles, the award
series' baseline in miles, brand, seats and the cash fare of the same
dates for scale. `ProgramSettings` per miles program: active, the
user's value of 1,000 miles and the program's price for 1,000 (both
unset by default), and `AwardDates`.

### tm-gflights (pure)

Everything about Google Flights that is not driving a browser.

- `tfs`: `Search { legs: Vec<LegSpec>, trip: Trip, cabin, passengers }`,
  `LegSpec { date, from: PlaceRef, to: PlaceRef }`,
  `PlaceRef::{Airport(code), City(freebase_id)}`,
  `Trip::{RoundTrip, OneWay}`; `encode(&Search) -> String`,
  `decode(&str) -> Result<Search, TfsError>`. Field layout:
  docs/research/google-flights.md.
- `url`: `Locale { hl, gl: CountryCode, curr: Currency }`;
  `search_url(&Search, &Locale)`, `explore_url(origin_query, cabin,
  &Locale)` (uses `q=`: "Flights from SAO", plus " business class"),
  `resolve_url(text, &Locale)` (a `q=` search used to learn a city's
  Freebase ID from the RPC it triggers).
- `rpc`: `Method::{ShoppingResults, CalendarGraph, CalendarGrid,
  ExploreDestinations, BookingResults, …}`, `method_of(url)`;
  `frames(body)` decodes `)]}'` + `rt=c` chunked responses to the inner
  `wrb.fr` JSON; typed parsers:
  `parse_calendar_request(form) -> CalendarRequest`,
  `parse_calendar(body) -> Vec<CalendarCell { departure, return_date, price }>`
  (graph and grid, one-way and round trip),
  `parse_explore(body) -> Vec<ExploreDestination>`,
  `parse_shopping_results(body) -> Vec<Itinerary>`,
  `parse_booking_results(body) -> Vec<SellerOffer>`.
  Every parser is tolerant of extra fields and strict about the ones it
  reads: a missing price is `None`; a structure it cannot recognise is
  an error, and the check ends as `Unexpected` (0059).
- `page`: page-level knowledge as data, used by the daemon through
  `tm-browser`: JS snippets and selectors (open the price graph, press
  "Scroll forward", read date-grid aria-labels such as
  `"R$3,972, cheapest price, Nov 7 to Nov 18"`), and block detection
  (`detect_block(url, text) -> Option<Block>`: `/sorry/`, "unusual
  traffic", reCAPTCHA).
- `plan`: how many price-graph pages cover a horizon (first page ~60
  days, then 21 per scroll).

### tm-browser

Chromium and CDP, async (tokio + tokio-tungstenite).

- `Chromium::launch(ChromiumConfig { binary, user_data_dir, headless,
  port, window, extra_args })` → a process whose browser endpoint is
  connected; `shutdown()`.
- `Browser::connect(ws_url)`, `Browser::new_context(ContextOptions {
  proxy, locale, timezone })` (`Target.createBrowserContext`, so each
  point of sale or proxy gets isolated cookies, 0033).
- `Page`: `navigate`, `url`, `eval`, `wait_until(js, timeout)`, `click`,
  `type_text`, `press_key`, `add_init_script`, `screenshot`,
  `set_timezone`/`set_locale`, `close`.
- `Page::capture(url_contains) -> Capture`: records matching XHR/fetch
  responses through the Network domain (request post data, status,
  body). `Capture::wait_for(n, timeout)`, `drain()`. This is how
  `GetCalendarGraph` & co. are read (0032).
- `profile`: a profile's cookie jar read while no Chromium runs on it
  (`login_cookies`, 0064), and `site_profile_dir(base, site)`: each
  site's profile is `<profile-dir>/<site slug>` (`google_flights`,
  `airline-tp`, `miles-smiles`; `tm_domain::site_slug`). The daemon's
  Chromium for a site and the app's login window for it both open that
  directory (0062).

### tm-store

SQLite through sqlx with checked queries (`query!`/`query_as!`) and
committed offline data (`crates/tm-store/.sqlx`), WAL mode so the app
reads while the daemon writes (0020, 0040).

- `Store::open(path)` creates, migrates and returns a pool;
  `seed_defaults()` on an empty database: SAO, the watchlist of 0006,
  points of sale BR/US/PT/IN, default settings, airline and miles
  sources (0012, 0023).
- Configuration (0035): places (origins, destinations, with coordinates,
  country, IATA code, Freebase ID), points of sale, settings (one typed
  row), airline sources (code, name, served destinations), miles
  programs (login state). Routes are derived: every active origin ×
  active destination. Removing keeps history.
- Families: `sync_families(&Settings)` creates the families implied by
  routes × cabins × sources × points of sale × kinds (legs everywhere,
  round trips on routes not yet shown additive, Explore per origin and
  cabin); scheduler state per family (posteriors, hints, last check,
  last minimum, additive streak, round-trip discount) read and written
  as one record.
- Query log: `start_query`, `finish_query` (reason, score, runner-up,
  outcome, page loads, timings).
- Observations (append-only), Explore hints, deals (candidate →
  confirmed/rejected/expired, `seen` for the app), seller quotes.
- Requests (0046): the app enqueues (`CheckRoute`, `CheckFamily`,
  `ConfirmDeal`, `OpenLogin { program }`, `CloseLogin { program }`,
  `ReloadConfig`); the daemon claims and completes them. A
  `CloseLogin` without a program (an app before 0062) still reads.
- Budgets and blocks (0039, 0060): `site_budgets` has one row per site
  (`start_budget`, `daily_budget` in force, `max_daily_budget`,
  `loads_today`, `day`, `last_ramp_at`). A site without a row starts
  from its defaults (`Settings::default_limits`): Google Flights from
  the settings' `daily_budget` and `max_daily_budget`, a miles program
  from `SiteLimits::MILES` (40, ramping to 80; the app edits it, 0083),
  LATAM's cash site from `SiteLimits::LATAM` (300, ramping to 1,000) and
  any other site from `SiteLimits::OTHER` (1,200, ramping to 4,000;
  0088). `configure_site_budget` applies configured limits (a changed
  start resets the budget in force; an unchanged one keeps the ramp;
  today's count is kept); at each start the daemon applies Google
  Flights', each `--site-budget` and every other stored non-miles
  site's defaults, so a new default reaches existing rows; `add_site_loads` counts one site's loads. Migration 0003
  turned the old single `budget` row into Google Flights' row and
  dropped the table. Block events are per site.
- Exchange rates (0038). The heartbeat (process state, totals over the
  sites) and `site_status` (per site: `idle`, `checking`, `paused for
  login: …`, `cooling down`, `browser unavailable`, `stopped`; 0057,
  0062) with `site_checks` (the checks in progress, several per site,
  0072) for the app's status chip and activity screen.
- Awards (0083, migration 0006): observations gain `fare_minor`/
  `fare_currency`, `brand`, `fare_basis`, `seats`, `low_availability`
  and `flights`; `deals` was rebuilt so `return_date` may be NULL and
  gained `program`, `leg`, `miles`, `baseline_miles`, `brand`, `seats`,
  `low_availability`, `cash_minor`/`cash_currency`; `miles_programs`
  gained `value_minor`, `buy_minor` (BRL per 1,000 miles), `dates` and
  `session_until`; `settings` gained `unusual_rate`.
  `award_rates(program, since)` is the program's implied rate per
  award check; `save_miles_settings`, `set_miles_session_until`;
  `RequestKind::CheckAward { route, cabin, departure, return_date }`.
  A miles program always has its round-trip family, additive route or
  not.

### tm-scheduler (pure)

- `score`: the greedy value index of 0028 with Explore hints as
  effective time; Thompson sampling optional (seeded RNG passed in).
- `plan_tick(candidates, now, budget, params, rng) -> Vec<Pick>`:
  per-site limits, minimum and maximum interval, overdue families,
  ~10% random checks, then highest scores. Each pick carries its
  `QueryReason` (score and runner-up).
- `rates`: change-rate estimation from changed/unchanged bits with
  irregular gaps (Cho & Garcia-Molina; interval-censored MLE).
- `update_after_check`: posterior updates from a check's summary.
- `hints`: Explore hint → families, hint precision learning.
- `fanout`: a drop raises related families (other cabins, points of
  sale, sources, legs ↔ round trip).
- `baseline`: days-to-departure buckets (3/7/14/21/45/90/180), Brazilian
  holiday and peak-season flags, 60-day median and MAD (0030).
- `deals`: `evaluate` → `Option<(DealReason, change)>`.
- `awards` (0083): award series (`cheapest_by_dates`, `award_points`
  in miles, for the same baselines and `evaluate`, kept apart from
  cash), the implied value of a mile (`implied_rate`: fare before taxes
  ÷ miles × 1,000), the program's `usual_rate` (30-day median of its
  checks) and `is_unusual`, `rate_against_cash` (display only), and
  `cost_at` / `buy_miles` (an award paid with miles at a value or a
  bought price, and its saving against cash).
- `trips`: legs → trips in the 5–10 day range, with the learned
  round-trip discount; discount learning and the additive stop rule
  (0042, 0043).
- `budget`: the conservative-then-ramp state machine of 0039. It knows
  nothing of sites' limits: the daemon runs one per site, with that
  site's start, maximum and floor in a copy of `Params` (0060).

### tm-daemon (`tmd`)

A coordinator and one worker per site (0060, 0062).

- **Coordinator** (`daemon`): start-up (seed, budget flags into the
  sites' rows, requeue, sync), the global heartbeat every 30 s, the
  configuration reload published to the workers, and the requests
  table. Requests are routed by site: a route's manual check to every
  site it has families on (the request completes when the last site's
  check ends), a deal confirmation to Google Flights, `OpenLogin` and
  `CloseLogin` to the program's `miles:<program>` site alone. It starts
  a worker for every site with eligible families (and for a site a
  login names) and stops them all on SIGINT/SIGTERM.
- **Worker** (`worker`), a tokio task per site, owns the site's
  `Sources` (its own Chromium on `<profile-dir>/<site slug>`, started
  when the site has work, closed after 5 idle minutes), its `Budget`
  (count, ramp, halving and cooldown of its own), its pacing, and its
  own `plan_tick` over the site's candidates with the site's budget.
  It runs up to three checks at once (`--parallel`, 0072), each on its
  own tab: it reloads, takes requests, and starts manual checks and
  picks in turn (0071) on free lanes, each lane resting 4–10 s after
  its check. A check's fetch runs on a task of its own; when it lands,
  the worker writes observations, updates scheduler state, evaluates
  trips and confirms deals (Google Flights only) and raises a desktop
  notification for each confirmed one (`--no-notify` to skip, 0080), records blocks on its
  own budget, and writes its `site_status` and `site_checks` rows as
  checks start and end. A `Pause` closes its browser for a login window; `Resume` reads
  the program's profile for a session (0064) and lets the site run.
  Different sites run at the same time; a block, cooldown or login on
  one never slows another.
- Shared state is the store. Fan-out and Explore hints raise other
  sites' families; a check re-reads its family's state just before
  writing it, so a raise that lands while it runs is kept.

Flags: `--parallel` (default 3) is how many checks each site runs at
once; `--budget`/`--max-budget` are Google Flights' start and maximum
(the settings' `daily_budget`/`max_daily_budget`); `--site-budget
airline:TP=1200:4000` (repeatable) sets another site's; `--cdp-port` is
Google Flights' Chromium (other sites pick free ports). Sources: Google
Flights (legs, round trips, Explore, booking pages) and TAP's booking
site (0051). A check whose page does not produce what is expected ends
as `Unexpected` and is retried later. Other airline sites and miles
programs have no reader and are skipped (0052, 0059); their workers only
pause and resume for logins. `deals` has the award half ready for the
LATAM Pass reader (`find_awards`, `select_awards`, `award_candidate`,
`award_expired`), and `notify` words miles deals and "log in again"
(0083); `CheckAward` requests are answered "no award reader yet".

### tm-app (`tm`)

gpui-pre `=0.3.7`. The main screen is the map + list of 0044; also a
routes overview, a route detail with the departure × length grid,
activity, and settings that edit the configuration. Reads the database
directly; writes only configuration and requests. The status chip and
the activity screen show every check running now (one "Now checking"
row per busy site) and each site's loads and budget today (`Google
1,234/4,800 · TAP 40/300`); settings list the site budgets read-only
(Google Flights' stay editable as settings). For a miles login (0024)
the app sends `OpenLogin`, waits until the daemon paused that program's
site, and opens the program's own profile (`<profile-dir>/miles-<program>`)
in a visible window; logins made in the shared profile before 0062 are
not carried over. Awards (0083): a Pay with switch (Both, Cash, Miles)
on the deals list, award lines and miles-deal rows, the award block on
the card and the detail (brands, taxes, seats, the measured value of a
mile against the program's usual rate, the cost with bought miles), and
a block per program in Settings (active, value, buy price, daily award
loads, dates).

### nix

`flake.nix` (dev shell, packages `tmd` and `tm`), `nix/packages.nix`,
`nix/hm-module.nix` (0037).
