# Iberia (iberia.com, IB): deep links, the sse-avm API and Akamai

- Date: 2026-10-06
- Site: `www.iberia.com/br/` (home and search form) and the booking app
  at `www.iberia.com/flights/` (Angular), backed by
  `ibisservices.iberia.com/api/...`.
- Method: `chrome-cdp` against the user's Chrome (not logged in to
  Iberia Club), one tab with an XHR/fetch hook installed before the
  first navigation, 2 page loads. Then the daemon's own Chromium 151
  (`--headless=new`, default `HeadlessChrome` user agent, fresh profile,
  as `tm-browser` launches it), 1 page load.
- Test search: GRU→MAD one-way 2026-11-10, economy, 1 adult, Brazil
  market.
- Result: **prices captured in the user's Chrome; the headless browser
  got a 403 block page on its first load.** We stopped there, as agreed.
- Fixtures: [fixtures/iberia/](fixtures/iberia/README.md).

## Search URLs

The home page form (`#tab1form`: origin, destination, trip type, dates,
passengers, `tarifa` cabin select) posts into the booking app. The app
reads its search from URL parameters, so a results page can be opened
directly:

```
https://www.iberia.com/flights/?market=BR&language=pt&appliesOMB=false&splitEndCity=false&initializedOMB=true&flexible=true&TRIP_TYPE=1&BEGIN_CITY_01=GRU&END_CITY_01=MAD&BEGIN_DAY_01=10&BEGIN_MONTH_01=202611&BEGIN_YEAR_01=2026&FARE_TYPE=R&ADT=1&CHD=0&INF=0&BNN=0&YTH=0&YCD=0#!/availability
```

This URL loaded the GRU→MAD results in the user's Chrome. Parameter
names come from the booking app's bundle (`web-booking-main-26-17-0.js`,
`BOOKING_PARAMS_NAMES`):

- `market`, `language`: point of sale and language (`BR`, `pt`).
- `TRIP_TYPE`: `1` one-way (tested). `2` round trip (from the bundle,
  not tested), with `END_DAY_01`, `END_MONTH_01` (`yyyyMM`),
  `END_YEAR_01` for the return date.
- `BEGIN_CITY_01`, `END_CITY_01`: IATA airport or city codes.
- `BEGIN_DAY_01` (`dd`), `BEGIN_MONTH_01` (`yyyyMM`), `BEGIN_YEAR_01`.
- `FARE_TYPE`: the preferred cabin, the same values as the home form's
  `tarifa` select: `R` economy ("Mais económica", tested), `N` premium
  economy, `B` business (not tested). The app stores it as
  `preferredCabin` and sends `preferredCabin: "ECONOMY"` to the API.
- `ADT`, `CHD`, `INF`, `YTH`, `YCD`, `BNN`: passenger counts.
- `flexible=true`: the app stores `flexDates: "true"`; the page showed the
  normal results with the 7-day ribbon.
- The bundle also knows `FARE_TYPE_END`, `Cabin`, `FlightNumber`,
  `BookingCode`, `ReferralId`, `DisplayedPrice` (metasearch deep links).

## Where prices come from

The app gets an anonymous OpenID token from
`POST https://ibisauth.iberia.com/api/auth/realms/commercial_platform/protocol/openid-connect/token`
(stored in `sessionStorage.anonymousToken`), then calls
`ibisservices.iberia.com`.

### Availability: `POST https://ibisservices.iberia.com/api/sse-avm/rs/v2/availability/itinerary`

Request:

```json
{"slices":[{"origin":"GRU","destination":"MAD","date":"2026-11-10"}],
 "passengers":[{"passengerType":"ADULT","count":1}],
 "marketCode":"BR","preferredCabin":"ECONOMY"}
```

Response (about 230 KB):

- `originDestinations[0].slices[]`: the itineraries. Each has `sliceId`,
  `departureDateTime` / `arrivalDateTime` (`"2026-11-10 20:30"`, local),
  `stopsNumber`, `duration`, and `segments[]` with
  `flight.marketingCarrier.code` + `flight.marketingFlightNumber`
  (`IB` `272`), `flight.operationalCarrier`, `departure.code`,
  `arrival.code`, `aircraft`.
- `offers[]`: one per itinerary × fare family.
  - `price`: `{fare, tax, total, currency}` (`total` 3694.95, `"BRL"`).
  - `originDestinations[0].applicableSlices[0].sliceId` links the offer
    to an itinerary; `segments[].fareFamily` (`BASLH`, `OPTLH`, `COLH`,
    `FLEXLH` economy; `PRECOOPT`, `PRECOCONFT`, `TPFLRS` premium economy;
    `BUSOPTLH`, `BUSCOMLH`, `BLRES` business), `segments[].bookingClass`
    (`ECONOMY`, `PREMIUMTOURIST`, `BUSINESS`), `rbd`, `remainingSeats`.
  - `specialDiscountsApplied[]` and `discountsApplied.originalPrice`: see
    below.
- Even with `preferredCabin: "ECONOMY"`, the response held every cabin
  (economy, premium economy, business).

GRU→MAD on 2026-11-10: 10 itineraries (IB268 and IB272 nonstop by
Iberia, IB6808 and IB6540 nonstop operated by LATAM, and connections via
LATAM or British Airways), 35 offers. Lowest economy R$3,694.95, premium
economy R$7,752, business R$16,911.

**Dynamic discount.** The cheapest offer (IB272, Basic) carried
`specialDiscountsApplied: [{"type":"DYNAMIC_PRICING","description":"Promoção especial","percentDiscount":13.0,"totalDiscount":541.87}]`
and `discountsApplied.originalPrice.total: 4236.82`. The page showed it
as R$3,695 with R$4,237 struck through. Only that one flight had it.
This is the kind of airline-site-only price that 0011 is looking for:
the monitor should read `price.total`, not the original price.

### Calendar ribbon: `POST https://ibisservices.iberia.com/api/sse-avm/rs/v2/calendar/ribbon`

The results page has a 7-day date ribbon. Its request is the availability
request plus a window:

```json
{"slices":[{"origin":"GRU","destination":"MAD","date":"2026-11-10",
  "daysAfter":3,"daysBefore":3}],
 "passengers":[{"passengerType":"ADULT","count":1}],
 "marketCode":"BR","preferredCabin":"ECONOMY"}
```

Response: `calendarRibbon.outBoundRow.cells[]` (and `inBoundRow` for
round trips), each `{boundDate, totalPrice.total, currency, bestPrice,
available}`. Clicking the ribbon's "next" arrow fetched the next 7 days
(`date: "2026-11-16"`, again ±3), with no page load.

- The ribbon showed R$4,236.82 for every day from Nov 7 to Nov 14, the
  **undiscounted** price: the ribbon does not include the dynamic
  discount that the availability search applied to Nov 10.
- We tried one in-page call with a wider window (`daysAfter: 30`) using
  the page's anonymous token. The browser rejected it ("Failed to
  fetch"), probably because the app sends other headers we did not
  record. We did not pursue it.

No month calendar was found on the booking app. The marketing pages
`/br/voos-baratos/` and `/br/voos-baratos/meses/` (cheapest fares by
month) were not opened.

## One-way, round trip, cabins, points of sale

- One-way: `TRIP_TYPE=1`, one slice. Round trip: `TRIP_TYPE=2` with the
  return date; the API request then has two slices (from the bundle; not
  captured).
- Cabins: every cabin is in the availability response, whatever
  `FARE_TYPE` says. The ribbon's prices follow `preferredCabin`
  (economy here).
- Point of sale: `market` in the URL and `marketCode` in the API body
  (`BR` → BRL). Other markets were not tested. The site also has a
  market/language selector (`localeMarketForm`: `mercado`, `idioma`).

## Bot protection

- **Akamai Bot Manager**: cookies `_abck`, `bm_sz`, `ak_bmsc`, `bm_s`,
  `bm_so`, `bm_sv`, `bm_mi`, `bm_lso`; the sensor script at
  `/akam/13/...` on the home page and sensor posts to an obfuscated
  first-party path (`/X3kmWB/DUL/...`) on the booking page.
- **CHEQ** (ad-fraud/bot scoring): script and beacons on
  `forroundprince.com`, cookies `_cq_duid`, `_cq_suid`, `_cq_s`,
  `_cq_session`.
- Also Quantum Metric, Monetate, Dynatrace (analytics).
- **User's Chrome**: home page and the deep link both loaded; the
  results rendered after about 10 s ("Enquanto procuramos os seus
  voos...").
- **Headless Chromium** (fresh profile, default `HeadlessChrome` user
  agent, straight to the deep link): the document came back **HTTP 403**
  (navigation `responseStatus` 403, 6 KB) with Iberia's generic
  "Sorry! The connection was interrupted due to an error" page. No
  Akamai cookies (`_abck`, `bm_sz`) were set, so the block happened at
  the edge before any sensor ran, most likely on the user agent or the
  TLS/HTTP fingerprint. We stopped after this one load and did not retry
  with other settings.
- `curl` with a desktop user agent on the booking bundle's static files
  also got an HTML page instead of JavaScript (not investigated).

## Recommendation

**Feasibility: poor** with the daemon's current browser (headless,
default user agent): it is blocked on the first page load.

If the daemon's browser can load iberia.com (for example the visible,
headed profile from 0024, or a launch that does not announce
`HeadlessChrome`; deciding that is a policy question, since it is about
getting past bot detection), the integration itself is simple:

1. Generated URL (above) for each route × direction × date.
2. Capture `sse-avm/rs/v2/availability/itinerary` (all cabins, exact
   prices including dynamic discounts) and `sse-avm/rs/v2/calendar/ribbon`
   (7 days of lowest prices per call).
3. For per-day one-way prices (0042), click the ribbon's next arrow
   (`.ib-date-carousel__button-next`): 7 days per click, about 26 clicks
   for six months per leg, after 1 page load. These are undiscounted
   lowest prices for the preferred cabin; confirm low days with an
   availability search (1 page load per date).

Risks: Akamai and CHEQ scoring over many clicks; the ribbon misses
dynamic discounts; the anonymous token's lifetime was not measured.

## Open questions

- Does Iberia block a headed (visible) daemon Chromium, or a headless one
  with a normal Chrome user agent? This needs a decision before any test,
  because it is a step toward bypassing Akamai.
- Does `calendar/ribbon` accept a wide window (`daysAfter` 30 or more)
  when the page itself sends it? That would give a month per call.
- Are dynamic discounts (`DYNAMIC_PRICING`) shown anywhere other than the
  availability search, and does Google Flights see them?
- Do `TRIP_TYPE=2` and `FARE_TYPE=B` deep links behave as the bundle
  suggests?
- Do the "voos baratos / meses" pages expose a usable monthly fare feed?
- Iberia Club (Avios) award search: needs login; not in scope (0023 lists
  only Smiles, LATAM Pass and Azul Fidelidade).
