# American Airlines (aa.com, AA): search URL and bot protection

- Date: 2026-10-06
- Method: `chrome-cdp` against the user's Chrome (one tab, XHR/fetch hook
  installed before navigation, 2 page loads), then a headless Chromium
  151 from the dev shell (`$TM_CHROMIUM`, `--headless=new`, fresh
  profile, 1 page load). The user's Chrome was not logged in to
  AAdvantage. No login, no award search.
- Test route: GRU→MIA, one way, 2026-11-10, Brazil point of sale
  (`locale=pt_BR`).
- Result: **no prices captured.** The user's Chrome got a CAPTCHA on the
  first results page; the headless browser got "Access Denied" on the
  home page. We stopped at each, as agreed.
- Fixtures: [fixtures/american-airlines/](fixtures/american-airlines/README.md)
  (the search form's fields and the challenge page; no price response).

## Search entry

The home page `https://www.aa.com/homePage.do?locale=pt_BR` loaded
normally in the user's Chrome, in Portuguese. Its search form
`#reservationFlightSearchForm` posts to
`https://www.aa.com/booking/find-flights` with:

| Field | Values |
| ----- | ------ |
| `tripType` | `roundTrip`, `oneWay` |
| `redeemMiles` | checkbox "Resgatar milhas" (award search) |
| `originAirport`, `destinationAirport` | airport or city codes |
| `adultOrSeniorPassengerCount` | 1–9 |
| `departDate`, `returnDate` | `dd/mm/aaaa` in pt_BR |
| `serviceclass` | `coach`, `business`, `first` (no premium economy option) |

A generated results URL was accepted (it was the second load):

```
https://www.aa.com/booking/search?locale=pt_BR&pax=1&adult=1&child=0&type=OneWay&searchType=Revenue&cabin=&carriers=ALL&travelType=personal&slices=[{"orig":"GRU","origNearby":false,"dest":"MIA","destNearby":false,"date":"2026-11-10"}]
```

(`slices` URL-encoded.) The server redirected it to
`https://www.aa.com/booking/choose-flights/1?sid=<uuid>&locale=pt_BR`,
which is the results route, so the deep link itself works. `type` would
be `RoundTrip` with two slices, `searchType=Award` for miles, and `cabin`
one of the cabin codes; none of these were tested.

## What happened

| Browser | Page | Result |
| ------- | ---- | ------ |
| User's Chrome | home, `locale=pt_BR` | Loaded. Akamai cookies set (`_abck`, `bm_sz`, `bm_sc`, `bm_so`, `bm_lso`, `aka_*`), two sensor POSTs (201) to an obfuscated path `/cRTTo5BR1/…`. |
| User's Chrome | generated search URL → `/booking/choose-flights/1?sid=…` | **CAPTCHA.** Title "Challenge Validation", "Please confirm you're a human", an "I'm not a robot" checkbox in an iframe (`/challenge-assets/v7/captcha.html`) and a disabled Continue button. New cookie `sec_cpt`. It did not resolve by itself after 20 s. No price request was made. |
| Headless Chromium | home, `locale=pt_BR` | **Blocked.** "Access Denied — You don't have permission to access …", reference `#18.…` on `errors.edgesuite.net` (Akamai edge). |

We did not click the checkbox, retry, or change the user agent.

## Bot protection

Akamai Bot Manager with its `sec_cpt` challenge (the AA-branded "I'm not
a robot" page), plus an outright edge denial for the headless browser.
This is stricter than United, which uses the same vendor but let the
user's Chrome through: AA challenged a real, long-lived Chrome profile
on its first search.

## Where prices come from (not observed)

Not captured, because the challenge came before the results app loaded.
Public descriptions of aa.com say the results page is a single-page app
that posts the search as JSON to an endpoint under
`/booking/api/search/…` and that AA offers a flexible-dates (monthly
calendar) view; we could not confirm either, the request shape, or a
calendar endpoint.

## Calendars, cabins, points of sale

- Calendar: not seen (see above).
- One way and round trip: both in the form and in the URL's `type`.
- Cabins: `coach`, `business`, `first` in the form.
- Point of sale: `locale=pt_BR` gave the Portuguese site; whether it
  also sets the currency (BRL) could not be checked without results.

## Recommendation for the daemon

**Feasibility: poor.** The URL can be generated, but a real Chrome got a
CAPTCHA on its first search and the headless browser is denied on the
home page. Nothing the daemon may do (no CAPTCHA solving, no evasion)
gets past that. AA's own fares on GRU–MIA also appear on Google Flights,
which is the main source (0011).

If AA is ever retried: one page load per search
(`/booking/search?…` → `choose-flights`), capture by
`url_contains = "/booking/api/"`, with the risk that every check is
challenged.

## Open questions

- Does the challenge come every time, or only for a new search session?
  Would a profile that the user once passed by hand stay clear, and for
  how long?
- Which request carries prices (`/booking/api/search/itinerary`?), and
  is there a calendar endpoint with many days per call?
- Does `locale=pt_BR` price in BRL, and which parameter sets the point
  of sale?
- Is AA worth keeping in 0012 given this, or should it be covered by
  Google Flights only?
