# Turkish Airlines: price calendar, availability API and PerimeterX

- Date: 2026-10-06
- Site: `www.turkishairlines.com/pt-br/` (point of sale Brazil, BRL).
- Method: `chrome-cdp` against the user's Chrome (not logged in to
  Miles&Smiles), one tab, XHR/fetch hook installed before the first
  navigation; then the daemon's own Chromium 151 (`--headless=new`,
  default user agent, as `tm-browser` launches it).
- Test search: GRU→IST one-way, economy, 1 adult, 2026-11-10, filled in
  through the home-page form.
- Fixtures: `fixtures/turkish-airlines/`.

## Search entry

The search form is the "Voo" tab on the home page: trip type (`Ida e
volta`, `Apenas ida`, `Stopover in Istanbul`, `Várias cidades`),
`#fromPort` / `#toPort` comboboxes (type a code, pick
`#booker-option-N`), a date picker (react-calendar, day buttons labelled
`novembro 10 terça-feira, 2026`), and a "Passageiros" panel that also
holds the cabin. "Pesquisar voos" navigates to:

```
/pt-br/flights/booking/availability-international/?cId=<uuid>
```

**No usable deep link was found.** The results URL carries only a
conversation id; the search itself is kept in `sessionStorage`
(`availabilityPayload`, `bookerParams`, `X-conversationId`), so the URL
cannot be replayed in a fresh session.

The site does have a deep-link route,
`/<lang>/flights/booking/availabilitydeeplink/?…`. Its page POSTs the
whole URL to `POST /api/v1/availability/deeplink` (`{"url": "…"}`) and
the server parses it. A guess
(`?origin=GRU&destination=IST&departureDate=10-11-2026&adult=1&cabinClass=ECONOMY&tripType=O`)
returned `success: false`, `Error-DS-12029` ("não é possível processar
o seu pedido"). The parameter names are server-side only; not found in
the client bundles or on the web.

The home-page form's typed default origin was "São Paulo" resolved to
CGH, for which the price calendar is not offered; GRU must be chosen
explicitly.

## Where prices come from

All JSON over `https://www.turkishairlines.com/api/v1/…`. Every call
carries `X-clientId`, `X-requestId`, `X-conversationId`, `X-country:
br`, `X-platform: WEB`, `Accept-Language: pt`, and **`X-bfp`**, a
browser fingerprint produced by `/web4js/fp-loader.js`. Direct calls
would need that fingerprint, so the page must make them. Responses are
wrapped as `{ "data": …, "success": true, "statusDetailList": null }`.

### `POST /api/v1/availability/price-calendar`: about two months per call

Fired by the home-page date picker as soon as origin and destination
are set (after `POST /api/v1/availability/price-calendar/validate-with-date`
returns `validRoute: true` for the pair), and again when the picker is
paged to a month outside the range already loaded.

Request:

```json
{
  "selectedBookerSearch": "O",
  "selectedCabinClass": "ECONOMY",
  "inbound": false,
  "passengerTypeList": [{ "quantity": 1, "code": "ADULT" }],
  "originDestinationInformationList": [{
    "originAirportCode": "GRU", "originMultiPort": false,
    "destinationAirportCode": "IST", "destinationMultiPort": false,
    "departureDate": "06-10-2026"
  }]
}
```

Response:

```json
{ "data": {
    "cheapestPrice": { "currencyCode": "BRL", "amount": 5000.00, … },
    "currency": "BRL",
    "dailyPriceMap": {
      "2026-10-06": { "price": { "currencyCode": "BRL", "amount": 5500.00, … },
                      "bestPrice": false, "passiveDate": false, "soldOut": false, "rate": "0.37" },
      …
    } } }
```

- 62 days per call (2026-10-06 to 2026-12-06 for the first call;
  paging two months ahead fired a call with `departureDate:
  "01-12-2026"`). **Six months ≈ 3–4 calls, all from one page load.**
- Prices are **rounded indicative fares** (to R$100: 5,000 / 5,300 /
  5,500). The exact fare for Nov 10 was R$5,298.81, shown in the
  calendar as R$5,300 ("5,3 mil" in the picker).
- `selectedBookerSearch` is `O` (one way) or presumably `R` (round
  trip); `inbound: true` is presumably the return-leg calendar once an
  outbound date is chosen. Neither was captured.
- `selectedCabinClass`: `ECONOMY` captured; `BUSINESS` not tested (the
  site challenged us before the cabin could be changed).

### `POST /api/v1/availability`: the flight list

Request: the same search plus country and city codes
(`originCountryCode: "BR"`, `originCityCode: "SAO"`,
`destinationCityCode: "IST"`, `departureDate: "10-11-2026"`,
`moduleType: "TICKETING"`, `selectedCabinClass`).

Response `data` (55 kB):

- `economyStartingPrice` / `businessStartingPrice`: `{ brandCode,
  startingPrice: { amount, currencyCode } }`. One economy search
  returns the business minimum too (R$5,298.81 and R$20,630.46).
- `originDestinationInformationList[0].originDestinationOptionList[]`,
  one per itinerary (2 here, TK16 and TK216):
  - `segmentList[]`: `flightCode.airlineCode` + `.flightNumber`,
    `departureAirportCode`, `arrivalAirportCode`,
    `departureDateTime` (`10-11-2026 19:30`), `arrivalDateTime`,
    `equipmentCode`, `stopCount`.
  - `startingPrice`, `cheapest`, `soldOut`, `lastSeatCount`.
  - `fareCategory.ECONOMY` / `.BUSINESS.bookingPriceInfoList[]`, one
    per fare brand: `brandCode` (economy `CL`, `LG`, `GN`, `FL`;
    business `BF`, `BL`), `cabinType`, `grandTotalFare.amount` +
    `.currencyCode`, `fareBasisCodeList`, `resBookDesigCodeList`.
    Sample TK16: CL R$5,298.81, LG R$5,597.96, GN R$6,196.27,
    FL R$7,093.73, BF R$20,630.46, BL R$22,375.52.
- `originalCurrency: "BRL"`, `tripType: "O"`,
  `amadeusJSessionId` (session token; redacted in the fixture).

### `POST /api/v1/availability/cheapest-prices`: exact ±3 days

Fired on the results page. Same request as `availability`; response
`data.dailyPriceList[]` with `date` (`07-11-2026`), `price.amount`
(exact, e.g. 5,298.81 / 5,548.10), `bestPrice`, `soldOut`. Seven days.

### Other calls

`/api/v1/booking/locations/TK/pt?searchText=…` (airport lookup),
`/com.thy.web.online.ibs/ibs/booking/daterestrictions` and
`routerestrictionmessage`, `/api/v1/passenger/pax-type-codes`,
`/api/v1/loyalty/login-status`, CMS endpoints.

## One-way, round trip, cabins, point of sale

- One-way is native (`selectedBookerSearch: "O"`), and the calendar is
  per day for one leg, exactly what the monitor wants
  ([0042](../../../DECISIONS.md)).
- Cabins: `ECONOMY` and `BUSINESS` (TK sells no premium economy on
  this route). The availability response lists both cabins' brands.
- Point of sale: the path locale (`/pt-br/`), the `X-country` header,
  and cookies `geo_country` / `akamai_country` / `Locale`. Prices came
  in BRL. Other markets (`/en-us/`, `/en-tr/`) were not tested.

## Bot protection

- **PerimeterX / HUMAN**: `_px2`, `_pxhd`, `_pxvid`, `pxcts` cookies,
  `/DqLTvkTO/init.js`, collectors at `collector-PXDqLTvkTO.px-cloud.net`,
  `/web4js/human-bot-protection.js`. The challenge is the HUMAN
  "Press & Hold" overlay.
- **Akamai Bot Manager** at the edge: `bm_mi`, `bm_sv`, `/akam/13/…`.
- **`X-bfp`** fingerprint header on API calls (`fp-loader.js`).

What happened:

| Browser | Load | Result |
| --- | --- | --- |
| User's Chrome | 1. home `/pt-br/` | OK; form filled; **price-calendar** captured (3 calls, 2 of them identical) |
| | 2. results page (form submit) | **availability** + **cheapest-prices** captured |
| | 3. guessed `availabilitydeeplink` URL | page loaded; API `Error-DS-12029` |
| | 4. a misclick on an FAQ link | OK |
| | 5. home `/pt-br/` | OK |
| | 6. home `/pt-br/` | **PerimeterX "Press & Hold" challenge** ("Before we continue… confirm you are a human"). Stopped. |
| Headless Chromium 151 | 1. home `/pt-br/` | `net::ERR_HTTP2_PROTOCOL_ERROR` (reset at the edge) |

The headless rejection matches Air France/KLM (same error; `curl` with
the headless user agent got the same HTTP/2 stream reset), so it is
Akamai at the edge, before PerimeterX's script runs. A control load of
`wikipedia.org` in the same headless browser worked. No evasion was
tried.

## Recommendation

**Feasibility: poor as things stand; the mechanism itself would be
good.**

- Best mechanism: one page load of the home page, Jev (or a small fixed
  script) fills one-way + origin + destination (+ cabin), opens the date
  picker and pages forward; capture `url_contains:
  "/api/v1/availability/price-calendar"`. About 1 page load and 3–4
  captured calls cover six months of one-way per-day prices for one
  route × cabin. The prices are rounded to R$100, good enough for the
  scheduler and trends but not as an exact observation.
- Exact fares need a results page (`/api/v1/availability` plus
  `cheapest-prices`, ±3 days): 2 page loads per date, so only for
  confirmations.
- Blockers: the daemon's headless Chromium is reset at the edge on the
  first request, and the real Chrome hit a PerimeterX challenge after
  six loads. No deep link, so every check needs a form fill
  (interactions are what PerimeterX scores). Getting past either would
  mean evasion, which needs a project decision first.

## Open questions

- Is the headless block only the `HeadlessChrome` user agent? (Testing
  means overriding it; a policy decision.)
- How long does the PerimeterX challenge last for this IP and browser,
  and how many home-page loads per day does it tolerate?
- Parameter names for `/flights/booking/availabilitydeeplink/`. A
  Google Flights "Book with Turkish Airlines" redirect would show a real
  one.
- Does `price-calendar` accept `BUSINESS` and round trip
  (`selectedBookerSearch: "R"`, `inbound: true`)? Not captured.
- Are calendar prices always rounded to R$100, and are they cached
  (stale) relative to `availability`?
- Other points of sale (`/en-us/`, `/en-tr/`) and their currencies.
- Miles&Smiles award search (needs login); not examined.
