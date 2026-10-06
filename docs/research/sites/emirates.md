# Emirates (emirates.com, EK): GET search URL, session-bound JSON, Akamai blocks headless

- Date: 2026-10-06
- Method: `chrome-cdp` against the user's Chrome (one tab, an XHR/fetch
  hook installed before navigating, recording request headers) and a
  headless Chromium 151 from the dev shell (`--headless=new`, fresh
  profile, no user-agent override, the same flags as the daemon).
  Brazil site (`/br/portuguese`). Not logged in. Tests: GRU→DXB round
  trip 2026-11-10 to 2026-11-17 (form and URL), round trip 2026-11-12
  to 2026-11-19 (URL), one way 2026-11-12 (URL), the ±3-day flexible
  view for one way and round trip, and the `pub=/us/english` site.
- Load count: 7 page loads in the user's Chrome, plus 1 in-page click
  ("Outbound Next", "Inbound Next") and 2 in-page `fetch` calls to the
  calendar endpoint. 1 page load in headless (blocked).

## Summary

- A results page can be opened with a **plain GET URL**. No form
  filling is needed.
- Prices come from **same-origin JSON GETs under
  `/service/search-results/`**. They carry no search parameters. The
  search is held in the server session that the results URL creates.
  - `branded-fares`: the flight list, every cabin and fare brand, with
    exact prices.
  - `flexi-fares`: a 7-day one-way calendar, or a 7×7 round-trip grid.
- Once a session exists, `flexi-fares?departureDate=…&cabinClass=…`
  returns any week and any cabin. An in-page `fetch` got a business
  week and an April 2027 week with no new page load.
- **Akamai Bot Manager.** The user's Chrome passed. The daemon's
  headless Chromium got `/error/accessrestricted.html` on its first load.
- Feasibility: **poor** as the daemon is today (headless is blocked).
  It would be **good** in a browser that passes Akamai: one page load
  plus about 26 in-page calendar calls covers six months of one leg in
  one cabin.

## 1. Search entry and results URL

The home page form (`Buscar voos`) has origin, destination (an
autocomplete of airports), a calendar with "Um trecho" (one way) and
"(+/- 3 dias)" (flexible) checkboxes, passengers and cabin. It submits
a POST form to:

```
https://www.emirates.com/booking/search-results/?pageurl=/IBE&pub=/br/portuguese&j=f&section=IBE
```

Fields: `seldcity1`, `selacity1`, `selddate1` (`10-Nov-26`),
`seladate1`, `seladults`, `selchildren`, `selinfants`, `selteenager`,
`selofw`, `selcabinclass`, `TID` (`SB`), `chkFlexibleDates`,
`resultby`, plus some empty hidden fields.

**The same fields work as a GET query.** Tested from a cold navigation:

```
https://www.emirates.com/booking/search-results/?pageurl=/IBE&pub=/br/portuguese&j=f&section=IBE
  &seldcity1=GRU&selacity1=DXB&selddate1=12-Nov-26&seladate1=19-Nov-26
  &seladults=1&selchildren=0&selinfants=0&selcabinclass=0&TID=SB&resultby=0
```

- **One way:** leave out `seladate1` and set `TID=OW`. The page showed
  "Um trecho", and the redux `searchRequest` had
  `journeyType: "ONEWAY"`.
- **Flexible dates:** add `chkFlexibleDates=true`. The redux store then
  had `searchType: "CALENDAR"`, and the page showed the 7-day bar (one
  way) or the 7×7 grid (round trip) instead of flights.
- **Dates** are `DD-Mon-YY` with English month abbreviations.
- **Cabin:** `selcabinclass=0` is economy. The other codes were not
  tested by URL (see Open questions). The results page has buttons for
  Econômica / Econômica Premium / Executiva / Primeira, and the calendar
  endpoint takes `cabinClass=Y|W|J|F` directly (below).
- The server answers with the results page and then rewrites the URL to
  `?pub=…&<the same fields>&refreshId=<6 hex>&ttid=<opaque>`. `ttid`
  identifies the server-side search. It is not needed to start a search.

## 2. Where prices come from

The results page is a Next.js app (`__NEXT_DATA__`, a redux store at
`window.__NEXT_REDUX_STORE__`). `__NEXT_DATA__` holds the search
request (`api.searchRequest`, `api.searchIdentification`) but no
prices. The page then makes these same-origin XHRs, **all GET, with no
body and no search parameters**:

| URL                                                  | What it holds                                             |
| ---------------------------------------------------- | --------------------------------------------------------- |
| `/service/search-results/branded-fares`              | Flights, cabins, fare brands, prices (normal search)       |
| `/service/search-results/flexi-fares`                | Calendar (flexible search)                                |
| `/service/search-results/flexi-fares?departureDate=YYYY-MM-DD&cabinClass=Y` | Calendar for another week or cabin (one way, outbound) |
| `/service/search-results/flexi-fares?arrivalDate=YYYY-MM-DD&cabinClass=Y`   | Round-trip grid with the return week moved           |
| `/service/search-results/simplified-fare-rules?…`    | Fare rules per option (not needed)                        |
| `/service/search-results/currency-conversion-rates?pricedCurrency=BRL&lowestPrice=…` | Conversion table for the currency switcher |

The search itself is not in the request. It lives in the server
session that the results page's own request created, so these URLs
cannot be called without first loading a results URL.

### `branded-fares`

Fixture: `fixtures/emirates/branded-fares-rt-GRU-DXB-2026-11-10.json`
(round trip; this response is for the outbound choice).

```
currency: { sale: { code: "BRL", scale: 2 }, priced: { code: "USD" } }
lowestFare: { total: [{ amount: 7552.11, type: "CASH" }], taxTotal: [...] }   ← the whole trip
responseProvider: "VAYANT"
bounds[]: {
  type: "OUTBOUND", origin: "GRU", destination: "DXB",
  availableCabins: ["Y","W","J","F"],
  navigationPreviousDate / navigationNextDate: { date, applicable },
  options[]: {
    id: "GRU_DXB_EK262_2143637996", numberOfConnections, ondDuration: "14H5M",
    airSegments[]: { carrierCode: "EK", flightNumber: "262", departure, arrival,
                     departureDateTime: "2026-11-10T01:30:00", arrivalDateTime,
                     operatingCarrierCode, aircraftType: "388" },
    cabins[]: { cabinClass: "Y", status,
      brandInformation[]: { fareBrand: "SAVER"|"FLEX"|"FLEXPLUS"|"SPECIAL",
        status: "AVAILABLE"|"SOLD_OUT", fareBasisCode, seatsAvailable, rbd…,
        priceDetails.summary: { total: [{ amount: 4104.32 }], baseFare, taxTotal,
                                fuelSurchargeTotal, equivalentBase (USD) } } } } }
```

- **One response covers every cabin.** An economy search returned Y, W,
  J and F brands with prices for each flight. GRU→DXB 2026-11-10 EK262:
  Y Flex R$4,104.32, Y Flex Plus R$7,833.78, W Flex Plus R$8,078.09,
  J Flex R$17,895.32, F Flex R$43,338.36. Saver was sold out on the
  nonstop.
- Amounts are decimals in the sale currency (BRL), with `scale: 2`.
- **Round trips:** the outbound list shows a per-bound "a partir de"
  price (R$4,104.32). The trip total is `lowestFare` (R$7,552.11). The
  inbound list appears only after an outbound fare is chosen, so it was
  not opened.
- Codeshares are included, for example TAP-operated EK4815 GRU–LIS +
  EK192.

### `flexi-fares` (the calendar)

Fixtures: `fixtures/emirates/flexi-fares-ow-GRU-DXB-2026-11-12.json`
(one way) and `fixtures/emirates/flexi-fares-rt-GRU-DXB-2026-11-10_17.json`
(round trip).

```
currency: { sale: { code: "BRL" }, priced: { code: "USD" } }
options[]: {
  optionId, lowestPrice?: true,
  priceSummary: { total: { amount: 4308.74 }, baseFare: {...}, taxTotal: {...} },
  outbound: { travelDate: "2026-11-12T01:30:00", cabin: ["Y"] },
  inbound?:  { travelDate: "2026-11-14T09:05:00", cabin: ["Y"] }      ← round trip only
}
requestedCabinAvailable, mixedCabinGridDisplay, isAllLowestPriced
```

- One way: 7 options, the requested day ±3. Each is the cheapest price
  that day, with the date and time of the flight but **no flight
  number**.
- Round trip: 49 options (departure ±3 × return ±3). In the sample, the
  Nov 10/17 cell is R$7,552.11, the same as `branded-fares`'
  `lowestFare`.
- **"Outbound Next"** on the page fetched
  `flexi-fares?departureDate=2026-11-13&cabinClass=Y`, so it moves one
  day at a time. **"Inbound Next"** fetched
  `flexi-fares?arrivalDate=2026-11-18&cabinClass=Y`.
- **The parameters can be set freely within the session.** From inside
  the page, `fetch("/service/search-results/flexi-fares?departureDate=2026-11-19&cabinClass=J")`
  returned seven business days (R$23,514.42 to R$30,888.57), and
  `departureDate=2027-04-01&cabinClass=Y` returned Mar 29 to Apr 4
  2027. Both returned 200, with no extra headers.

## 3. Calendars

`flexi-fares` is the low-fare calendar: 7 days per call, as the
cheapest total per day, for any cabin (`Y`, `W`, `J`, `F`). Six months
of one leg in one cabin is about 26 calls. They can be in-page calls
after one results page load, as long as the session's route is the one
wanted. The calendar is tied to the session's origin and destination.
The return leg (DXB→GRU) needs its own results-page load.

The round-trip grid (49 cells per call) covers fixed trip lengths with
far fewer calls than one-way legs. It is the closest thing to Google's
price graph.

There is also a "Destinos em destaque" box on the home page (fares
"a partir de USD 1029" for Dubai, round trip, economy). That is
marketing content in USD, not usable as observations.

## 4. One way, round trip, cabins, points of sale

- One way and round trip: `TID=OW` without `seladate1`, or `TID=SB`
  with it. Multi-city exists through "Pesquisa avançada" (not tested).
- Cabins: Economy (Y), Premium Economy (W), Business (J), First (F).
  Fare brands: Special, Saver, Flex, Flex Plus. W on this route has
  Flex Plus only.
- **Point of sale follows the origin, not the site.** Loading the same
  one-way flexible URL with `pub=/us/english` gave the same BRL prices
  (R$4,308.74), and `searchIdentification` still said
  `currencyCode: "BRL", pointOfSale: { country: "BR", city: "SAO" }`.
  `priced` is always USD and `sale` is the origin country's currency.
  A foreign point of sale would need something other than `pub`, if it
  is possible at all (see Open questions).
- Display currency can be switched on the page ("Alteração de moeda"),
  using `currency-conversion-rates`. This converts for display and does
  not reprice.

## 5. Bot protection

- **Akamai Bot Manager**: cookies `_abck`, `bm_sz`, `bm_sv`, `bm_so`,
  `bm_lso`, and sensor POSTs to an obfuscated same-origin path
  (`/MbbhK1IMlP2_2YcKjw/…`, a body with `sensor_data`). Headless runs
  also set an `ekbot` cookie. No CAPTCHA was seen.
- **User's Chrome:** every load worked: the home page, the form submit,
  four generated GET URLs and the in-page calendar fetches.
- **Headless Chromium (daemon setup):** the first load, the one-way
  flexible URL opened directly, was redirected to
  `https://www.emirates.com/error/accessrestricted.html?refid=18.…`
  ("The page you're trying to access is restricted. Reference number:
  18.97601702…"). Per the rules, we stopped there. We did not retry,
  warm up on the home page first, or override the user agent. The
  headless user agent was the default (`HeadlessChrome/151`).

## 6. Recommendation for the daemon

**Feasibility: poor today. Good if the browser passes Akamai.**

Plan, once a non-blocked browser exists:

1. For each leg (origin → destination), load the generated one-way
   flexible URL (`TID=OW`, `chkFlexibleDates=true`, any date). Capture
   `url_contains = "/service/search-results/flexi-fares"`.
2. Ask the page for the remaining weeks with in-page
   `fetch("/service/search-results/flexi-fares?departureDate=D&cabinClass=C")`,
   stepping D by 7 days. That is about 26 calls per cabin for six
   months. The responses are captured like the page's own. This is a
   same-origin read the page itself makes with different parameters,
   not a token replay. It is a step beyond "the page makes the call"
   (0032), so it needs a decision.
3. For flight-level detail on a deal (flight numbers, fare brands),
   load the plain results URL for that day and capture
   `url_contains = "/service/search-results/branded-fares"`.
4. Round trip at 7 days: the round-trip flexible URL gives a 7×7 grid
   per load, and `arrivalDate`/`departureDate` move it.

Cost per route × cabin, six months: 2 page loads (one per leg) and
about 52 in-page calls. If in-page calls are not allowed: about 26 page
loads per leg, which is far too many for Akamai.

Risks: Akamai blocks the daemon's headless Chromium outright. The
session-bound design means every check starts with a results page load,
which is exactly what Akamai scores. Jev is not needed: the URL is
fully generated.

## Open questions

- Does Akamai still block when the daemon runs headful (the visible
  login-window profile, 0024) or uses the user's warmed profile? We did
  not test this, so as not to evade the block.
- `selcabinclass` codes for premium economy, business and first in the
  URL (0 is economy). Likely moot, because `flexi-fares` takes
  `cabinClass` directly.
- How long a search session stays valid for in-page `flexi-fares`
  calls, and how many calls Akamai tolerates in one session.
- Can a foreign point of sale be priced at all? `pub` did not change it.
  Possibly only by IP, through Mullvad.
- How far ahead `flexi-fares` answers (April 2027 worked; the 355-day
  limit was not probed).
- Round-trip inbound `branded-fares` (after choosing an outbound) was
  not opened, to avoid selecting into the booking flow.

## Fixtures (`fixtures/emirates/`)

- `branded-fares-rt-GRU-DXB-2026-11-10.json`: the flight list, round
  trip 2026-11-10/17, outbound, every cabin and brand.
- `flexi-fares-ow-GRU-DXB-2026-11-12.json`: the one-way 7-day calendar.
- `flexi-fares-rt-GRU-DXB-2026-11-10_17.json`: the round-trip 7×7 grid.

Session UUIDs (`transactionId`, `flightSearchId`, `id`) are replaced
with `<scrubbed>`. No cookies or headers are stored.
