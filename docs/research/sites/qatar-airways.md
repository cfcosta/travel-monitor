# Qatar Airways (qatarairways.com, QR): GET search URL, JSON BFF, Akamai blocks headless

- Date: 2026-10-06
- Method: `chrome-cdp` against the user's Chrome (one tab, an XHR/fetch
  hook installed before navigating, recording request headers) and a
  headless Chromium 151 from the dev shell (`--headless=new`, fresh
  profile, no user-agent override, the same flags as the daemon).
  Brazil site (`/pt-br/`). Not logged in. Tests: GRU→DOH round trip
  2026-11-10 to 2026-11-17 (form), one way 2026-11-12 in economy and
  business (generated URL), and the "Ver outras datas" calendar.
- Load count: 4 page loads in the user's Chrome, plus 1 in-page click
  ("Ver outras datas") and 5 in-page `fetch` calls to the calendar
  endpoint. 1 page load in headless (blocked).

## Summary

- The results page has a **plain GET URL**
  (`/app/booking/flight-selection?…`). A generated one opened directly.
- Prices come from two **JSON POSTs** to a BFF under
  `/dapi/public/bff/web/flight-search/`, with the whole search in the
  body:
  - `fare-calendars`: the cheapest fare per day, 7 or 15 days.
  - `flight-offers`: the flight list with every fare family in economy
    and business.
- In-page replays of `fare-calendars` with a different date returned
  200, so a 15-day window can be moved anywhere with no page load.
- **Akamai Bot Manager.** The user's Chrome passed. The daemon's
  headless Chromium got Akamai's edge "Access Denied" on its first load.
- Feasibility: **poor** as the daemon is today (headless is blocked).
  It would be **good** in a browser that passes Akamai: one page load
  plus about 12 calendar calls covers six months of one leg in one
  cabin.

## 1. Search entry and results URL

The home page widget (`Reserve um voo`) is an Angular app inside the
shadow root of `<app-nbx-explore>`. Its autocomplete options render in
a CDK overlay in the main document. It offers Volta / Somente ida /
Múltiplas cidades, De, Para, a date range picker, passengers and class
(Econômica or Executiva), and "Reserve utilizando Avios". Submitting
the form navigates to:

```
https://www.qatarairways.com/app/booking/flight-selection?widget=QR&searchType=F
  &addTaxToFare=Y&minPurTime=0&selLang=pt&tripType=R&fromStation=GRU&toStation=DOH
  &departing=2026-11-10&returning=2026-11-17&bookingClass=E&adults=1&children=0
  &infants=0&ofw=0&teenager=0&flexibleDate=off&allowRedemption=N
```

**Generated URLs work.** Opened cold:

- `tripType=O`, no `returning`, `departing=2026-11-12`: one-way results.
- `bookingClass=B`: business. The page sent `cabinClass: "PREMIUM"` (see
  below).

Parameters: `tripType` R/O (M is multi-city), ISO dates,
`bookingClass` E or B, passenger counts, and `allowRedemption=N` for
cash (Y is an Avios search, which needs login; not tested).

## 2. Where prices come from

The flight-selection page makes two same-origin XHR POSTs as soon as it
loads.

### Request

```
POST /dapi/public/bff/web/flight-search/fare-calendars
POST /dapi/public/bff/web/flight-search/flight-offers
Content-Type: application/json
Accept-Language: pt
qr-lang: pt
session-id: <UUID, sessionStorage "nbx.session.id">
X-AssignedDeviceID: <32 chars, localStorage "nbx.device.id">
nbx_fs_api_key: <static key embedded in the app>
Request-Id / traceparent: Azure Application Insights tracing
```

Body (`flight-offers`; `fare-calendars` adds `flexibility`):

```json
{"channel":"WEB_DESKTOP",
 "itineraries":[{"origin":"GRU","destination":"DOH","departureDate":"2026-11-10","isRequested":true},
                {"origin":"DOH","destination":"GRU","departureDate":"2026-11-17","isRequested":false}],
 "cabinClass":"ECONOMY","ignoreInvalidPromoCode":true,
 "passengers":[{"type":"ADT","count":1}]}
```

`cabinClass` values: `ECONOMY`; `PREMIUM`, which is **business**
(what `bookingClass=B` sends); and `FIRST`, which was accepted but had
no prices on GRU–DOH. `BUSINESS` is rejected with 400
`{"code":"0000002","error":"Messaging error."}`.

### `flight-offers`

Fixture: `fixtures/qatar-airways/flight-offers-ow-GRU-DOH-2026-11-12.{request,response}.json`.

```
flightOffers[]: {
  origin.iataCode, destination.iataCode, duration (s), numberOfStops,
  segments[]: { flightNumber: "QR774", vehicle.code: "351",
                departure: { origin.iataCode, dateTime: "2026-11-12T02:10:00" },
                arrival: {...}, operatingAirlineName, qSuiteEquipped },
  fareOffers[]: { cabinType: "ECONOMY"|"BUSINESS",
                  fareFamilyCode: "ECONOMY_CLASSIC"|"ECONOMY_CONVENIENCE"|"ECONOMY_COMFORT"|
                                  "BUSINESS_COMFORT"|"BUSINESS_ELITE",
                  price: { base: 3993.70, total: 4700.74, currencyCode: "BRL" },
                  availableSeats, totalYqYrTax, isLowestFare, boundId },
  tags: ["LOWEST_FARE","QSUITE","STARLINK"] }
currencies[], usdExchangeRate, viaStations, dictionaries.localization
```

- **One economy search returns both cabins.** QR774 one way 2026-11-12:
  Classic R$4,700.74, Convenience R$5,403.75, Comfort R$11,172.44,
  Business Comfort R$34,606.18, Business Elite R$40,679.00.
- Amounts are decimals in BRL.
- **Round trip:** the outbound list (and its calendar) shows a per-bound
  price. The GRU→DOH outbound in the Nov 10–17 round trip was
  R$3,329.62, against R$4,700.74 one way. The trip total appears only
  after an outbound fare is chosen, which was not done.

### `fare-calendars`

Fixtures: `fixtures/qatar-airways/fare-calendars-*.{request,response}.json`
(one way economy, one way business, round trip).

```json
{"faresFromCalendar":[{"date":"2026-11-09","lowestPrice":4700.74,"currencyCode":"BRL"}, …],
 "moreDates":[], "requestStationDetails":{"GRU":{"countryCode":"BR"},"DOH":{"countryCode":"QA"}}}
```

- On load, the page asks with `flexibility: 3`, which returns 7 days.
- **"Ver outras datas"** asks with `flexibility: 7`,
  `includeCurrencies: true`, `includeMoreDates: false`, which returns
  15 days. `flexibility: 30` gave 400.
- Days with no fare come back as just `{ "date": … }`, as in `FIRST`
  on this route.
- No flight numbers. It is the cheapest total per departure day.

## 3. Calendars

`fare-calendars` with `flexibility: 7` gives 15 days per call. In the
user's Chrome, replaying the page's own request from inside the page
(the same headers, `departureDate` changed to 2027-02-15) returned 200
with Feb 8–22 2027. Six months of one leg in one cabin is therefore
about 12–13 calls after one results page load. Unlike Emirates, the
route is in the body, not the session, so the same page could ask for
both legs. That was not tested.

For round trips the calendar is per outbound date at a fixed return
date, giving bound prices, not a trip-length grid.

## 4. One way, round trip, cabins, points of sale

- One way, round trip and multi-city via `tripType` O, R, M.
- Cabins: economy and business (Qsuite). There is no premium economy;
  the API's `PREMIUM` means business. First is accepted, but GRU–DOH
  has none.
- **Point of sale:** the API body has no country or currency. The site
  locale (`/pt-br/`, cookie `country`, localStorage `d_country`) sets
  the market, and prices came in BRL. `currencies[]` in `flight-offers`
  lists display currencies for "Alterar a moeda" (all `local: false`).
  Foreign markets were not tested.
- Avios: `allowRedemption=Y` and the "Reserve utilizando Avios" checkbox
  need a Privilege Club login. The user's Chrome was not logged in (the
  page's `my-profile/validate` call returned 400). Not tested.

## 5. Bot protection

- **Akamai Bot Manager**: cookies `_abck`, `bm_sz`, `bm_sv`, and sensor
  POSTs to an obfuscated same-origin path (`/7L8WfAGJvi0LKclQQq4k/…`).
  The home page also loads **reCAPTCHA Enterprise**
  (`recaptcha.net/recaptcha/enterprise`), tied to the newsletter form
  (`g-recaptcha-response`). It did not appear in the search flow.
- **User's Chrome:** the home page, the form submit, two generated URLs,
  the calendar click and the in-page replays all worked. No challenge.
- **Headless Chromium (daemon setup):** the first load, the one-way URL
  opened directly, returned an Akamai edge block page: title "Access
  Denied", "You don't have permission to access … on this server.
  Reference #18.96601702.…", with a link to `errors.edgesuite.net`.
  Per the rules, we stopped there. We did not retry, warm up, or
  override the user agent (`HeadlessChrome/151`).

## 6. Recommendation for the daemon

**Feasibility: poor today. Good if the browser passes Akamai.**

Plan, once a non-blocked browser exists:

1. Load the generated one-way URL (`tripType=O`, cabin `E` or `B`).
   Capture `url_contains = "/flight-search/fare-calendars"` and
   `"/flight-search/flight-offers"`. That is 7 days of calendar plus the
   day's flights for both cabins.
2. Move the calendar with in-page replays of the page's own
   `fare-calendars` request (its headers come from the captured request
   or from `nbx.session.id` / `nbx.device.id` in storage), using
   `flexibility: 7` and `departureDate` stepped by 15 days: about 12
   calls per leg and cabin for six months. As with Emirates, these are
   page-context requests with new parameters, not the page's own calls.
   That needs a decision against 0032.
3. Without in-page calls: about 26 page loads per leg and cabin
   (7 days each), or Jev clicking "Ver outras datas" and its date
   navigation (not explored).

Risks: Akamai blocks headless outright. The API key and the session
and device IDs are app-generated and could change shape. `PREMIUM`
meaning business is a trap for a parser. Jev is not needed for search:
the URL is fully generated.

## Open questions

- Does Akamai let the daemon through when headful, or with a warmed
  profile? Not tested, so as not to evade the block.
- Whether one results page can ask `fare-calendars` for the reverse leg
  (DOH→GRU) and for business (`PREMIUM`) without a new page load.
- How many in-page calendar calls one session tolerates before Akamai
  scores it as a bot.
- Foreign points of sale: whether switching the site locale (for
  example `/en-us/`) reprices, or only converts.
- How far ahead the calendar answers (Feb 2027 worked; the 355-day edge
  was not probed).
- Avios award search (needs login; the user's Chrome was logged out).

## Fixtures (`fixtures/qatar-airways/`)

- `flight-offers-ow-GRU-DOH-2026-11-12.{request,response}.json`: the
  one-way flight list, economy search (economy and business offers).
- `fare-calendars-ow-GRU-DOH-2026-11-12.{request,response}.json`: the
  7-day calendar, economy.
- `fare-calendars-ow-business-GRU-DOH-2026-11-12.{request,response}.json`:
  the same with `cabinClass: "PREMIUM"` (business).
- `fare-calendars-rt-GRU-DOH-2026-11-10_17.{request,response}.json`:
  the round-trip outbound calendar.

Request headers (`session-id`, `X-AssignedDeviceID`, `nbx_fs_api_key`)
and cookies are not stored.
