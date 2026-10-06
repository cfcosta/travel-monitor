# Air France / KLM: deep links, GraphQL offers and Akamai

- Date: 2026-10-06
- Sites: `wwws.airfrance.com.br` (AF) and `www.klm.com.br` (KL). Both run
  the same Air France–KLM web platform: same URL scheme, same `/gql/v1`
  GraphQL API, same Akamai setup.
- Method: `chrome-cdp` against the user's Chrome (not logged in to
  Flying Blue), one tab per site, XHR/fetch hook installed before the
  first navigation; then the daemon's own Chromium 151 (`--headless=new`,
  default user agent, as `tm-browser` launches it).
- Test searches: GRU→CDG round trip 2026-11-10 / 2026-11-17, economy, 1
  adult (AF, captured); GRU→CDG one-way business 2026-12-10 (AF,
  blocked); GRU→AMS one-way economy 2026-11-10 (KLM, blocked).
- Fixtures: `fixtures/air-france-klm/`.

## Search URLs

The search form lives at `/search/advanced` (also on the home page).
A results page can be opened directly with a generated URL:

```
https://wwws.airfrance.com.br/search/offers?pax=1:0:0:0:0:0:0:0&cabinClass=ECONOMY&activeConnection=0&connections=GRU:A:20261110>CDG:A-CDG:A:20261117>GRU:A&bookingFlow=LEISURE
```

- `connections`: legs separated by `-`; each leg is
  `<origin>:<type>:<yyyymmdd>><destination>:<type>`. `A` = airport,
  `C` = city (not tested). One leg = one-way. `>` should be sent as
  `%3E`.
- `cabinClass`: `ECONOMY`, `PREMIUM`, `BUSINESS` (the values the API
  uses; `PREMIUM` and `BUSINESS` from the response's
  `availableCommercialCabinsPerConnections`; only ECONOMY loaded
  successfully, the BUSINESS one-way got a 403).
- `pax`: eight counters; `1:0:0:0:0:0:0:0` is one adult (the order of
  the other slots was not verified).
- `activeConnection=0`: which leg's flight list to show first.
- `bookingFlow=LEISURE`. `REWARD` is the Flying Blue award flow (the
  form's "Reservar com Milhas" switch); not tested (needs login).
- Point of sale is the domain (`airfrance.com.br`, `klm.com.br`,
  `airfrance.fr`, `klm.com`...). It is also sent as `country=BR&
  language=pt` on the GET GraphQL calls and stored in `country` /
  `language` cookies. Currency follows the country (BRL on `.com.br`).
- The page immediately rewrites the URL to `/search/flights/0`; the
  search state moves to a server-side `searchStateUuid`.
- `/search/flights/0?connections=…` (the rewritten form) does **not**
  work as an entry point: it redirects to `/search/advanced`.

## Where prices come from

Everything is GraphQL over `POST /gql/v1?bookingFlow=LEISURE&operationName=<Op>`
with Apollo persisted queries. The body is:

```json
{
  "operationName": "…",
  "variables": { … },
  "extensions": {
    "hashcash": { "version": 2, "timestamp": "<ISO>", "hash": "<sha256 hex>-<nonce>" },
    "persistedQuery": { "version": 1, "sha256Hash": "<query hash>" }
  }
}
```

`hashcash` is a client-side proof of work computed per request by the
page's scripts. Together with Akamai's sensor cookies, it means the API
cannot be replayed from outside the page; the daemon must let the page
make the calls and read the responses.

### `SearchResultAvailableOffersQuery`: the flight list

Request `variables`:

```json
{
  "activeConnectionIndex": 0,
  "bookingFlow": "LEISURE",
  "availableOfferRequestBody": {
    "commercialCabins": ["ECONOMY"],
    "passengers": [{ "id": 1, "type": "ADT" }],
    "requestedConnections": [
      { "origin": { "code": "GRU", "type": "AIRPORT" }, "destination": { "code": "CDG", "type": "AIRPORT" }, "departureDate": "2026-11-10" },
      { "origin": { "code": "CDG", "type": "AIRPORT" }, "destination": { "code": "GRU", "type": "AIRPORT" }, "departureDate": "2026-11-17", "dateInterval": "2026-11-14/2026-11-20" }
    ],
    "bookingFlow": "LEISURE",
    "withUpsellCabins": true
  },
  "searchStateUuid": "<uuid>"
}
```

Response (`data.availableOffers`, 160 kB, 15 itineraries for the
outbound leg):

- `offerItineraries[]`
  - `activeConnection.segments[]`: `origin.code`, `destination.code`,
    `departureDateTime`, `marketingFlight.carrier.code` + `.number`
    (`AF` `0453`), plus `duration`, `isDirect`, `operatingCarriers`.
  - `upsellCabinProducts[]`: one per cabin, with
    `connections[0].cabinClass` (`ECONOMY` / `PREMIUM` / `BUSINESS`),
    `fareFamily.code` (`LIGHTLH`, `PELIGHTLH`, `BUSLIGHTLH`,
    `BUSSTANDLH`…), `price.amount` + `price.currencyCode`,
    `numberOfSeatsAvailable`, `isPromo`.
  - `flightProducts[]`: per-cabin price summary.
- `searchMetadata.availableCommercialCabinsPerConnections`: the cabins
  sold on each leg.

So one economy search also returns premium economy and business prices
for every outbound flight (`withUpsellCabins: true`). Sample, outbound
GRU→CDG Nov 10 within the round trip: AF0459 economy R$2,442.93,
premium R$4,639.23, business R$9,213.79.

**Round-trip pricing.** In round-trip mode the outbound list shows a
per-leg share of the round-trip fare, not a one-way fare (the
lowest-fare response has `displayPrice` 2,400.14 and
`displayPriceItinerary` 4,800.27 for the same day). The full round-trip
total appears only after choosing an outbound flight (a second
`SearchResultAvailableOffersQuery` with `activeConnectionIndex: 1`; not
captured). A one-way search URL gives one-way fares.

### `SharedSearchLowestFareOffersForSearchQuery`: the date strip

Request `variables.lowestFareOffersRequest`: same shape plus
`"type": "DAY"` and, per leg, `"dateInterval": "2026-11-07/2026-11-13"`
(the selected date ±3 days).

Response `data.lowestFareOffers.lowestOffers[]`, one per day:
`flightDate`, `displayPrice` (lowest per-leg price, BRL),
`displayPriceItinerary` (whole round trip), `currency`, `isPromoFare`,
`noFlight`, `connections[].price`. Seven days per call. Sample: Nov 7
R$2,400.14, Nov 11 R$2,375.63, Nov 13 R$2,849.99 (per-leg share of a
round trip; economy).

### Other calls seen

- `SharedSearchContextItineraryForSearchQuery`,
  `SharedSearchContextPassengersForSearchQuery`: store the URL's search
  in the server-side search state.
- `SharedSearchBoxReferenceDataForSearchQuery` (GET, 1.1 MB): airports
  and cities for the form.
- `AviatoSharedGetContactsQuery`: call-centre contacts.
- `/endpoint/v1/oauth/login`: login status check.

## Calendars

- The results page has only the 7-day strip
  (`SharedSearchLowestFareOffersForSearchQuery`, `type: "DAY"`).
- The `type` field suggests other granularities (AF/KLM sites have a
  monthly "lowest fares" view on some markets); `MONTH` was not
  observed and not tested, because the site blocked further loads.
- Six months of one-way per-day prices from the strip alone would need
  about 26 searches per route × cabin. That is far beyond what Akamai
  tolerated here (see below).

## Bot protection

- **Akamai Bot Manager**: `_abck`, `bm_sz`, `bm_sv` cookies, the
  `/akam/13/pixel_…` beacon and a sensor POST to an obfuscated path
  (`/A3Z4D7JP64hr8/…`).
- **Jscrambler** client integrity (`*.wpi.jscrambler.com` POST).
- **reCAPTCHA** iframe present on the search form.
- **hashcash** proof of work on every GraphQL POST.

What happened:

| Browser | Load | Result |
| --- | --- | --- |
| User's Chrome, AF | 1. home | OK |
| | 2. `/search/flights/0?connections=…` | redirected to `/search/advanced` (wrong URL form) |
| | 3. `/search/offers?…` GRU⇄CDG economy | **prices** (200) |
| | 4. `/search/offers?…` GRU→CDG one-way business | **403 Access Denied** from Akamai on both price queries; page stuck on skeleton loaders |
| User's Chrome, KLM (a few minutes later) | 1. home | OK |
| | 2. `/search/offers?…` GRU→AMS one-way economy | **403** on the search-context mutations; redirected to `/search/advanced` |
| Headless Chromium 151, AF | 1. home | `net::ERR_HTTP2_PROTOCOL_ERROR` (connection reset at the edge) |
| Headless Chromium 151, KLM | 1. home | `net::ERR_HTTP2_PROTOCOL_ERROR` |

The 403 body is Akamai's standard "Access Denied … Reference
#18.c5601702…" page (`fixtures/air-france-klm/af-akamai-403.html`). A
`curl` with the headless user agent got the same HTTP/2 stream reset,
so the headless block happens before any JavaScript runs, most likely on
the `HeadlessChrome` user agent and/or the TLS/HTTP2 fingerprint. A
control load of `wikipedia.org` in the same headless browser worked.
Per the research rules, no evasion (user-agent override, stealth flags)
was tried.

The block in the user's real Chrome after only three search loads, and
its carry-over to KLM, suggests an IP- or device-level Akamai score
shared across the AF/KLM properties.

## Recommendation

**Feasibility: poor.**

- Mechanism, if it were allowed through: generated
  `/search/offers?…` URL, then capture `url_contains:
  "operationName=SearchResultAvailableOffersQuery"` (flight list with
  every cabin) and `"operationName=SharedSearchLowestFareOffersForSearchQuery"`
  (7 days around the date). One page load per route × date; one-way
  searches give true one-way fares.
- In practice the daemon's headless Chromium is rejected at the first
  request, and even a real, warm Chrome was blocked on its third
  search. A per-day six-month series is out of reach.
- At most this could be an occasional **confirmation** source (one
  search when Google Flights shows an AF/KL deal), and only if a headed
  or non-flagged browser is allowed through, which is an evasion
  question the project has not decided.
- Google Flights already shows AF/KL's own fares (AF sells on Google
  Flights); prefer it.

## Open questions

- Is the headless block only the `HeadlessChrome` user-agent string?
  Testing that is a user-agent override, i.e. a policy decision.
- How long does the Akamai 403 on this IP last, and is it shared with
  other Akamai-protected airlines (Turkish also uses Akamai)?
- Does `SharedSearchLowestFareOffersForSearchQuery` accept
  `type: "MONTH"` (a month of lowest fares per call)? Not observed.
- `pax` slot order beyond the first (adults); `C` city codes in
  `connections` (e.g. `SAO:C`, `PAR:C`).
- Does the AF/KL site ever beat Google Flights for AF/KL flights
  (web-only promo fares, "Tarifa Promo")?
- Flying Blue award search (`bookingFlow=REWARD`) needs login; not
  examined.
