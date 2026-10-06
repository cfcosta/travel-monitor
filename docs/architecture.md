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
                 ▲                                   │
   tm-scheduler (pure) ◀── daemon asks ──            │ Jev (navigation fallback,
   tm-gflights  (pure) ◀── URLs + parsers ──         │ airline and miles sites)
   tm-domain    (types, everyone)                 tm-jev
```

## Crates and their public surface

Every crate depends on `tm-domain`. Only `tm-store` touches SQLite, only
`tm-browser` touches Chromium, only `tm-jev` calls Jev over HTTP.
`tm-gflights` and `tm-scheduler` do no I/O, so they are tested with
fixtures and properties alone.

### tm-domain (done)

Places, routes, cabins, points of sale, money (integer minor units),
query families (`FamilyKey` = route × cabin × source × pos × kind,
`FamilyKind::{Leg, RoundTrip, Explore}`), observations, Explore hints,
deals, seller quotes, query reasons and outcomes, settings and their
validation. Read the source: it is the shared vocabulary.

### tm-jev

A copy of `../tau-agent/crates/plugins/tau-jev` (DECISIONS.md 0015),
without `tau-ai`: usage and cost are local types, and retry/backoff is
local code.

- `trait Jev: Send + Sync + 'static { async fn ask(&self, &Request) -> Result<Response, JevError>; }`
- `Request { state, questions }`, `Question::{Noul, Choice, Score}`,
  `Response`, `Answer` with strict validation (a missing or ill-typed
  answer is an error, never a default).
- `TypeSafe` (HTTPS client, `TYPESAFE_API_KEY`, retries 429/503/529
  honoring `retry-after`), `fake::FakeJev` for tests.
- `api_key_from_file(path)`: the daemon reads its key from a file
  (0037).

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
  an error that triggers Jev recovery (0041).
- `page`: page-level knowledge as data, used by the daemon through
  `tm-browser`: JS snippets and selectors (open the price graph, press
  "Scroll forward", read date-grid aria-labels such as
  `"R$3,972, cheapest price, Nov 7 to Nov 18"`), and block detection
  (`detect_block(url, text) -> Option<Block>`: `/sorry/`, "unusual
  traffic", reCAPTCHA).
- `plan`: how many price-graph pages cover a horizon (first page ~60
  days, then 21 per scroll).

### tm-browser

Chromium and CDP, async (tokio + tokio-tungstenite). Ported and adapted
from `../vs1/crates/vs1-browser` (DECISIONS.md 0016), keeping its
attribution.

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
- `Page::snapshot()`: vs1-browser's DOM observation (actionable controls
  with labels, values and kinds, visible text), with its input guards.
- `agent::run(&Page, &dyn Jev, &Task) -> AgentOutcome`: vs1-browser's
  Jev policy. A `Task` has a goal, a map of field values to type (no
  text model: Jev chooses which known value fits a field), step limits
  and optional checks. Outcomes: `Done`, `Blocked`, `Exhausted`, with a
  trace.

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
  outcome, page loads, Jev calls, timings).
- Observations (append-only), Explore hints, deals (candidate →
  confirmed/rejected/expired, `seen` for the app), seller quotes.
- Requests (0046): the app enqueues (`CheckRoute`, `CheckFamily`,
  `ConfirmDeal`, `OpenLogin`, `CloseLogin`, `ReloadConfig`); the daemon
  claims and completes them.
- Budget and blocks (0039): today's budget, loads used, block events
  per site.
- Exchange rates (0038), daemon heartbeat (state, next check, loads
  today) for the app's status chip.

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
- `trips`: legs → trips in the 5–10 day range, with the learned
  round-trip discount; discount learning and the additive stop rule
  (0042, 0043).
- `budget`: the conservative-then-ramp state machine of 0039.

### tm-daemon (`tmd`)

The loop: reload configuration, sync families, read requests, ask the
scheduler for this tick's picks, run each pick through its source, write
observations, update scheduler state, evaluate trips and deals, confirm
candidates, record blocks and adjust the budget, write the heartbeat.
Sources: Google Flights (legs, round trips, Explore, booking pages);
airline sites and miles programs through Jev-driven navigation, marked
unverified until probed (0047).

### tm-app (`tm`)

gpui-pre `=0.3.7`. The main screen is the map + list of 0044; also a
routes overview, a route detail with the departure × length grid,
activity, and settings that edit the configuration. Reads the database
directly; writes only configuration and requests. Shows a desktop
notification for each newly confirmed deal (0040). Opens the daemon's
Chromium profile in a visible window for miles logins (0024), after a
`OpenLogin` request has paused the daemon's browser.

### nix

`flake.nix` (dev shell, packages `tmd` and `tm`), `nix/packages.nix`,
`nix/hm-module.nix` (0037).
