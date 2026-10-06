# LATAM Airlines (latamairlines.com, LA): URL search, JSON offers, Akamai blocks headless

- Date: 2026-10-06
- Method: `chrome-cdp` against the user's Chrome (one tab, XHR/fetch
  hook installed before navigating, also recording request headers) and
  a headless Chromium 151 from the dev shell (`--headless=new`, fresh
  profile, no user-agent override, the same flags as the daemon).
  Brazil site (`/br/pt`). Tests: GRU→LIS one way 2026-11-10; GRU→MIA
  round trip 2027-03-04 to 2027-03-11 (economy and business); both
  GRU→MIA legs one way; GRU→LIS under the US site.
- Load count: 9 page loads in the user's Chrome, plus 2 in-page clicks
  (one fare brand selected on the outbound leg to see the return leg).
  1 page load in headless, which was blocked.
- Fixtures: `fixtures/latam/` (listed at the end).

## Summary

| Question | Answer |
| -------- | ------ |
| Results URL can be generated | **Yes.** All parameters are in the query string. Tested 7 generated URLs. |
| Where prices come from | `GET /bff/air-offers/v2/offers/search?…` returns JSON with every itinerary and fare brand, with prices to the cent. |
| Many days per request | `GET /bff/air-offers/v2/calendar?…&extended=true` returns 2 months of daily lowest economy prices. These are cached, 2–5% above the live price. |
| Bot protection | Akamai Bot Manager (`_abck`, `bm_sz`, `ak_bmsc`, `bm_s*`, sensor POSTs), Akamai waiting room (`akavpau_CyberLatam`), and reCAPTCHA plus a signed search token on the offers call. |
| User's Chrome | Worked on every load. No challenge. |
| Headless Chromium | **Blocked on its first request**: Akamai "Access Denied" page. |
| Feasibility | **Possible.** The data is excellent, but headless is refused at the edge. |

## 1. Search entry and results URL

The search form is on `https://www.latamairlines.com/br/pt`. The
results page is a client-side app that reads its search from the URL:

```
https://www.latamairlines.com/br/pt/oferta-voos
  ?origin=GRU
  &destination=LIS
  &outbound=2026-11-10T12%3A00%3A00.000Z
  &inbound=null                       (or 2026-11-17T12%3A00%3A00.000Z)
  &adt=1&chd=0&inf=0
  &trip=OW                            (or RT)
  &cabin=Economy                      (or Business)
  &redemption=false                   (true = LATAM Pass, see latam-pass.md)
  &sort=RECOMMENDED
```

- Dates are ISO timestamps. The page uses only the date part, and noon
  UTC avoids a timezone shift.
- The point of sale is the path: `/br/pt/oferta-voos` (Brazil, BRL),
  `/us/en/flight-offers` (United States, USD). The path word changes
  with the language. The currency follows the country. No currency
  parameter was seen.
- Origin and destination take airport codes (GRU, LIS, MIA). City codes
  (SAO) were not tested. The home page itself uses `origin=SAO` for its
  deals.
- Every generated URL loaded straight into results, with no form step
  and no redirect.

## 2. Where prices come from

The results page makes these same-origin XHR calls, all `GET`:

| Call | What it returns |
| ---- | --------------- |
| `/bff/air-offers/v2/offers/search?…` | The itineraries and their fares. 150–315 KB. |
| `/bff/air-offers/v2/calendar?…&isRoundTrip=…&extended=true` | The price carousel above the results: daily lowest prices for 2 months. |
| `/bff/air-offers/v2/calendar?…&isRoundTrip=…` (no `extended`) | Date-picker metadata. Always empty `detailsCalendar` in our tests. |

### `offers/search`

The query string repeats the search:
`origin, destination, outFrom=2026-11-10, inFrom=null|YYYY-MM-DD,
adult, child, infant, cabinType=Economy|Business, redemption=false,
sort=RECOMMENDED, locale=pt-br, outOfferId=null, inOfferId=null,
outFlightDate=null, inFlightDate=null`, plus tracking fields set to the
string `undefined` (`utm_*`, `kayakclickid`, `idMetasearch`).

Response (Spring page object):

```
{ totalElements, totalPages, numberOfElements, pageable{pageSize: 50},
  content: [ {
    summary: {
      flightCode: "LA8146", stopOvers: 0, duration: 580 (minutes),
      origin: { departure: "2026-11-10T17:40:00", iataCode: "GRU", … },
      destination: { arrival: "2026-11-11T06:20:00", iataCode: "LIS", … },
      lowestPrice: { currency: "BRL", amount: 1913.39, … },
      brands: [ {
        id: "SL", brandText: "LIGHT",
        cabin: { id: "Y", label: "Economy" },
        farebasis: "AKTSLZ6E", offerId: "<64 hex>",
        price: { currency: "BRL", amount: 1913.39, display: "BRL 1.913,39" },
        priceAfterSelection: { … }, seatsRemaining: { count: 9 }
      }, … ],
      flightOperators: ["LATAM Airlines Brasil"], …
    },
    itinerary: [ { flight: { airlineCode: "LA", flightNumber: 8146,
                             operatingAirlineCode: "LA" },
                   origin: "GRU", destination: "LIS",
                   departure, arrival, equipment: "773", … } ],
    newPrices: [ { fare: 1844.78, taxes: 68.61, total: 1913.39 }, … ]
                   (one per brand, same order)
  } ],
  promo, scarcity, extraDetails }
```

- Amounts are decimal numbers in the point of sale's currency, taxes
  included. `newPrices` splits fare and taxes.
- `summary.brands[]` is one entry per fare family. Brands seen:
  LIGHT `SL`, STANDARD `KM`, PLUS `SE`, FULL `KD`, DELTA COMFORT `SF`
  (all cabin `Y`), PREMIUM ECONOMY `RY` (cabin `W`, label
  `PremiumEconomy`), PREMIUM BUSINESS STANDARD `EV` and PREMIUM
  BUSINESS FULL `EJ` (cabin `J`).
- **A `cabinType=Economy` search returns every cabin.** Each itinerary
  carries its Y, W and J brands. A `cabinType=Business` search returns
  only J brands, at the same prices (RT GRU–MIA outbound EV R$10,304.60
  in both). So one economy search per route and date prices all three
  cabins.
- Codeshares are included: on GRU–MIA, 30–40% of segments were
  operated by Delta (`operatingAirlineCode: "DL"`).
- `totalPages` was 1 in every test, but GRU–MIA one way hit exactly
  50 (`pageSize`). Larger markets may page.

Sample, Brazil, economy LIGHT (cheapest), one adult:

| Search | Cheapest | Notes |
| ------ | -------- | ----- |
| GRU→LIS OW 2026-11-10 | R$1,913.39 | LA8146 nonstop, 29 itineraries; business EV R$7,692.05 |
| GRU→LIS OW 2026-11-10, `/us/en` | US$383.10 | the same flight; business US$1,542.10 |
| GRU→MIA OW 2027-03-04 | R$4,176.93 | nonstop LA8198 |
| MIA→GRU OW 2027-03-11 | R$2,276.05 | via LIM; nonstop R$2,510.39 |
| GRU→MIA RT 2027-03-04/11, outbound | R$2,736.00 | fare R$2,458.04 + taxes R$277.96 |
| … the same RT, return (after choosing the outbound) | R$2,625.07 | fare R$2,458.05 + taxes R$167.02 |

### Round trips

A round trip is two `offers/search` calls. The first returns the
outbound options, each priced as its half of the round trip. Choosing a
brand (`Escolher`, then `Continuar` past the LATAM+ upsell) makes the
second call with `outOfferId=<chosen offerId>` and
`outFlightDate=<arrival time>`. It returns the return options, again
priced per leg. The trip's total is the sum of the two legs.

On GRU–MIA, 2027-03-04/11 (nonstop, LIGHT): round trip R$2,736.00 +
R$2,625.07 = **R$5,361.07**. The two one-ways cost R$4,176.93 +
R$2,510.39 = R$6,687.32. **The round trip is 20% cheaper**, in line
with the 15–21% gap found on Google Flights (0042).

The URL does not carry the selected outbound. Pricing a round trip
therefore needs one click and one more request after the page loads.
That is about 3 interactions in Jev, or 2 clicks on known selectors:
`[data-testid="wrapper-card-header-0"]`, then
`#bundle-detail-0-flight-select`, then `#latam-plus-continue` when
that modal appears.

## 3. Calendars (many days per request)

`GET /bff/air-offers/v2/calendar?origin=GRU&destination=LIS&month=11&year=2026&isRoundTrip=false&extended=true`

```
{ disabledDays: [],
  days: [ { calendar: [ {
     month: "2026-11", origin, destination,
     travelType: "ONEWAY" | "ROUNDTRIP",
     direction: "OUTBOUND" | "INBOUND",
     minimum | maximum | average: { amount, currency },
     detailsCalendar: [ { date: "2026-11-01",
                          fare: { amount: 2001.41, currency: "BRL" },
                          formattedAmount: "2K", percentile: 0,
                          enabled: true, lowPrice: true }, … ]
  } ] }, … ] }
```

- **One call returns two months**: the requested month and the next.
  On a one-way search the page asks for the month before and the month
  of the search (4 months of days across 2 calls). Six months of one
  direction is 3 calls.
- `isRoundTrip=true` returns OUTBOUND and INBOUND entries per day. Each
  entry is that leg's share of a round trip, comparable with the
  round-trip leg prices above.
- **No cabin, passenger or brand parameter.** It is the cheapest
  economy price only.
- **The values are cached and not exact.** They ran 2–5% above the
  live cheapest price on the same day:

  | Day | Calendar | Live `offers/search` |
  | --- | -------- | -------------------- |
  | GRU→LIS OW 2026-11-10 | R$2,001.41 | R$1,913.39 (+4.6%) |
  | GRU→MIA OW 2027-03-04 | R$4,250.72 | R$4,176.93 (+1.8%) |
  | MIA→GRU OW 2027-03-11 | R$2,484.43 | R$2,276.05 (+9%; R$2,510.39 nonstop) |
  | GRU→MIA RT outbound 03-04 | R$2,863.27 | R$2,736.00 (+4.6%) |
  | GRU→MIA RT inbound 03-11 | R$2,750.31 | R$2,625.07 (+4.8%) |

  GRU→LIS showed one flat R$2,001.41 for every day from Nov 1 to Dec 9,
  then steps. That looks like a cached fare table, not daily shopping.
- The page's carousel shows these numbers. The `/us/en` results page
  made no `extended=true` call.
- The calendar call needs only the plain `X-latam-*` headers (no
  captcha, no search token). A `fetch` from inside the page with those
  headers returned 200 for other months and for `isRoundTrip=true`. A
  `fetch` without them returned
  `400 {"description":"Missing latam headers"}`. See
  `fixtures/latam/request-headers.json`.

The calendar is a good **scheduler hint**: a cheap per-day shape of the
next six months in 3 calls per direction. A real `offers/search` must
confirm the price, as on Google Flights (0031).

## 4. One way, round trip, cabins, points of sale

- `trip=OW|RT`. Round-trip leg prices are per leg; the sum is the trip.
- Cabins: `cabin=Economy` returns Y, W (premium economy, only on some
  segments, such as via Lima) and J brands together. `cabin=Business`
  returns only J. We did not find a premium-economy-only value.
- Points of sale: the path segment (`/br/pt`, `/us/en`). The page sends
  `X-latam-Application-Country`, `-Lang` and `-Oc` from it. The US
  site priced GRU→LIS 2026-11-10 at US$383.10 against R$1,913.39 in
  Brazil (R$4.99 per dollar). Whether that beats Brazil after IOF and
  spread depends on the day's rate. Other countries (`/pt/pt`, `/cl/es`,
  `/ar/es`) were not tested.
- Passengers: `adt`, `chd`, `inf` in the URL. Prices are per person.

## 5. Bot protection

**Vendor: Akamai.** Seen on the first load in the user's Chrome:

- Cookies `_abck`, `bm_sz`, `ak_bmsc`, `bm_s`, `bm_so`, `bm_sv`,
  `bm_lso` (Bot Manager), `akavpau_CyberLatam` (Akamai visitor
  prioritization, a waiting room), `RT` (mPulse).
- Sensor POSTs to an obfuscated same-origin path
  (`/OeQB0y2…/…`, 201) on every page, and `/akam/13/pixel_…`.
- On `offers/search` only, two more headers set by the page:
  - `x-latam-search-token`: an HS512 JWT with a 20-minute lifetime.
    Its payload is `{country, language, origin, destination, iat, exp}`
    and the message "Hi Hacker friend, if you need to see our offers
    out of our site, feel free to contact us …".
  - `x-latam-captcha-token`: a reCAPTCHA token (about 2 KB). It is
    invisible: no challenge was ever shown.

We did not call `offers/search` ourselves. It needs a fresh captcha
token, and forging one would mean getting around the bot protection.
The page makes the call itself on every results load, so capturing it
is enough.

**User's Chrome:** 9 loads and 2 clicks, all fine. No challenge, no
slowdown.

**Headless Chromium** (UA `HeadlessChrome/151.0.0.0`, fresh profile):
the first request to `https://www.latamairlines.com/br/pt` returned the
Akamai **"Access Denied"** page ("For security reasons, we cannot grant
you access to our website at this moment …", `Reference Code:
#0.9a601702.…`). No JavaScript ran, so this is an edge rule, most
likely on the `HeadlessChrome` user agent or the TLS/HTTP fingerprint.
Following the research rules we stopped there. We did not try a
user-agent override or a headed window.
`fixtures/latam/headless-access-denied.html`.

## 6. Recommendation for the daemon

**Feasibility: possible.** The extraction is easy and exact once the
page loads. Getting the page to load in the daemon's browser is the
open problem.

If the daemon's browser can load the site:

- **Generated URL + capture.** Build the `oferta-voos` URL in code.
  Capture `url_contains = "/bff/air-offers/v2/offers/search"` for
  prices and `"/bff/air-offers/v2/calendar"` (keep only responses with
  non-empty `detailsCalendar`) for the carousel. No Jev is needed for
  one-way checks.
- **Per-day legs (0042):** one `trip=OW&cabin=Economy` load per route,
  direction and date gives exact Y, W and J prices for that day, plus
  the 2-month calendar. The calendar can rank which days to load. Six
  months exact would be about 180 loads per direction, too many, so use
  the calendar as the hint and spot-check.
- **More calendar months:** either move the carousel (Jev, untested) or
  have the daemon `fetch` `/bff/air-offers/v2/calendar` from inside a
  loaded page with the `X-latam-*` headers (3 calls per direction for 6
  months, no captcha). The second is a direct API call; decide whether
  that is acceptable (open question).
- **Round trip confirmation:** load the RT URL, click the first card's
  cheapest brand and `Continuar`, then capture the second
  `offers/search`. 1 load plus 2–3 clicks.
- Estimated cost per route × direction check: 1 page load (results +
  calendar). Per round-trip confirmation: 1 load + 3 clicks.

Risks:

- The headless block. Options to decide (none tested): run this source
  with the daemon's `--headed` mode, or override the user agent.
  Akamai also scores behaviour, so even then a fresh profile may be
  challenged later.
- reCAPTCHA scoring on `offers/search` may start failing for a profile
  that only ever loads results URLs. Mixing in a home-page load per
  session may help.
- `pageSize` 50: big markets may need a second page.
- The calendar is cached and runs a few percent high. Never record it
  as an observation.

## Open questions

1. Does the daemon's Chromium get past the Akamai edge headed, or with
   a normal Chrome user agent? Is either acceptable under our rules?
2. Is calling `/bff/air-offers/v2/calendar` from inside a loaded page
   (plain headers, no tokens) acceptable, or should the daemon only
   capture what the page requests itself?
3. How far ahead does the calendar return data (we saw up to
   2027-04)? What does it return for a month with no flights?
4. Is there a `cabinType` value for premium economy alone?
5. Do city codes (`origin=SAO`) work in the results URL?
6. Which other points of sale exist (`/pt/pt`, `/cl/es`, `/ar/es`,
   `/co/es`, `/pe/es`), and do they quote in local currency?
7. Does `offers/search` ever return `totalPages > 1`, and does the page
   then request page 2?

## Fixtures (`fixtures/latam/`)

| File | Source |
| ---- | ------ |
| `offers-search-ow-gru-lis-2026-11-10-br.json` | `offers/search`, GRU→LIS OW, Brazil, all 29 itineraries |
| `offers-search-ow-gru-lis-2026-11-10-us.json` | the same under `/us/en` (USD), first 6 itineraries |
| `offers-search-rt-gru-mia-2027-03-04-outbound-br.json` | RT economy, outbound leg, first 6 of 50 |
| `offers-search-rt-gru-mia-2027-03-11-inbound-br.json` | RT economy, return leg after choosing LA8198 LIGHT, first 6 of 45 |
| `offers-search-rt-gru-mia-2027-03-04-business-br.json` | RT `cabinType=Business`, outbound, first 6 of 44 |
| `calendar-ow-gru-lis-2026-11-extended.json` | `calendar … isRoundTrip=false&extended=true` (Nov–Dec 2026) |
| `calendar-ow-gru-lis-2026-11-plain.json` | `calendar` without `extended` (empty) |
| `calendar-rt-gru-mia-2027-03-extended.json` | `calendar … isRoundTrip=true&extended=true` (Mar–Apr 2027, OUTBOUND + INBOUND), fetched from inside the page |
| `request-headers.json` | header names the page sets; ids and tokens replaced |
| `headless-access-denied.html` | the Akamai block page served to headless |

`extraDetails` (per-session track ids) is replaced by `<scrubbed>` in
every offers fixture. The fixtures contain no cookies, tokens or
account data.
