# Decisions

Each decision is recorded once, in the order it was made. A later
decision that changes an earlier one says so and links back to it.

## 0001: Monitor travel prices departing from São Paulo

- Date: 2026-10-05
- Refined by [0035](#0035-origins-and-destinations-are-configured-in-the-app): SAO and this list are only the initial configuration.

The tool watches prices for trips that depart from São Paulo (city code
SAO). Which airports and destinations are included is decided separately.

## 0002: Written in Rust

- Date: 2026-10-05

The project is a Rust workspace, following the layout and tooling of
`../tau-agent` and `../vs1`.

## 0003: The interface uses gpui-pre

- Date: 2026-10-05

The UI is built with `gpui = { package = "gpui-pre", version = "=0.3.7" }`,
the same pin `../tau-agent` uses. When one of the two projects moves the
pin, the other moves with it.

## 0004: Browsing is driven by Jev over Chrome CDP

- Date: 2026-10-05
- Superseded in part by [0059](#0059-jev-is-removed): Jev is no longer used.

Prices are read from real pages in a Chrome/Chromium instance, controlled
through the Chrome DevTools Protocol. Jev, a decision model, chooses each
navigation step, as the proof of concept in `../vs1` (`vs1-browser`)
does. That proof of concept already reaches Google Flights results from a
natural-language goal (`../vs1/examples/flights.json`).

## 0005: The origin is the whole SAO city code

- Date: 2026-10-05
- Refined by [0035](#0035-origins-and-destinations-are-configured-in-the-app): SAO and this list are only the initial configuration.

Searches use SAO, so GRU, CGH and VCP all count. An observation records
the airport the fare actually departs from.

## 0006: A pinned watchlist plus an Explore sweep

- Date: 2026-10-05
- Refined by [0035](#0035-origins-and-destinations-are-configured-in-the-app): SAO and this list are only the initial configuration.

Destinations come from two sources:

1. **A pinned watchlist.** Each route is searched on its own. The first
   list (codes are cities or airports):
   - Brazil: RIO, BSB, SSA, REC, FOR, FLN, POA, NAT, MCZ, IGU
   - South America: BUE, SCL, MVD, LIM, BOG, BRC
   - North America and Caribbean: MIA, MCO, NYC, CUN, PUJ
   - Europe: LIS, OPO, MAD, BCN, PAR, LON, ROM
   - Long-haul: DXB, TYO
2. **An Explore sweep.** Google Flights Explore from SAO, with
   destination "anywhere", records the cheapest price shown for every
   destination. It finds cheap fares outside the watchlist.

The watchlist is data, not code, so routes can be added or removed
without a rebuild.

## 0007: Observations are round trips of fixed lengths

- Date: 2026-10-05
- Refined by [0042](#0042-both-legs-are-tracked-per-day-round-trips-where-they-differ).

A price observation is a round trip with a fixed trip length, sampled at
several departure dates. Because the length is fixed, prices for the same
route can be compared over time. The lengths and the date horizon are
decided separately.

## 0008: Track economy, premium economy and business

- Date: 2026-10-05

All three cabins are tracked. Premium economy and business are only
searched on routes where they exist; which routes those are is decided
separately.

## 0009: Seven-day trips, departing up to six months ahead

- Date: 2026-10-05
- Refined by [0042](#0042-both-legs-are-tracked-per-day-round-trips-where-they-differ).
- Completes [0007](#0007-observations-are-round-trips-of-fixed-lengths).

Every round trip is 7 days long, on every route. Departure dates cover
the next six months.

## 0010: Explore runs daily, the watchlist weekly

- Date: 2026-10-05
- Superseded by [0029](#0029-a-daily-budget-with-a-weekly-maximum-interval).

The Explore sweep runs once a day. Each watchlist route is refreshed
once a week. Routes are spread across the days of the week, so the
daily load stays about even.

## 0011: Prices come from Google Flights and airline sites

- Date: 2026-10-05

Google Flights is the main source. Airline sites are searched as well,
because they sometimes sell fares that Google Flights does not show.
Which airlines are included is decided separately.

## 0012: Airline sites to search directly

- Date: 2026-10-05
- Completes [0011](#0011-prices-come-from-google-flights-and-airline-sites).

- Brazilian carriers: LATAM, Azul, GOL.
- Long-haul carriers that fly from GRU: TAP, Iberia, Air France/KLM,
  American, United, Emirates, Qatar, Turkish.

An airline site is searched only for the routes that airline flies.
Each site is a separate integration, so they are added one at a time,
after Google Flights works.

## 0013: Departure dates come from Google's date grid

- Date: 2026-10-05
- Refined by [0042](#0042-both-legs-are-tracked-per-day-round-trips-where-they-differ).
- Completes [0009](#0009-seven-day-trips-departing-up-to-six-months-ahead).

For each route and cabin, the monitor reads Google Flights' date grid
and price graph with the trip length fixed at 7 days. A few page loads
cover six months of departures. A full search for one date is made only
when that date needs more detail, for example to confirm a low price.

## 0014: Jev learns a search once, then the URL is replayed

- Date: 2026-10-05
- Refined by [0041](#0041-google-flights-urls-are-generated-jev-handles-the-rest): for Google Flights, URLs are generated, not learned.

The first time a search is needed, Jev drives the browser to the results
page. The resulting URL (for Google Flights, the `tfs` parameter) is
saved. Later runs load the saved URL directly and read the prices.
Jev drives the browser again only when a replayed URL stops producing
the expected page, for example after the site changes.

## 0015: Our own Jev client, copied from tau-agent

- Date: 2026-10-05
- Superseded in part by [0059](#0059-jev-is-removed): Jev is no longer used.

The Jev client is a crate in this workspace, started as a copy of
`../tau-agent/crates/plugins/tau-jev`. That crate is used instead of the
`../vs1` client because its design is better: a `Jev` trait, a fake for
tests, and strict checks on every answer. Its `tau-ai` dependency (used
only for usage and cost types) is replaced with local types. There is no
dependency on `../vs1` or `../tau-agent`.

## 0016: The browser runtime is ported from vs1-browser

- Date: 2026-10-05
- Superseded in part by [0059](#0059-jev-is-removed): Jev is no longer used.

The CDP layer is a copy of `../vs1/crates/vs1-browser`, adapted to this
project: its DOM snapshot, its action guards (fresh node identity,
visibility and hit tests before input) and its Jev policy. It already
reaches Google Flights results. Its decision calls go through our own
Jev client ([0015](#0015-our-own-jev-client-copied-from-tau-agent)).
Upstream attribution (Browser Use's MIT-licensed Jev Ultrafast) is kept.

## 0017: A daemon collects prices; the gpui app shows them

- Date: 2026-10-05

There are two programs:

- **A daemon**, run as a systemd user unit, follows the schedule
  ([0010](#0010-explore-runs-daily-the-watchlist-weekly)), drives the
  browser and writes observations to the database.
- **A gpui app** reads the database, shows prices and deals, and can ask
  the daemon to start a run now.

Monitoring continues when the app is closed.

## 0018: A deal is a price below the route's own history

- Date: 2026-10-05

A fare is a deal when it is well below the rolling median for the same
route, cabin and trip length, or when it is the lowest price ever seen
for them. There are no hand-set price targets. A route has no deals
until it has enough history; the threshold and the window are decided
separately.

## 0019: Deals are shown in the app and as desktop notifications

- Date: 2026-10-05
- Refined by [0040](#0040-the-app-runs-on-the-same-box-and-raises-the-alerts): the app, not the daemon, raises the notification.

The app has a deals view. When the daemon finds a new deal, it also
sends a desktop notification. Nothing is sent off the machine.

## 0020: Observations are stored in SQLite through sqlx

- Date: 2026-10-05

One SQLite file holds all data, using `sqlx` with checked queries and
migrations, as `../tau-agent` does. The daemon writes to it and the app
reads it. Observations are append-only: a price is never updated, a new
observation is added, so the full history stays available for medians.

## 0021: The daemon runs its own headless Chromium

- Date: 2026-10-05

The daemon starts a dedicated headless Chromium with its own profile
directory and CDP port. It never attaches to the user's browser.

## 0022: A deal is 20% under the 60-day median, or an all-time low

- Date: 2026-10-05
- Superseded by [0030](#0030-deal-baselines-are-per-days-to-departure-bucket).
- Completes [0018](#0018-a-deal-is-a-price-below-the-routes-own-history).

A fare is a deal when either holds, for the same route, cabin and trip
length:

- it is at least 20% below the median of the last 60 days, and that
  route has at least 4 weeks of observations; or
- it is lower than every earlier observation.

## 0023: Award prices are tracked in Smiles, LATAM Pass and Azul Fidelidade

- Date: 2026-10-05

Besides cash fares in BRL, the monitor records award prices (miles or
points, plus taxes in BRL) from Smiles, LATAM Pass and Azul Fidelidade.
Award prices are a separate series and are never compared with cash
fares. Each program needs a logged-in session; how logins are handled is
decided separately.

## 0024: Miles programs are logged in by hand, once

- Date: 2026-10-05
- Completes [0023](#0023-award-prices-are-tracked-in-smiles-latam-pass-and-azul-fidelidade).

The app opens the daemon's Chromium profile
([0021](#0021-the-daemon-runs-its-own-headless-chromium)) in a visible
window, and the user logs in to each program by hand, including any 2FA.
The session cookies stay in that profile. No password is stored. A
Chromium profile can only be open in one process at a time, so the
daemon pauses its headless browser while the login window is open. When
a session expires, the daemon skips that program and the app asks the
user to log in again.

## 0025: The Explore sweep covers economy and business

- Date: 2026-10-05
- Refines [0006](#0006-a-pinned-watchlist-plus-an-explore-sweep).

The daily Explore sweep runs twice: once for economy and once for
business. Premium economy is only searched on the watchlist.

## 0026: Premium cabins are searched on international routes only

- Date: 2026-10-05
- Completes [0008](#0008-track-economy-premium-economy-and-business).

On the watchlist, premium economy and business are searched for every
route outside Brazil. Domestic routes are searched in economy only.

## 0027: Built with a Nix flake and versioned with jj

- Date: 2026-10-05

The repository follows `../tau-agent`: a `flake.nix` with the dev shell
and packages (the daemon, the app and its systemd user unit), a nightly
toolchain in `rust-toolchain.toml`, `clippy.toml` and `deny.toml`, and
jj colocated with git.

## 0028: The scheduler runs checks by a greedy value index

- Date: 2026-10-05
- Research: [docs/research/scheduling.md](docs/research/scheduling.md).

The unit of work is a *query*: one page load, such as a Google Flights
date grid, covering many cells (route × cabin × departure date ×
source). Each *query family* (route × cabin × source) keeps a learned
change rate, the probability that a change is a drop, a count of
Explore hints since its last check, and a cost. Each tick the daemon
scores every family:

```
q     = change rate × P(change is a drop)
τe    = time since last check + n_hints · (−ln(1 − hint precision)) / q
x     = q · τe
score = weight · (1 − e^{−x}(1 + x)) / (q · cost)
```

It then runs the highest-scoring queries that fit the tick's share of
the daily budget. This is the crawl value of Azar et al. (PNAS 2018),
with Explore hints counted as extra elapsed time as in Busa-Fekete
et al. (arXiv:2502.02430).

Guards:

- **Per-site rate limits:** Google Flights, each airline and each miles
  program has its own.
- **A minimum interval** between checks of the same family, so very
  volatile entries are not chased.
- **A maximum interval** after which a family is checked regardless of
  its score.
- **About 10% random checks**, marked as such, so change-rate estimates
  stay unbiased.
- **Fan-out:** when a query shows a drop, nearby dates, the other cabins
  and the same route on other sources get a higher score.

Change rates are learned per days-to-departure bucket. Every query
records why it ran and the score it had, so the policy can be replayed
and compared later.

## 0029: A daily budget with a weekly maximum interval

- Date: 2026-10-05
- Replaces [0010](#0010-explore-runs-daily-the-watchlist-weekly).

Fares change about once a day, and most low fares last about a day or
more, so weekly checks would miss them. The watchlist shares a daily
page-load budget, spent by the scheduler
([0028](#0028-the-scheduler-runs-checks-by-a-greedy-value-index)).
Every query family is still checked at least once a week: one week is
the maximum interval. The Explore sweeps
([0025](#0025-the-explore-sweep-covers-economy-and-business)) still
run daily.

## 0030: Deal baselines are per days-to-departure bucket

- Date: 2026-10-05
- Replaces [0022](#0022-a-deal-is-20-under-the-60-day-median-or-an-all-time-low).

Fares rise steeply in the last 21 days and move little beyond about 90
days, so a single median would label every far-out fare a deal and miss
deals close to departure. A baseline is kept per route × cabin × trip
length × days-to-departure bucket, with edges at 3, 7, 14, 21, 45, 90
and 180 days. Departures near Brazilian holidays and in peak season have
their own baselines.

A fare is a deal when it is well below its bucket's median, measured by
a robust spread (percentiles or median absolute deviation) over the last
60 days, or when it is the lowest ever seen in that bucket. The exact
cut-off is tuned on collected data. A bucket needs at least 4 weeks of
observations before it can flag deals. Award prices have their own
baselines.

## 0031: A deal is confirmed before the alert

- Date: 2026-10-05

A deal candidate triggers an immediate second query, and a check on the
airline's booking page when one exists. The notification is sent only
if the price holds. Both queries are logged, so we learn how often
first readings are wrong.

## 0032: Google Flights prices are read from the page's own RPC responses

- Date: 2026-10-05
- Research: [docs/research/google-flights.md](docs/research/google-flights.md).

The daemon loads a saved search URL, opens the price graph, and pages
forward about six times to cover six months of 7-day trips. It reads
the `GetCalendarGraph` responses the page itself makes, through CDP.
Each response holds `[departure, return, price]` entries with integer
prices. The date grid's cell aria-labels are the fallback when the RPC
format changes.

The daemon never calls these RPCs itself. They require a BotGuard token
that only the page's scripts generate, and the project does not forge
or replay it.

## 0033: Several points of sale are tracked continuously

- Date: 2026-10-05

Each watchlist route × cabin is also searched under foreign points of
sale, chosen with the `gl` URL parameter. The first set is BR (home),
US, PT and IN, the four tested in the research; the set is data, not
code. Each point of sale is a separate source in the scheduler
([0028](#0028-the-scheduler-runs-checks-by-a-greedy-value-index)), so
one that never beats Brazil is checked less and less.

Foreign prices are compared as an effective BRL cost: the converted
price plus IOF (3.5%) plus a configured card exchange spread. Baselines
and deals ([0030](#0030-deal-baselines-are-per-days-to-departure-bucket))
are per point of sale.

A VPN is not needed for Google Flights, because `gl` sets the point of
sale. Mullvad becomes relevant only if we check that an OTA honours its
foreign price when opened from a Brazilian IP.

## 0034: Record both the cheapest price and the airline's own price

- Date: 2026-10-05

The price graph gives the cheapest price from any seller, often a
low-cost online agency. When a deal is confirmed
([0031](#0031-a-deal-is-confirmed-before-the-alert)), the booking page
is opened and every seller's price is recorded, including the
airline's own. A deal alert shows both prices and names the seller.

## 0035: Origins and destinations are configured in the app

- Date: 2026-10-06
- Refines [0001](#0001-monitor-travel-prices-departing-from-são-paulo),
  [0005](#0005-the-origin-is-the-whole-sao-city-code) and
  [0006](#0006-a-pinned-watchlist-plus-an-explore-sweep).

Nothing about São Paulo is hard-coded. The configuration lives in the
SQLite database ([0020](#0020-observations-are-stored-in-sqlite-through-sqlx)),
and the gpui app is where it is edited:

- **Origins:** a list. Each entry is a city code (such as SAO) or a
  single airport (such as VCP).
- **Destinations:** a list, each with an optional weight for the
  scheduler.
- **Points of sale:** the list from
  [0033](#0033-several-points-of-sale-are-tracked-continuously).

Every origin × destination pair is a route. Each origin gets its own
daily Explore sweeps ([0025](#0025-the-explore-sweep-covers-economy-and-business)).
Whether a route is domestic, which decides its cabins
([0026](#0026-premium-cabins-are-searched-on-international-routes-only)),
comes from the countries of its origin and destination, not from a
fixed list.

Removing a route stops new checks but keeps its history; adding it back
continues the same series. The daemon picks up configuration changes
without a restart. On first run, the database is seeded with SAO and
the watchlist from 0006.

## 0036: The passenger count is one global setting

- Date: 2026-10-06

All searches use the same passengers: a number of adults (and,
if set, children and infants), configured once in the app, not per
route. Prices are stored as returned for the whole party, together with
the passenger count. Changing the count starts new series, because
prices for different party sizes are not comparable.

## 0037: The daemon runs on an always-on NixOS box, set up by a home-manager module

- Date: 2026-10-06
- Refines [0017](#0017-a-daemon-collects-prices-the-gpui-app-shows-them) and
  [0027](#0027-built-with-a-nix-flake-and-versioned-with-jj).

The flake exports a home-manager module. Enabling it installs the
daemon and runs it as a systemd user service, with its own headless
Chromium ([0021](#0021-the-daemon-runs-its-own-headless-chromium)). The
service must keep running without a login session, so the module
requires user lingering. Its options cover at least:

- `enable` and `package`;
- the database path;
- the Chromium package and the CDP port;
- the Jev API key, as a path to a file read at start-up (never a value
  in the Nix store);
- the daily page-load budget and per-site rate limits
  ([0039](#0039-the-page-load-budget-starts-conservative-and-ramps-up));
- the log level.

## 0038: The card's exchange spread is a setting

- Date: 2026-10-06
- Completes [0033](#0033-several-points-of-sale-are-tracked-continuously).

The effective BRL cost of a foreign-currency price is
`price × exchange rate × (1 + IOF) × (1 + card spread)`. IOF (3.5%) and
the card spread are both settings in the app, so either can change
without a rebuild. Observations store the raw price and currency. The
effective cost is computed when read, so changing a setting re-prices
the whole history.

## 0039: The page-load budget starts conservative and ramps up

- Date: 2026-10-06

The daemon starts at about 300 page loads a day, paced with random
jitter between loads. While no CAPTCHA, "unusual traffic" page or block
appears, the budget is raised step by step. When one appears, the
daemon backs off for that site, lowers the budget, and logs the event.
Every block is recorded with the load rate that preceded it, so the
real limit is measured over time.

## 0040: The app runs on the same box and raises the alerts

- Date: 2026-10-06
- Refines [0019](#0019-deals-are-shown-in-the-app-and-as-desktop-notifications)
  and [0037](#0037-the-daemon-runs-on-an-always-on-nixos-box-set-up-by-a-home-manager-module).

The always-on box has a desktop session, and the gpui app runs there.
It opens the daemon's SQLite file directly, as in
[0017](#0017-a-daemon-collects-prices-the-gpui-app-shows-them) and
[0020](#0020-observations-are-stored-in-sqlite-through-sqlx). There is
no network API.

The daemon records each confirmed deal in the database. The app watches
for new deals and shows a desktop notification for each one. Deals
found while the app is closed are marked unread and shown when it next
opens. The daemon never sends notifications itself.

## 0041: Google Flights URLs are generated; Jev handles the rest

- Date: 2026-10-06
- Superseded in part by [0059](#0059-jev-is-removed): Jev is no longer used.
- Refines [0014](#0014-jev-learns-a-search-once-then-the-url-is-replayed).
- Research: [docs/research/google-flights.md](docs/research/google-flights.md).

Google Flights search URLs are built in code by encoding the `tfs`
protobuf: legs, dates, origin and destination, cabin (1 economy,
2 premium economy, 3 business, 4 first), and one entry per adult.
Explore URLs use `q=` text, with "business class" for business. `gl`
and `curr` set the point of sale and currency. No navigation is needed
to reach a results page.

City codes are stored with their Freebase ID (SAO is `/m/022pfm`). The
ID is learned once, with a `q=` search, when an origin or destination
is added in the app. Single airports use their IATA code.

Jev drives the browser where no URL scheme exists: airline sites and
miles programs. It is also the fallback when a generated Google Flights
URL stops producing the expected page.

Explore results are about one week long (6 to 9 days) with a single
date pair per destination. They are hints for the scheduler
([0028](#0028-the-scheduler-runs-checks-by-a-greedy-value-index)), not
observations in the 7-day series.

## 0042: Both legs are tracked per day; round trips where they differ

- Date: 2026-10-06
- Refines [0007](#0007-observations-are-round-trips-of-fixed-lengths),
  [0009](#0009-seven-day-trips-departing-up-to-six-months-ahead) and
  [0013](#0013-departure-dates-come-from-googles-date-grid).
- Research: [docs/research/google-flights.md](docs/research/google-flights.md#round-trip-vs-two-one-ways).

Measured on the same dates, a domestic round trip (SSA) cost exactly
its two one-way legs. International round trips cost 6–21% less than
their legs: BUE 10%, LIS 6% (21% nonstop), MIA 15%.

Every route therefore tracks:

- **Both legs, one way, per day:** the outbound (origin → destination)
  and the return (destination → origin), each read from the one-way
  price graph. About 14 calls cover six months for one route × cabin ×
  point of sale. Any trip length can be priced from these.
- **The round trip at 7 days**, on routes where it differs from the sum
  of its legs. It is read from the round-trip price graph (about 7
  calls). Each route learns its round-trip discount from pairs of
  observations on the same dates.

When a route's round trip has matched the sum of its legs for a
configured number of checks (as on domestic routes), round-trip
tracking stops for it. It resumes if a confirmation shows a gap.

The price of a trip of length *n* departing on day *d* is estimated as
`(outbound[d] + return[d + n]) × (1 − learned discount)`. An estimated
deal is a hint for the scheduler
([0028](#0028-the-scheduler-runs-checks-by-a-greedy-value-index)). It
becomes an alert only after a real round-trip search for those exact
dates confirms it ([0031](#0031-a-deal-is-confirmed-before-the-alert)).

Baselines ([0030](#0030-deal-baselines-are-per-days-to-departure-bucket))
are kept per leg and per round trip, by days-to-departure bucket.

## 0043: Trips of 5 to 10 days

- Date: 2026-10-06
- Replaces the fixed length of [0009](#0009-seven-day-trips-departing-up-to-six-months-ahead).

A trip is any outbound and return pair with 5 to 10 days between them,
departing within the next six months. The range is a global setting in
the app. A deal is reported for a specific date pair, and the app shows
the departure × return grid around it, so a nearby pair can be chosen.

## 0044: The app's main screen is a map with a list

- Date: 2026-10-06
- Design: "G · Map + list" on the layouts canvas.

The main screen has a list of deals and routes on the left (filters for
cabin and sort order) and a night map on the right. The map draws an
arc from the origin to every watched destination, with deals
highlighted. Selecting a row highlights its arc and opens a detail card
over the map: price, dates, carrier, the cheapest seller per point of
sale as effective BRL, and the airline's own price. Other screens
(routes overview, route detail, activity, settings) follow the same
look.

## 0045: Crates are split by responsibility, with the `tm-` prefix

- Date: 2026-10-06

```
crates/
  tm-domain/     shared types, no I/O (see 0048)
  tm-jev/        Jev client (copied from tau-jev)
  tm-browser/    Chromium launch, CDP, page observation and Jev policy
  tm-gflights/   tfs encoder, Google Flights URLs, RPC parsers
  tm-store/      sqlx, migrations, configuration and observations
  tm-scheduler/  scores, change rates, baselines, deal rules (no I/O)
  tm-daemon/     binary `tmd`: the loop, budget, confirmations
  tm-app/        binary `tm`: the gpui interface
nix/
  hm-module.nix  the home-manager module
```

Domain logic that needs no browser or database (`tm-gflights`,
`tm-scheduler`) stays pure, so it is tested without either.

## 0046: The app asks the daemon for work through a requests table

- Date: 2026-10-06
- Refines [0017](#0017-a-daemon-collects-prices-the-gpui-app-shows-them).

"Check now" and similar actions insert a row in a `requests` table. The
daemon polls the database (it already polls configuration,
[0035](#0035-origins-and-destinations-are-configured-in-the-app)),
runs the request ahead of the schedule, and marks it done. There is no
socket. Requests survive restarts of either program.

## 0047: The first implementation covers every decision so far

- Date: 2026-10-06

The first implementation is not a thin slice. It covers all accepted
decisions: Google Flights legs, round trips and Explore, the
scheduler, deals and confirmations, multiple points of sale, Jev
recovery, airline and miles sources as integrations behind a common
interface, the gpui app and the home-manager module. Airline and miles
sites whose pages have not been studied yet get the interface and the
Jev-driven navigation, with their extraction marked as unverified until
probed.

## 0048: Shared types live in `tm-domain`

- Date: 2026-10-06
- Refines [0045](#0045-crates-are-split-by-responsibility-with-the-tm-prefix).

Routes, cabins, points of sale, money, observations, query families and
settings are plain types in a `tm-domain` crate, with serde and no I/O.
Every other crate depends on it, so `tm-scheduler` stays I/O-free and
the store, daemon and app share one vocabulary.

## 0049: Bootstrap core families first, starting at 600 loads a day

- Date: 2026-10-06
- Refines [0028](#0028-the-scheduler-runs-checks-by-a-greedy-value-index)
  and [0039](#0039-the-page-load-budget-starts-conservative-and-ramps-up).

The first live configuration has about 2,200 query families (about 840
on Google Flights). At 300 page loads a day and about 3 loads per check,
a first pass over all of them would take more than a week. Two changes:

- **Core first.** Families that were never checked are bootstrapped in
  rank order: home point of sale, economy, both legs and the Explore
  sweep first; then the home point of sale's round trips; then premium
  economy and business at home; then the foreign points of sale. Within
  a rank, the scheduler's order is kept. Every route gets core coverage
  in about a day.
- **A higher start.** The daily budget starts at 600 page loads instead
  of 300, and ramps from there as in 0039, backing off on any block.

## 0050: Airline and miles sites are probed before they run

- Date: 2026-10-06
- Refines [0047](#0047-the-first-implementation-covers-every-decision-so-far).

Airline and miles sources stay paused until each site has been
investigated the way Google Flights was
([docs/research/google-flights.md](docs/research/google-flights.md)):
how a search is expressed in a URL, which requests carry prices, how
the page responds to a headless browser, and what blocks it. Each
site's findings go in `docs/research/sites/`. A site is enabled in the
daemon only once its extraction is built on those findings and tested
against captured responses.

## 0051: TAP is the first airline source

- Date: 2026-10-06
- Refines [0012](#0012-airline-sites-to-search-directly) and
  [0050](#0050-airline-and-miles-sites-are-probed-before-they-run).
- Research: [docs/research/sites/tap.md](docs/research/sites/tap.md).

TAP's booking site served full results and its calendar to the
daemon's headless Chromium, behind Cloudflare, with no challenge. Its
calendar request (`bfm/rest/booking/availability/calendar/`) returns a
month of per-day one-way prices, so six months of both legs costs
about two page loads per route. TAP's leg families read that calendar;
its availability search (`bfm/rest/booking/availability/search`)
confirms a date and gives every cabin. Calendar days carry the date
they were cached; stale days are not recorded as fresh observations.
`airline:TP` is the first verified site.

## 0052: Sites that deny the daemon's browser stay off

- Date: 2026-10-06
- Refines [0012](#0012-airline-sites-to-search-directly),
  [0023](#0023-award-prices-are-tracked-in-smiles-latam-pass-and-azul-fidelidade)
  and [0050](#0050-airline-and-miles-sites-are-probed-before-they-run).

LATAM, GOL, United, Emirates and Qatar expose clean prices, but Akamai
denies the daemon's headless Chromium on the first page load. Azul,
Iberia, Air France/KLM, Turkish and American blocked or challenged even
a normal Chrome within a few loads. The miles programs need a login and
returned no award prices. All of them stay off. The daemon does not
disguise its browser to get past bot protection; Google Flights already
shows these airlines' fares. A site is reconsidered only if it stops
denying automated browsers or offers an API.

## 0053: How TAP is read

- Date: 2026-10-06
- Completes [0051](#0051-tap-is-the-first-airline-source).

- **Freshness:** a calendar day counts as an observation only if TAP
  cached it within the last 48 hours. Older days are dropped.
- **Cabins:** economy legs read the month calendar (one page load for
  the deep link plus one per month, about 8 for six months). Premium
  economy and business have no calendar: they price one date per check
  (the cheapest date Google Flights showed) from the availability
  search, for one page load.
- **One adult only:** the calendar prices one adult and cannot be asked
  for a party, so TAP families are skipped when the passenger setting
  is not one adult. They are not multiplied.
- **One way only:** TAP's round-trip families are skipped; round trips
  and deal confirmation stay on Google Flights, whose sellers include
  TAP.

## 0054: Aracaju and the Antarctic gateways join the watchlist

- Date: 2026-10-06
- Adds to [0006](#0006-a-pinned-watchlist-plus-an-explore-sweep) through
  [0035](#0035-origins-and-destinations-are-configured-in-the-app).

Three destinations are added, in the live database and in the
first-run seed:

- **AJU**, Aracaju (Brazil).
- **USH**, Ushuaia (Argentina), and **PUQ**, Punta Arenas (Chile): the
  two South American gateways where Antarctic cruises and flights leave
  from.

Puerto Williams and the other continents' gateways (Cape Town,
Christchurch, Hobart) were considered and left out.

## 0055: South America's main airports join the watchlist

- Date: 2026-10-06
- Adds to [0054](#0054-aracaju-and-the-antarctic-gateways-join-the-watchlist).

The busiest South American airports not yet watched are added, in the
live database and the first-run seed: CNF Belo Horizonte, CWB Curitiba,
BEL Belém, MAO Manaus, GYN Goiânia, VIX Vitória, CGB Cuiabá (Brazil);
MDE Medellín, CTG Cartagena, CLO Cali (Colombia); UIO Quito, GYE
Guayaquil (Ecuador); CCS Caracas (Venezuela); ASU Asunción (Paraguay);
VVI Santa Cruz, LPB La Paz (Bolivia); CUZ Cusco (Peru); COR Córdoba,
MDZ Mendoza (Argentina). The watchlist grows from 33 to 52
destinations.

## 0056: Core families are checked every day

- Date: 2026-10-06
- Refines [0029](#0029-a-daily-budget-with-a-weekly-maximum-interval),
  [0039](#0039-the-page-load-budget-starts-conservative-and-ramps-up)
  and [0049](#0049-bootstrap-core-families-first-starting-at-600-loads-a-day).

Every watched destination must be covered at least once a day. "Core"
families (bootstrap rank 0 of 0049: the home point of sale's economy
legs in both directions, and the Explore sweeps) get a maximum interval
of 24 hours instead of a week, so the scheduler checks each of them at
least daily. Premium cabins, round trips and foreign points of sale
keep the weekly maximum.

At about 8 page loads per leg check, 52 destinations need about 850
loads a day for the core alone, so the daily budget starts at 1,200 and
may ramp to 3,000. Blocks still halve it and cool the site down (0039).

## 0057: The app updates as the daemon writes

- Date: 2026-10-06
- Refines [0040](#0040-the-app-runs-on-the-same-box-and-raises-the-alerts)
  and [0044](#0044-the-apps-main-screen-is-a-map-with-a-list).

The app no longer re-reads everything on a fixed timer. It watches
SQLite's `PRAGMA data_version` on a connection of its own (it changes
whenever another connection commits) a few times a second, and rebuilds
its snapshot only when the daemon has written something, debounced so a
burst of writes is one refresh. A slow periodic refresh stays as a
fallback.

The daemon writes its heartbeat when a check starts and when it ends,
so the app shows what is being checked right now (route, cabin, leg,
point of sale, elapsed time). A one-second clock re-renders relative
times and countdowns without touching the database. Rows that just
arrived (activity, deals, observations on the open route) are briefly
highlighted.

## 0058: The daily budget doubles to 2,400 page loads

- Date: 2026-10-06
- Refines [0056](#0056-core-families-are-checked-every-day).

The daily budget starts at 2,400 page loads instead of 1,200, and the
ramp may reach 6,000 instead of 3,000. After the first day at 1,200,
with about 1,100 loads and no block from Google, the user asked to
double the rate. The core still needs about 850 loads a day; the rest
lets premium cabins, round trips and foreign points of sale come round
more often than weekly. Blocks still halve the budget and cool the
site down (0039).

## 0059: Jev is removed

- Date: 2026-10-06
- Supersedes the Jev parts of [0004](#0004-browsing-is-driven-by-jev-over-chrome-cdp),
  [0014](#0014-jev-learns-a-search-once-then-the-url-is-replayed),
  [0015](#0015-our-own-jev-client-copied-from-tau-agent),
  [0016](#0016-the-browser-runtime-is-ported-from-vs1-browser),
  [0037](#0037-the-daemon-runs-on-an-always-on-nixos-box-set-up-by-a-home-manager-module)
  and [0041](#0041-google-flights-urls-are-generated-jev-handles-the-rest).

A check either works or fails. When a generated Google Flights or TAP
page does not produce what is expected, the check ends as
`Unexpected` and the scheduler retries it later; nothing tries to
navigate around the problem. Jev's only other use, driving airline and
miles sites, had no site left to run on after
[0052](#0052-sites-that-deny-the-daemons-browser-stay-off).

So Jev goes entirely: the `tm-jev` crate, `tm-browser`'s Jev navigation
agent (and anything only it used), the daemon's Jev recovery and
Jev-driven airline and miles sources, the `--jev-key-file` and
`--jev-model` flags, and the home-manager module's `jev.*` options.
Airline sources other than TAP and the miles programs stay configured
but have no reader; they are skipped.

## 0060: Each website has its own daily budget

- Date: 2026-10-06
- Refines [0039](#0039-the-page-load-budget-starts-conservative-and-ramps-up),
  [0056](#0056-core-families-are-checked-every-day) and
  [0058](#0058-the-daily-budget-doubles-to-2400-page-loads).

A shared total makes no sense across sites: each site rate-limits on
its own, so spending on one should not starve another. Every site
(`Source::site()`: `google_flights`, `airline:TP`, …) has its own daily
budget, its own count of loads today, its own +10%-a-day ramp and its
own halving and cooldown on a block.

Starting values: Google Flights (searches and Explore) 2,400, ramping
to 6,000; TAP 300, ramping to 1,000 (about 30 leg families at about 8
loads each need about 240 a day); any other site 300, ramping to 1,000,
until set otherwise.

## 0061: Deals can be sorted by price per kilometre

- Date: 2026-10-06
- Refines [0044](#0044-the-apps-main-screen-is-a-map-with-a-list).

The deals list gains a "Lowest price per km" sort next to "Lowest
price". A row's figure is its best trip's price divided by the
round-trip great-circle distance (origin to destination and back,
haversine on the mean Earth radius, from the places' coordinates), so a
cheap trip that goes far ranks above an equally cheap short hop. Actual
routings with stops are longer; the figure compares destinations, not
itineraries. While that sort is active each row shows its value
(`R$0.25/km`).

## 0063: Sixty-two leisure and long-haul destinations join the watchlist

- Date: 2026-10-06
- Adds to [0055](#0055-south-americas-main-airports-join-the-watchlist).

The watchlist grows from 52 to 114 destinations, in the live database
and the first-run seed:

- **Brazil leisure:** FEN, BPS, JPA, SLZ, JJD, IOS, NVT, CGR, BYO, STM,
  PMW, THE.
- **South America and the Caribbean:** FTE, PDP, GPS, IPC, CJC, UYU,
  AQP, PMC, SMR, ADZ, PTY, SJO, AUA, CUR, HAV, MEX.
- **North America:** LAX, SFO, LAS, BOS, ORD, WAS (city), ATL, DFW, IAH,
  FLL, YYZ, YUL.
- **Europe, Africa, Asia and Oceania:** MIL (city), FRA, MUC, AMS, ZRH,
  VIE, PRG, ATH, IST, DUB, BER, CPT, JNB, RAK, DOH, SEL (city), BKK,
  SIN, DPS, HKG, SYD, AKL.

The daily core (0056) becomes about 228 leg families, roughly 1,830
Google Flights page loads a day, within that site's 2,400 (0060).

## 0064: A login counts only if the profile holds a session

- Date: 2026-10-06
- Refines [0024](#0024-miles-programs-are-logged-in-by-hand-once).

The app's login window and the daemon's Chromium start with the same
profile flags (`--password-store=basic`, `--use-mock-keychain`,
`--disable-sync`, shared from `tm-browser`), so cookies written while
logging in stay readable by the daemon. A LATAM Pass login was lost
because the window did not use them.

Closing the login window no longer marks the program logged in. When
the daemon handles `CloseLogin`, it reads the profile's cookie jar
(read-only, after the window has exited) and records the program as
logged in only if a persistent, unexpired cookie exists on the
program's domains that is not an analytics, consent or bot-protection
cookie (Akamai's `_abck` and `_ga` are set on any visit). The app runs
the same check and says whether a session was found. Limits: a
leftover preference cookie reads as logged in; a session ended
server-side still counts until its cookie expires; a login held only in
session cookies or local storage reads as logged out.

## 0065: Fill in and refresh the oldest data first; Google Flights doubles again

- Date: 2026-10-06
- Refines [0028](#0028-the-scheduler-runs-checks-by-a-greedy-value-index),
  [0049](#0049-bootstrap-core-families-first-starting-at-600-loads-a-day)
  and [0060](#0060-each-website-has-its-own-daily-budget).

- **New data first, oldest first.** Never-checked families still go
  first in their bootstrap rank (core first, 0049); within a rank, the
  families of routes added earlier are filled in before those of routes
  added later (the route id stands for "when added").
- **Then the stalest data.** After that, the scored slots go to the
  family whose data is oldest; the value index of 0028 only breaks
  ties. The index stays available as an option (`Order::Value`) and its
  simulation still runs against it. Overdue families (past their
  maximum interval) and the ~10% random checks are unchanged.
- **Google Flights doubles again:** its daily budget starts at 4,800
  page loads and may ramp to 12,000 (was 2,400 and 6,000).

## 0062: A browser profile per site, and sites checked in parallel

- Date: 2026-10-06
- Refines [0021](#0021-the-daemon-runs-its-own-headless-chromium),
  [0024](#0024-miles-programs-are-logged-in-by-hand-once) and
  [0060](#0060-each-website-has-its-own-daily-budget).

The daemon ran every site through one Chromium profile, so anything
else that needed the profile (a miles login, a probe) had to stop the
daemon for a minute. Instead:

- **One profile per site.** Each site (`Source::site()`: Google
  Flights, TAP, each miles program) gets its own Chromium profile
  directory and its own browser process, started when the site has work
  and closed when idle. A login opens only that site's profile, so only
  that site pauses; every other site keeps running.
- **Sites in parallel.** Each site runs at most one check at a time,
  with its own pacing and its own budget (0060), and different sites run
  at the same time. One site's pace and blocks never slow another.
- **Points of sale** stay isolated as browser contexts inside the
  site's process (0033).

Logins made before this change live in the old shared profile; they
are not migrated.

## 0066: Iceland, Greenland and Alaska join the watchlist

- Date: 2026-10-06
- Adds to [0063](#0063-sixty-two-leisure-and-long-haul-destinations-join-the-watchlist).

Three destinations join the first-run seed, the biggest airport of each
place: Keflavík (KEF) for Iceland, Nuuk (GOH) for Greenland, whose new
runway took over the international flights from Kangerlussuaq in 2024,
and Anchorage (ANC) for Alaska. The watchlist grows from 114 to 117.
A database seeded before this gets them from the app's settings.

## 0067: "No flights" is an outcome of its own

- Date: 2026-10-06
- Refines [0059](#0059-jev-is-removed).

When a source loads correctly and says there is nothing to sell
(Google Flights' "No results returned" / "No options matching your
search" / "No business class flights found"), the check ends as
`QueryOutcome::NoFlights`, not `Unexpected`. It costs its page load, is
shown quietly in Activity (not as an error), and that family waits a
week before the next check: a family whose last check found no flights
gets a minimum interval of 7 days and loses the daily core cadence of
0056. A family that finds flights again goes back to normal.

## 0068: Nuuk leaves the watchlist

- Date: 2026-10-06
- Removes GOH from [0066](#0066-iceland-greenland-and-alaska-join-the-watchlist).

Google Flights has no results at all for São Paulo ↔ Nuuk, in any
cabin or point of sale: no single ticket to Greenland is sold through
it. GOH is deactivated in the live database (its history is kept, as
for any removed route) and dropped from the first-run seed. Reykjavík
and Anchorage stay.

## 0069: How "no flights" is recognised

- Date: 2026-10-06
- Completes [0067](#0067-no-flights-is-an-outcome-of-its-own).

A Google Flights page counts as "no flights" when it says "No options
matching your search" or "No … flights found" (for example "No
Business Class flights found"). "No results returned." alone does not:
Google also shows it above its "Oops, something went wrong" error page,
and any "something went wrong" text means the check failed, not that
there is nothing to sell. A family's latest *loaded* check decides its
7-day wait: a later block or error does not end it, a check with prices
does.

## 0070: New families are filled in one route at a time, in turn

- Date: 2026-10-06
- Refines [0065](#0065-fill-in-and-refresh-the-oldest-data-first-google-flights-doubles-again).

Within a bootstrap rank, never-checked families are taken one per
route per pass instead of a whole route at a time: the first pass
takes one family from each route (earliest-added routes first), the
second pass the next family of each, and so on. Every destination gets
a first price sooner, and "older first" still holds within each pass.
"Check now" is unchanged: it still checks every family of the route,
ahead of the schedule (0046).

## 0071: "Check now" shares the site with the schedule

- Date: 2026-10-06
- Refines [0046](#0046-the-app-asks-the-daemon-for-work-through-a-requests-table).

"Check now" still queues every family of the route, but manual checks
no longer all run ahead of the schedule. Each site's worker runs one
manual check, then one scheduled pick, and so on; when the scheduler has
nothing due, manual checks run one after another, paced like any other
check. When several requests are pending, the next manual check comes
from a different request than the last, so several clicks take turns.
A single click on a big route (36 families) no longer holds Google
Flights for twenty minutes.

## 0072: Each site runs three checks at once

- Date: 2026-10-07
- Refines [0062](#0062-a-browser-profile-per-site-and-sites-checked-in-parallel)
  and [0039](#0039-the-page-load-budget-starts-conservative-and-ramps-up).

A test on the live profile ran one, two and three Google Flights checks
side by side: each still took about 15 s (8 page loads), with no blocks
and no errors. The time goes to waiting on Google's pages, not to the
machine, so each site's worker now runs up to three checks at once,
each in its own tab of the site's Chromium. TAP gets the same.

- **Lanes.** A worker has three lanes. A check starts on a free one:
  the worker writes its query row and its fetch runs on a task of its
  own; when it lands, the worker records it as before. Starting and
  recording stay on the worker, one at a time, so two checks never
  write the same rows at once; only the browser work overlaps.
- **Pacing is per lane.** After its check, a lane rests the usual
  random 4–15 s (0039) before taking the next, so the lanes drift apart
  and a site makes about three times as many checks an hour. The daily
  budget still caps the total: at 4,800 loads Google Flights spends its
  day sooner, not more.
- **Queues.** A tick's picks wait in a queue and take turns with manual
  checks as before (0071). A new tick is planned once the last one's
  picks have all started, without the families still in flight; the
  loads of up to three checks in flight are not yet counted, so a tick
  can overshoot the budget by that much.
- **What stays one at a time.** Deal confirmations and Freebase lookups
  run on the worker itself, beside the lanes. A block ends the tick's
  picks; the checks already in flight still land. A login pause and a
  shutdown wait for the checks in flight before closing the browser.
- **Status.** A site's running checks move from `site_status` into a
  `site_checks` table (migration 0005), and the app shows each one.

`tmd --parallel <n>` changes the number of lanes (1 gives the old
behaviour).
