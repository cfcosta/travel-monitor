# Smiles (smiles.com.br, GOL's miles program): award search

- Date: 2026-10-06
- Method: `chrome-cdp` against the user's Chrome, one tab, XHR/fetch
  hook installed before navigation; then one load in a fresh headless
  Chromium 151 (`--headless=new`). 3 page loads in the user's Chrome
  and 1 in headless. The front-end bundles (static JS) were read to
  find URL builders and API definitions.
- The user's Chrome was **not logged in** to Smiles ("Acessar conta";
  `members-green.smiles.com.br/v1/members` answered 452 "Token
  incorreto"). We did not log in.
- Test search: GRU→SSA, one way, 2026-11-10, 1 adult, all cabins.
- Fixtures: `fixtures/smiles/`.

## Search URLs

The home page's search widget (`smiles-react-flight-search`,
`createSearchFlightsUrl`) builds a GET URL that can be generated in
code:

```
https://www.smiles.com.br/mfe/emissao-passagem/?adults=1&cabin=ALL&children=0&departureDate=1794279600000&infants=0&isElegible=false&isFlexibleDateChecked=false&returnDate=&searchType=g3&segments=1&tripType=2&originAirport=GRU&originCity=&originCountry=&originAirportIsAny=false&destinationAirport=SSA&destinCity=&destinCountry=&destinAirportIsAny=false&novo-resultado-voos=true
```

| Param | Meaning |
| ----- | ------- |
| `tripType` | `1` round trip, `2` one way, `3` two destinations |
| `departureDate`, `returnDate` | Epoch milliseconds (`new Date(day).getTime()`); empty `returnDate` for one way |
| `cabin` | `ALL`, `ECONOMIC`, `BUSINESS` (also `PREMIUM_ECONOMIC`, `FIRST_CLASS` in the code). The form offers "Todas as classes / Econômica / Executiva"; domestic routes get Economy only |
| `searchType` | `g3` (GOL only), `both` (GOL and partner airlines, "congêneres") |
| `originAirport`, `destinationAirport` | IATA codes; `…City`, `…Country`, `…AirportIsAny` for "any airport in the city" (note the truncated `destin…` keys) |
| `adults`, `children`, `infants` | Passengers |
| `segments` | `1`, or `2` with `departureDate1/2` for two destinations |
| `novo-resultado-voos=true` | Selects the new results app at `/mfe/emissao-passagem` (otherwise `/emissao-com-milhas`) |

The generated URL loaded the results page ("GRU → SSA, Ter, 10 nov,
1 pessoa adulta") in the user's Chrome and in headless. Results never
arrived in either (see bot protection).

## Where prices come from

The results app (`/mfe-apps/flight-availability/0.32.1138/…`) calls,
from the browser:

```
GET https://api-air-flightsearch-green.smiles.com.br/v1/airlines/search
    ?cabin=ALL&originAirportCode=GRU&destinationAirportCode=SSA
    &departureDate=2026-11-10&memberNumber=&adults=1&children=0&infants=0
    &forceCongener=false&cookies=_gid%3Dundefined%3B
headers: x-api-key (public key from the page constants), Channel
withCredentials: true
```

(`-green` is the current blue/green deployment; it may become
`-blue`.) A round trip adds `returnDate`; two destinations add
`originAirportCode2`, `destinationAirportCode2`, `departureDate2`.
Before it, the page gets an anonymous partner token from
`POST apigw-green.smiles.com.br/b2b/partner/oauth/token/`.

**We captured no live response.** The field names below come from mock
responses in the app's own bundle (saved, labelled, as
`fixtures/smiles/airlines-search.bundle-mock.response.json`):

- `requestedFlightSegmentList[]` (`SEGMENT_1`, `SEGMENT_2`), each with
  `flightList[]`, `calendarDayList[]` and `bestPricing
  {miles, sourceFare, fare: {type}}`.
- `flightList[]`: `uid`, `cabin` (`ECONOMIC`…), `stops`, `sourceGDS`
  (`G3` or the partner), `departure`/`arrival` `{date, airport{code…}}`,
  `airline {code}`, `legList[] {flightNumber, cabin, equipment,
  marketingAirline, operationAirline}`.
- `flightList[].fareList[]`: one entry per fare type:
  `type` (`SMILES`, `SMILES_CLUB`, `SMILES_MONEY`, `SMILES_MONEY_CLUB`,
  outlet offers), `miles`, `money` (the cash part of Smiles & Money),
  `baseMiles`, `airlineFare`, `uid`.
- Top level: `hasCalendar`, `resultType`, `hasG3`, `hasCongener`,
  `tripTypeRequest`.

Taxes are **not** in the search response. The page says "Você vai
visualizar as taxas de viagem depois de escolher os voos". The bundle
gets them from `v1/airlines/flight/boardingtax` per fare `uid`
(API domain `ApiBoardingTax`). 0023 wants miles plus taxes, so each
recorded award needs a second call or a per-route tax estimate.

## Calendars

The bundle defines `ApiAirlinesCalendar` → `GET v1/airlines/calendar/month`
with `originAirportCode`, `destinationAirportCode`, `startDate`,
`endDate` (and `…2` variants for the return), `cabin`, passengers and
`memberNumber`. Its mock response
(`fixtures/smiles/airlines-calendar-month.bundle-mock.response.json`):

```json
{"calendarSegmentList":[{"type":"SEGMENT_1",
  "calendarDayList":[{"date":"2022-11-01","isLowestPrice":true,"miles":10000,
                      "priceRange":1,"fare":{"type":"SMILES_CLUB"}}, …]}],
 "calendarStatus":"ENABLED","hasCalendar":true,"tripType":"ONE_WAY"}
```

That is one award price per day for a whole month, the shape 0042
wants. The host for `ApiAirlinesCalendar` is set at runtime and was not
observed, and when the page calls it is not known (probably the
results page's date carousel or the "flexible dates" option,
`isFlexibleDateChecked=true`). The home page's date picker showed no
prices.

There is also `v1/airlines/search/mundo-inteiro` ("whole world":
destinations from an origin), like Google's Explore.

## Bot protection

**Akamai Bot Manager** on both www and the APIs: cookies `_abck`
(flag `-1`, not validated, in both browsers), `bm_sz`, `bm_so`,
`bm_sv`, `bm_lso`; sensor POSTs to an obfuscated first-party path
(`/Dq6XVtj0uXv_Z/…`); `server-timing: ak_p`.

- **User's Chrome:** the pages loaded, but the search call failed in
  the browser: *"blocked by CORS policy: Response to preflight request
  doesn't pass access control check: No 'Access-Control-Allow-Origin'
  header"*, `net::ERR_FAILED`, about 36–650 ms. The page then waits on
  "Aguarde enquanto buscamos os melhores voos" with no error. (The
  first attempt, submitting the home-page form, also hung the tab; it
  was closed.)
- **Headless Chromium:** the same: the page loaded, and the search
  preflight failed the same way.
- **Diagnosis:** a single `OPTIONS` to the same URL from curl got
  **HTTP 406** with Akamai's JSON block body (`"message": "Something
  went wrong"`, a `referenceId` and the client IP). Akamai answers the
  preflight with its block response, which has no CORS headers, so the
  browser reports a CORS error. We did not retry. We cannot tell from
  here whether the 406 is for this IP/fingerprint, for anonymous
  visitors, or for everyone at the moment.

## Recommendation for the daemon

Feasibility: **poor** for now. The URL can be generated and the API
shapes are known, but no browser got a single award price. Both the
real Chrome and headless were refused by Akamai at the search API.

If a logged-in session (0024) gets through:

1. Generated `/mfe/emissao-passagem/?…` URL, capture
   `url_contains: "/v1/airlines/search"` (one load per date and
   direction), plus `"/v1/airlines/flight/boardingtax"` if taxes are
   needed.
2. For the 6-month series, find what triggers
   `url_contains: "/v1/airlines/calendar/month"` (probably flexible
   dates or the results carousel) and drive it with Jev. That would be
   about 6 calls per leg per cabin.

Risks: the Akamai 406 on the API (current blocker); session expiry
(0024); the blue/green host names (`-green`) change; taxes need extra
calls; heavy pages (13 MB app bundle) per check.

## Open questions

- Does the search API answer (no 406) when the user is logged in to
  Smiles in a normal Chrome window? The user has to log in for this,
  and then one award search in their Chrome would settle it.
- Is the 406 specific to this IP (other agents browsing airline sites
  from the same address today) or to the time? Retry from the same
  Chrome on another day, once.
- Which UI action calls `v1/airlines/calendar/month`, and on which
  host? Is `startDate`–`endDate` limited to one month?
- Does `searchType=both` (partner airlines) change GOL's own award
  prices, or only add partners?
- Are Smiles Club (`SMILES_CLUB`) prices shown to non-members? The
  monitor should record the fare type with the miles.
