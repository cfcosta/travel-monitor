# GOL (voegol.com.br, G3): search URLs, price APIs and bot protection

- Date: 2026-10-06
- Method: `chrome-cdp` against the user's Chrome (not logged in), one tab,
  XHR/fetch hook installed before navigation; then one load in a fresh
  headless Chromium 151 (`--headless=new`, as the daemon launches it).
  About 11 page loads in the user's Chrome and 1 in headless.
- Test searches: GRU→SSA and CGH→SSA one way on 2026-11-10;
  GRU→AEP one way on 2026-11-10 and round trip 2026-11-10 to 2026-11-17.
- Fixtures: `fixtures/gol/`.

## Search URLs

The search form on `www.voegol.com.br` is a web component
(`<gol-flight-search>`, Angular, in shadow DOM, from
`comp.voegol.com.br/component/browser/main.bundle.js`). Its
`buildB2CSearchUrl` builds a plain GET URL, and that URL can be
generated in code:

```
https://b2c.voegol.com.br/compra/busca-parceiros?pv=br&tipo=DF&lang=pt-BR&de=GRU&para=SSA&ida=10-11-2026&ADT=1&ADL=0&CHD=0&INF=0&voebiz=0
```

| Param          | Meaning                                                                 |
| -------------- | ----------------------------------------------------------------------- |
| `pv`           | Point of sale: `br`, `us`, `eu`, `ar`, `py`, `uy` (from currency BRL/USD/EUR/ARS/PYG/UYU) |
| `tipo`         | Always `DF`                                                             |
| `lang`         | `pt-BR`, `es-AR`, `en-US`…                                              |
| `de`, `para`   | IATA airport codes                                                      |
| `ida`, `volta` | Dates as `DD-MM-YYYY`; leave `volta` out for one way                    |
| `ADT`, `ADL`, `CHD`, `INF` | Adults, teenagers, children, infants                        |
| `voebiz`       | `0`                                                                     |
| `promocode`    | Optional discount code                                                  |
| `additionalItineraries` | Multi-city: `FROM,TO,DD-MM-YYYY`                               |

The URL redirects to `/compra/selecao-de-voo2/ida`, which shows the
results. Generated URLs for GRU→SSA, CGH→SSA, GRU→AEP (one way and
round trip) and `pv=ar` all loaded correctly.

There is no cabin parameter. Cabins are fare families inside the
results (see below). City codes (SAO, BUE) were not tested; airports
work.

## Where prices come from

### `POST https://bff-flight.voegol.com.br/flights/search`

The results page makes one call per search. The response comes back as
a `Blob` (`responseType: "blob"`), so a capture hook must read
`response.text()`; CDP `Network.getResponseBody` is unaffected.

Request (fixture `flights-search.oneway-GRU-SSA-2026-11-10.request.json`):

```json
{"searchType":"BRANDED","promoCodes":[],"pointOfSale":"BR","currency":null,
 "itineraryParts":[{"fromCode":"GRU","toCode":"SSA","when":"2026-11-10"}],
 "passengers":{"adt":1,"chd":0,"inf":0},"hasCourtesyTicket":false,
 "orderSort":"sort-network-priority"}
```

A round trip has two `itineraryParts`. Headers: `Authorization: Bearer
<token>` (an anonymous JWT from
`GET gol-auth-api.voegol.com.br/api/authentication/create-token`,
`issueUsing: "DC-Anonymous"`, sent with a static `X-AAT` key from the
bundle), and `x-sabre-cookie-encoded`. The backend is Sabre Digital
Connect.

Response (`execution`, `offers[]`, `selectedOffers`,
`alternateDateOffers[]`, `error`, `errorSabreResponse`):

- `offers[].itinerary`: `origin`, `destination`, `departure`,
  `arrival` (local ISO times), `stops`, `totalDuration` (minutes),
  `isInternational`, `airlineCode[]`.
- `offers[].segments[]`: `flightNumber` (integer, e.g. `1690`),
  `airlineCode` (`G3`), `operatingAirlineCode`, `equipment` (`738`).
- `offers[].fareFamily[]`: `brandId`, `fareBasis`, and `price`
  `{currency, total, fare, taxes, totalWithoutDiscount, hasDiscount}`.
  Amounts are decimals in the point-of-sale currency (`1929.65`).
- `alternateDateOffers[]`: 7 days centred on the searched date,
  `{hasValue, departure, price}` with the cheapest price per day. This
  is the date strip at the top of the page.

Brands seen: `LI` (Light), `CL` (Classic), `FL` (Flex) on every
flight, and `PR` on most GRU→AEP flights. `PR` is probably GOL's
Premium Economy (the page's i18n has a "Premium Economy" title). It
was sometimes cheaper than `LI` (GOL 7640: LI R$2,202.57, PR
R$1,788.74). The lowest price on a day is the minimum over all
`fareFamily[].price.total`.

Sample, GRU→SSA one way, 2026-11-10: cheapest R$828.65 (G3 1692 and
1356 nonstop, LI). CGH→SSA same day: R$486.04. GRU→AEP: R$1,330.04
(G3 7644, LI); 7 days from R$1,005.95 to R$2,148.59.

Clicking the strip's "Próximas datas" arrow runs a new
`flights/search` for the next day (no page load), and the strip shifts
by one day.

### `POST https://bff-flight.voegol.com.br/flightcalendar` (the calendar)

The home page's date picker shows a price under each day. When origin
and destination are set and the picker opens, it calls:

```json
{"baseSelect":{"origin":"GRU","destination":"SSA"},
 "calendar":{"month":11,"quantity":2,"year":2026},
 "currencyCode":"BRL","pax":{"numAdt":1,"numChd":0,"numInf":0}}
```

with `authorization: Bearer <anonymous token>`. The response covers
**two months per call**:

```json
{"calendar":[{"data":"2026-11-10","hasFlight":true,"value":829}, …]}
```

`value` is the day's lowest one-way price, rounded to whole BRL (829
for R$828.65; 971 for R$970.65; both match `flights/search`). Days in
the past or without flights have `value: 0`. Each "Avançar" in the
picker fetches the next pair of months (it moves one month at a time).
The request for March–April 2027 returned all 61 days priced, so six
months take 3 calls at 2 months each (6 if driven by the UI's
one-month steps).

For round trips the bundle has a `roundTripAnchored` variant: the same
body plus `selectFlight: {origin, departureDate, destination}`, giving
return-day prices for a fixed outbound. Not captured.

This is the endpoint 0042 wants: per-day one-way prices for six months
in 3–6 calls per leg.

## One way vs round trip

The round-trip search GRU→AEP 10–17 Nov returned only outbound offers
(39, all GRU→AEP), with the same prices as the one-way search for that
day. Return flights load only after an outbound fare is chosen, which
we did not do (it starts the booking flow). So GOL prices each leg
separately at this stage; whether the round-trip total has a discount
is not known (see open questions).

## Cabins

No cabin selector. Domestic flights have three economy brands
(LI/CL/FL). International flights also have `PR` (probably Premium
Economy). The i18n also mentions "Insignia Business Classic/Flex",
probably on the new long-haul routes (Lisbon, New York); not tested.

## Points of sale

`pv` sets the point of sale and the currency. `pv=ar` with
`lang=es-AR` sent `pointOfSale: "AR"` and returned ARS prices
(GOL 7644 LI: ARS 526,732.50 vs R$1,330.04 under BR; the ratio differs
slightly between fares because taxes differ). The header showed
"País AR, Moeda AR$". Whether a foreign point of sale is cheaper after
conversion was not evaluated. The calendar takes `currencyCode`.

## Bot protection

**Akamai Bot Manager**: cookies `_abck`, `bm_sz`, `bm_so`, `bm_sv`,
`bm_lso`, and sensor POSTs to an obfuscated first-party path (for
example `/fgs1JZz2q0/…` on www, `/El3uvy930S4Ve0n9YQ/…` on b2c). Also
Datadog RUM, mPulse, Qualtrics.

- **User's Chrome:** no challenge or block in 11 loads. Everything
  above worked.
- **Headless Chromium** (default `HeadlessChrome/151` user agent): the
  first navigation to the generated `b2c.voegol.com.br` URL returned
  Akamai's edge page **"Access Denied — You don't have permission to
  access … Reference #18.…"** (`errors.edgesuite.net`), before any
  JavaScript ran and with no cookies set. Per the rules, we stopped
  there and did not retry with another user agent.

## Recommendation for the daemon

Feasibility: **possible**. The data is excellent (a 2-month calendar
endpoint and clean JSON results), but the daemon's headless browser is
refused at the edge.

If the edge block can be handled within the project's rules (see open
questions), the plan is:

1. **Calendar (main series, 0042):** load `https://www.voegol.com.br/pt/`,
   then have Jev (or fixed shadow-DOM steps: trip type "Só ida ou
   volta", origin, destination, open the date input, press "Avançar")
   open the date picker. Capture `url_contains: "bff-flight.voegol.com.br/flightcalendar"`.
   About 1 page load and 3–6 calendar responses per leg for six months.
   The inputs are inside shadow roots, so `snapshot.js` must see into
   them (not verified).
2. **Confirmation / details:** a generated `busca-parceiros` URL and
   capture `url_contains: "bff-flight.voegol.com.br/flights/search"`.
   One page load per date, flight numbers and brand prices, plus 7 days
   of minimum prices from `alternateDateOffers`.

Risks: the edge block for headless; Akamai sensor scoring over repeated
use; the anonymous token's lifetime; the blob response type (the
capture must not assume text). Prices exclude any optional extras.

## Open questions

- Would headless pass with a normal Chrome user agent, or with a
  visible (non-headless) window from the daemon's profile? This changes
  how the browser presents itself, so the user should decide before
  anyone tests it.
- Does a round-trip booking (after choosing an outbound) cost less than
  the two one-way legs on international routes? This needs a look at
  the return step, which the read-only rules did not allow here.
- What exactly is brand `PR`, and where do the "Insignia Business"
  brands appear (GIG/GRU–LIS, –JFK)?
- Do city codes (`SAO`, `BUE`) work in `de`/`para` or in the calendar's
  `baseSelect`?
- Does the calendar accept `quantity` above 2?
- Is `pv=us`/`eu` cheaper than BR after IOF and spread?
