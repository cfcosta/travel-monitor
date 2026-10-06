# TAP Air Portugal (flytap.com, TP): deep links, BFM REST API and the monthly calendar

- Date: 2026-10-06
- Sites: `www.flytap.com` (Next.js marketing site with the search form)
  and `booking.flytap.com` (the Angular booking engine, "IBE", backed by a
  REST API under `/bfm/rest/`).
- Method: `chrome-cdp` against the user's Chrome (not logged in to TAP
  Miles&Go), one tab with an XHR/fetch hook installed before the first
  navigation, 3 page loads. Then the daemon's own Chromium 151
  (`--headless=new`, default `HeadlessChrome` user agent, fresh profile,
  as `tm-browser` launches it), 3 page loads plus in-app navigation.
- Test searches: GRU→LIS one-way 2026-11-10 (BR and US markets); GRU→LIS
  round trip 2026-11-10 / 2026-11-17 (BR); the monthly calendar for
  GRU→LIS and LIS→GRU.
- Result: **prices captured in both browsers, no challenge.**
- Fixtures: [fixtures/tap/](fixtures/tap/README.md).

## Search URLs

The form on `www.flytap.com/pt-br` builds a deep link in its JavaScript
(found in the page bundle) and opens it in a new tab. The deep link can
be generated directly:

```
https://booking.flytap.com/booking/flights/deeplink?market=BR&language=pt&origin=GRU&destination=LIS&flexibleDates=false&flightType=single&adt=1&chd=0&inf=0&yth=0&depDate=10.11.2026&x_tap_source=WEB
```

- `flightType`: `single` (one-way), `return` (round trip, adds
  `retDate=dd.MM.yyyy`), `openJaw` (multi-city, which uses
  `/booking/flights-multicity/deeplink` with `origin1`, `destination1`,
  `depDate1`… for the later legs; not tested).
- `depDate` / `retDate`: `dd.MM.yyyy`.
- `adt`, `yth`, `chd`, `inf`: passenger counts.
- `market`: the point of sale (`BR`, `US`, `PT`…). It sets the currency
  and the selling office (see below). `language`: `pt`, `en`…
- `flexibleDates`: `true` or `false`. Both produced the same request and
  the same page; it does not open the calendar.
- `award=true` is added by the form's "Pagar com milhas" switch (award
  search; not tested, needs login).
- There is **no cabin parameter**. The availability response always holds
  every cabin (see below).
- `headerfooterhidden=false` is also sent by the form; it is optional.

The deep link redirects to
`/booking/flights?origin=GRU&destination=LIS&…` after the app posts the
parameters to `POST /bfm/rest/deeplink/ibe`. Both a one-way and a round
trip deep link loaded the results directly, in both browsers.

## Where prices come from

Every API call goes to `https://booking.flytap.com/bfm/rest/…` with
`Authorization: Bearer <token>`. The token comes from
`POST /bfm/rest/session/create` on page load and is kept in
`sessionStorage.token`. The page makes all these calls itself; nothing
has to be replayed.

### Availability: `POST /bfm/rest/booking/availability/search`

URL: `/bfm/rest/booking/availability/search?payWithMiles=false&starAlliance=false`.
Request (from the deep link, one-way):

```json
{"adt":1,"airlineId":"TP","bfmModule":"BFM_BOOKING","cabinClass":"E",
 "departureDate":["10112026"],"destination":["LIS"],"origin":["GRU"],
 "language":"pt-br","market":"BR","oneWay":true,"tripType":"O",
 "returnDate":"10112026","passengers":{"ADT":1,"YTH":0,"CHD":0,"INF":0},
 "...": "about 30 fields in all, see fixtures/tap/availability-search.request.json"}
```

Round trip: `tripType: "R"`, `oneWay: false`, `returnDate` set.

Response (`data`, about 260 KB uncompressed, 10 KB on the wire):

- `data.offers.currency`: `"BRL"`.
- `data.offers.listOffers[]`: one entry per flight × fare family.
  - `outFareFamily`: `DISCINT` (Discount), `BASINT` (Basic), `CLAINT`
    (Classic), `PLUSINT` (Plus) are economy; `RPEINT`, `TPEINT` are
    Economy Prime (premium economy); `EXEINT`, `TOPINT` are business.
  - `outCabin`: `M` economy, `W` Economy Prime, `C` business. Also
    `outbound.cabin: ["Economy"]`, `outbound.rbd`, `outbound.fareBasis`.
  - `totalPrice.price` (total, 1911.99), `basePrice`, `tax`.
  - `groupFlights[]`: `{idOutBound, idInBound, seatOutBound}`, linking the
    offer to flights in `listOutbound` (and `listInbound`).
  - Round trip: `inFareFamily`, `inCabin`, and the price is for the whole
    trip.
- `data.listOutbound[]`: `idFlight`, `duration` (minutes),
  `numberOfStops`, `relateOffer` (offer ids), `listSegment[]` with
  `departureAirport`, `arrivalAirport`, `departureDate`, `arrivalDate`,
  `carrier`, `flightNumber`, `operationCarrier`, `equipment`.
  **The times are local times with a fake `Z`**: TP88's
  `"2026-11-10T21:45:00.000Z"` is the 21:45 departure shown on the page.
- `data.outPanel.listTab[]` (and `inPanel` for round trips): the 7-day
  ribbon around the searched date (±3 days), `date` (`ddMMyyyy`) and
  `totalPrice.price`, the lowest price of each day.
- `data.officeId`: the selling office (`SAOTP08AA` for BR,
  `EWRTP08AA` for US).

GRU→LIS one-way on 2026-11-10 (BR): 35 flights (3 nonstop), 92 offers.
Lowest R$1,911.99 economy (Discount), Economy Prime R$5,639.72, business
R$12,142.79, all on the nonstops.

### Monthly calendar: `POST /bfm/rest/booking/availability/calendar/`

The booking flow's first step, "Datas" (`/booking/dates`), is a month
calendar with one price per day. Clicking the "Datas" step on the results
page opens it (in-app navigation, no page load). The page fetches **one
month per request**, when that month scrolls into view:

```json
{"cabinClass":"E","destination":"LIS","market":"BR","origin":"GRU",
 "tripType":"O","paxType":"ADT","month":11,"year":2026}
```

Response: `data.bestPriceForDates[]`, one per day:

```json
{"departureAirport":"GRU","arrivalAirport":"LIS",
 "departureDate":"2026-11-02T00:00:00","bestTotalPrice":1911.99,
 "currency":"BRL","cabinClass":"E","tripType":"O","market":"BR",
 "insertionDate":"2026-10-06T05:01:55","monthlyMinimum":true,
 "monthlyMaximum":false,"soldOut":false,"noFlights":false,
 "starAlliance":false,"bestTotalMiles":0}
```

- The prices are **cached**: `insertionDate` says when each one was
  stored. Near months were fresh (Oct/Nov/Dec 2026: 0 to 3 days old). Far
  months were older: Feb 2027 from 2026-09-11, Mar and May 2027 from
  2026-07-16, Jul 2027 from 2026-07-29.
- The calendar matched the live search where we compared: 2026-11-10
  showed R$1,911.99 in both.
- **Economy only.** The same request with `cabinClass: "C"` returned
  exactly the economy prices.
- The calendar goes about 10 months ahead: from 2026-10 to 2027-07 had
  prices; Aug and Sep 2027 came back empty (`bestPriceForDates: []`).
- The return leg is the same request with origin and destination swapped
  (LIS→GRU, `market: "BR"`, BRL prices).
- The `market` in the body wins over the session: a call with
  `market: "BR"` returned BRL from a session opened under `market=US`.
- Round-trip calendars exist too (`/booking/availability/calendarReturns/`,
  with the chosen `departureDate`); not captured.

In-page calls with the page's own token (`sessionStorage.token`) worked:
one month per call, about 1 KB on the wire. We made three of them (Nov
economy, Nov "business", LIS→GRU Nov) to check the fields. The daemon
does not need to do this: scrolling the calendar makes the page fetch
each month.

### Monthly minimum: `GET /bfm/rest/booking/availability/calendar/prices/lowest/monthly`

`?marketCode=BR&origin=GRU&destination=LIS&tripType=O&paxType=ADT` gives
the lowest price for each of the 12 months
(`monthlyLowestPrices.data[]: {month, price}`). It said R$2,647.04 for
November while the daily calendar had R$1,911.99, so it is stale or
computed differently. Not useful as a price source.

## Calendars and the monitor's per-day one-way prices (0042)

The monthly calendar is what decision 0042 needs: one request covers a
whole month of one-way, per-day economy prices for one direction and one
market. Six months × two legs is 12 to 14 calendar responses per route ×
point of sale, from two page loads (one deep link per direction) plus
scrolling.

Business and Economy Prime have no calendar. Their per-day prices come
only from `availability/search`, one date per request (each response
also gives the ±3-day ribbon, but the ribbon is the lowest fare of any
cabin, which is economy).

## Points of sale

`market` in the deep link sets the point of sale. Under `market=US`
(language `en`) the same one-way search returned USD prices (lowest
$373.80, 86 offers, office `EWRTP08AA`), against R$1,911.99 under `BR`.
At about R$5.3 per dollar that is R$1,980 before IOF, so the US market
was not cheaper here. The `/bfm/rest/booking/market` call confirms the
market and returns its currency and decimals.

## Bot protection

- **Cloudflare**: cookies `__cf_bm` and `cf_clearance`, and the
  `/cdn-cgi/challenge-platform/.../precursor/main.js` script, which posts
  to `/cdn-cgi/challenge-platform/h/b/precursor/...` a few times per page.
  No challenge page appeared.
- Also on the page: Dynatrace RUM (`dtCookie`, `rxVisitor`), VWO, Adobe,
  Clarity, Criteo, Salecycle (analytics and A/B testing, not blocking).
- **User's Chrome**: all three loads worked.
- **Headless Chromium** (default `HeadlessChrome` user agent, fresh
  profile, cold start straight on the deep link, no home page first): the
  one-way BR, round-trip BR and one-way US deep links all returned full
  results, and the "Datas" calendar loaded and fetched 12 months while
  scrolling.

## Notes for capture

- In our page-side hook, `XMLHttpRequest` `loadend` saw status 0 and an
  empty body for every IBE call on the first load. Recording on
  `readystatechange` (`readyState === 4`) worked. The daemon reads
  responses through CDP `Network`, so this should not affect it, but it
  shows the IBE's XHR wrappers (zone.js, Dynatrace) are unusual.
- The cookie banner (OneTrust) covers the bottom of the page; nothing
  needed it dismissed.

## Recommendation

**Feasibility: good.**

1. Per route × direction × market: load the generated one-way deep link
   (1 page load). Capture `bfm/rest/booking/availability/search`: all
   cabins for that date, and the ±3-day ribbon.
2. Click the "Datas" step (in-app, Jev or a fixed selector: the step
   label "Datas"/"Dates" in the progress bar), then scroll the calendar
   month by month. Capture `bfm/rest/booking/availability/calendar/`:
   one month of per-day economy prices per response.
3. For business or Economy Prime on a date, load that date's deep link
   and read `listOffers` (`outCabin` `C` or `W`).

Estimated cost: 2 page loads and about 14 calendar responses for six
months of both legs (economy) per route and market; 1 page load per
date for business or Economy Prime.

`url_contains` substrings: `bfm/rest/booking/availability/search` and
`bfm/rest/booking/availability/calendar/` (the second also matches
`calendarReturns/` and `calendar/prices/lowest/monthly`; filter on the
path end or on the request body).

Risks:

- Calendar prices are cached, sometimes for months. Use
  `insertionDate` to weight or refresh them; confirm deals with an
  availability search.
- Cloudflare may start challenging at a higher rate or from another IP;
  only 3 headless loads were made.
- The deep-link API is the marketing site's own; parameter names could
  change with a redeploy.

## Open questions

- Does the round-trip calendar (`calendarReturns/`) give per-day return
  prices for a fixed outbound, and is it worth using for 0042's 7-day
  round trips?
- How stale can the calendar's `insertionDate` get on less popular
  routes, and does an availability search refresh the cached day?
- Award search (`award=true`, `payWithMiles=true`): what does it need
  (login) and does the calendar return `bestTotalMiles` then?
- Rate limits: how many deep-link loads per hour before Cloudflare
  challenges a headless browser?
- Does `market=PT` (EUR) differ from BR for this route?
