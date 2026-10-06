# Azul (voeazul.com.br, AD): blocked at the edge

- Date: 2026-10-06
- Method: `chrome-cdp` against the user's Chrome (one tab, XHR/fetch hook
  installed before navigating) and a headless Chromium 151 from the dev
  shell (`--headless=new`, fresh profile, no user-agent override, the
  same flags as the daemon). Planned tests: VCP→LIS and CGH/VCP→SSA.
- Result: **both browsers were blocked on their first page load**. No
  search ran, no price request was captured, and no price fixture
  exists. Following the research rules, nothing was retried.
- Sections marked **(third party, not verified)** come from public
  reverse-engineering notes on GitHub, cited inline. They match what we
  saw where the two overlap (the block page, the Akamai hosts), but we
  did not confirm them.

## What happened

| Browser | Request | Result |
| ------- | ------- | ------ |
| User's Chrome | `GET /br/pt/home`, 1st load | **403**, custom page "Oops! Just a moment. We detected unusual activity from your IP. Access is temporarily limited." with the client IP and `Ref: 0.cf031502.…` |
| Headless Chromium | `GET /br/pt/home`, 1st load, about 3 min later | **403**, stock Akamai "Access Denied … Reference #18.66c1002.…" with an `errors.edgesuite.net` link |
| WebFetch (data-centre IP) | `GET /br/pt/home/selecao-voo` | 403 |

The user's Chrome was blocked before Azul's app loaded. So we could
not see whether that profile is logged in to Azul Fidelidade, and the
hook recorded no XHR/fetch calls.

The two block pages are different:

- **The IP block** (user's Chrome) is Azul's own page, served by
  Akamai with assets from `cybersecurity.voeazul.com.br`. It names the
  IP, and the third-party notes below say it lasts more than 30 minutes.
  We do not know what flagged the IP. It may be earlier traffic from
  this network (including an earlier run of this research) or the
  IP's reputation.
- **Access Denied** (headless) is Akamai's default edge deny. The
  headless browser sends `HeadlessChrome/151` in its User-Agent, and
  Akamai probably rejects that on sight. The IP block may also apply.
  We cannot tell the two causes apart from one load.

Fixtures (scrubbed, IP replaced with 203.0.113.1):

- `fixtures/azul/block-ip-limited-chrome.txt`: the IP-block page
  (truncated) and the cookie names.
- `fixtures/azul/block-access-denied-headless.html`: the Akamai deny
  page, complete.

## Bot protection: Akamai Bot Manager

- DNS: `www`, `b2c-api`, `passagens` and `cybersecurity.voeazul.com.br`
  all CNAME to `voeazul.com.br.edgekey.net` → `e192958.dscb.akamaiedge.net`.
- Cookies set even on the block page: `_abck`, `bm_sz`, `bm_so`,
  `bm_lso` (Akamai Bot Manager), plus `ardsp` and `ATAB` (`=A` / `=B`,
  probably an A/B test bucket).
- The block page loads the Bot Manager sensor script from an obfuscated
  first-party path (`/j69L/uGKq/O/ktc/…?v=<uuid>`).
- No CAPTCHA was offered. The block is a plain 403 with no challenge.

## Search entry (third party, not verified)

Source: [eduard0vieira/award-tool, docs/recon/azul.md](https://github.com/eduard0vieira/award-tool/blob/main/docs/recon/azul.md)
(recon dated 2026-08-18) and
[DavyCosta701/TCC, azul_scraper](https://github.com/DavyCosta701/TCC/tree/main/azul_scraper).

The results page can be opened from a URL, with no form:

```
https://www.voeazul.com.br/br/pt/home/selecao-voo
  ?c[0].ds=VCP&c[0].std=11/10/2026&c[0].as=LIS
  &c[1].ds=LIS&c[1].std=11/17/2026&c[1].as=VCP      (return leg; omit for one-way)
  &p[0].t=ADT&p[0].c=1&p[0].cp=false
  &f.dl=3&f.dr=3
  &cc=BRL                                            (PTS = Azul Fidelidade points)
```

- `c[i]` is one leg: `ds` departure station, `as` arrival station,
  `std` date as **M/D/YYYY**. One-way is a single leg; round trip is two.
- `p[i]` is a passenger type: `t=ADT` and `c` the count; `cp` is the
  companion pass.
- `f.dl` / `f.dr` are the flexible days to the left and right of the
  date.
- `cc` is the currency: `BRL` for cash, `PTS` for points.
- Cabin: no parameter is known. Azul sells economy, "Espaço Azul"
  (extra legroom, a fare/seat product) and business ("Azul Business")
  on widebodies. In the third-party captures, fares carry `cabin` and
  `productClass {code, category, name}`. `cabin` was `null` on a
  domestic route.
- Point of sale: the path holds country and language (`/br/pt/`);
  `/us/en/` is likely. Not tested.

We did not test our generated URL (`VCP→LIS 11/10/2026`, one-way) because
the site was blocked.

## Where prices come from (third party, not verified)

The SPA calls Azul's B2C API on `b2c-api.voeazul.com.br`, which sits
behind Azure API Management and Akamai:

1. `POST /authentication/api/authentication/v1/token`: anonymous session
   token, made by the page itself (no login).
2. Cash: `POST /reservationavailability/api/reservation/availability/v6/availability`.
   Points: `POST /tudoAzulReservationAvailability/api/tudoazul/reservation/availability/v6/availability`.
   Older code uses v4/v5, so the version changes over time.

Headers set by the app: `authorization` (the token from step 1),
`ocp-apim-subscription-key` (a public gateway key from the site's JS),
`device: novosite`, `culture: pt-BR`.

Request body:

```json
{"criteria":[{"departureStation":"VCP","arrivalStation":"REC",
              "std":"10/17/2026","departureDate":"2026-10-17"}],
 "passengers":[{"type":"ADT","count":"1","companionPass":false}],
 "flexibleDays":{"daysToLeft":"3","daysToRight":"3"},
 "currencyCode":"BRL"}
```

Response (points version, the cash one is analogous):

```
data.trips[]                      one per criteria entry (date)
  .std
  .fareInformation                {lowestAmount | lowestPoints, highest…}  ← the day's minimum
  .journeys[]
    .identifier                   {carrierCode, flightNumber, std, sta, duration, connections}
    .fares[]                      available, classOfService, cabin, productClass{code,category,name}
      (cash: fare amounts; points: paxPoints[].levels[] with points, taxesAndFees, convenienceFee)
    .segments[].legs[].legInfo    capacity, sold, remainingSeats
```

`data.trips[k].fareInformation.lowestAmount` is the per-day cash
minimum that the TCC scraper reads.

**Direct calls are refused.** The award-tool notes report that `fetch`
or XHR from inside the page, from an iframe, and from outside the
browser all failed: the browser reports a CORS error, but the real
response is Akamai's 403 HTML. Only the request the SPA builds itself
got through. Changing that request's body in flight (more dates) worked,
but that is request tampering, close to getting around the bot
protection, and this project should not do it. The daemon's passive
model (load the page, read the response) is unaffected.

## Calendars (third party, not verified)

- No low-fare calendar endpoint is known. `flexibleDays ±3` changed
  nothing in the award-tool test (identical bytes for ±3 and 0).
- `criteria` takes **up to 6 dates per call** (7 or more returns
  400 `GetTripAvailabilityRequestFailed`). The page itself sends one
  date per leg, so passive capture gives about one day per direction
  per page load.
- The home page and `passagens.voeazul.com.br` have promotional
  "buscador" fare finders. Not examined.

## Rate limit (third party, matches our block page)

The award-tool test: 8 navigations in 47 s with a clean profile; the 9th
got the same "unusual activity from your IP" 403 as our first load.
The block was per IP (clearing cookies did not help) and still active
after more than 30 minutes. Whether a slower pace avoids it is not known.

## Recommendation for the daemon

**Feasibility: poor** (at present, from this network).

- **Mechanism, if it opens up:** a generated `selecao-voo` URL (one
  leg, `cc=BRL`) plus capture with
  `url_contains = "reservation/availability/v"`, reading
  `data.trips[0].fareInformation.lowestAmount` for the day's minimum
  and `journeys[].fares[]` for flight-level detail. This captures the
  cash endpoint only, because the points endpoint path has
  `tudoazul/reservation/availability`. That is 1 page load per route ×
  direction × day, so six months of both legs is about 360 loads per
  route. That is far above the measured budget of about 8 loads per IP
  burst.
- **Blocking issues:**
  1. Headless Chromium gets Akamai "Access Denied" on its first load.
     The daemon runs headless (0021), so as things stand it cannot reach
     the site at all. Masking the headless User-Agent or fingerprint
     would be evading bot protection, so it is out of scope unless
     decided otherwise.
  2. The IP block hits the whole site, lasts more than 30 min, and is
     shared with the user's own browsing. A daemon that trips it locks
     the user out of voeazul.com.br as well.
- **What would make it "possible":** a headed (non-headless) daemon
  window, or a site that lets headless through. Then a very low
  frequency, such as a handful of confirmation loads per day on watched
  dates only, with Google Flights as the calendar source (Azul's own
  fares do appear on Google Flights), and a hard stop at the first 403.
  Treat Azul as a **confirmation source**, not a calendar source.

## Open questions

1. Is the IP block still active, and for how long? Recheck once from
   the user's Chrome after 24 h. If the first load still gets 403, the
   IP has a lasting bad reputation.
2. Does headless get "Access Denied" because of the UA, or because of
   the IP? Only a test from an unflagged IP can tell.
3. Does the SPA run a new search (new date) without a full navigation,
   for example with the result page's date strip? If so, one page load
   could yield several days.
4. Is there a cabin parameter in the URL or the body for Azul Business
   (VCP→LIS is widebody)? What do `cabin` / `productClass` hold on an
   international route?
5. Does Azul's price differ from what Google Flights shows for AD
   flights? If it never does, Azul adds little beyond confirmation.
6. Do `/us/en/` or other country paths change the price or currency?
