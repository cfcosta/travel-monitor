# LATAM Pass (award search on latamairlines.com): login wall, same engine as cash

- Date: 2026-10-06
- Method: `chrome-cdp` against the user's Chrome (one tab, XHR/fetch
  hook installed before navigating). The cash-fare research for the
  same site is in [latam.md](latam.md); read it first.
- Result: **the user's Chrome is not logged in to LATAM** (the header
  shows "Entrar na minha conta LATAM / Fazer login"). An award search
  URL redirects to the login page, so no award search ran and no award
  price was captured. Following the rules, nothing was typed into the
  login form.
- Load count: 1 page load (the award URL). Headless was not tried for
  LATAM Pass: the same site already returns Akamai "Access Denied" to
  headless on the first request (latam.md §5).

## 1. Search entry and results URL

LATAM Pass award search is not a separate site. It is the same
`oferta-voos` results page as cash fares, with `redemption=true`:

```
https://www.latamairlines.com/br/pt/oferta-voos
  ?origin=GRU&destination=LIS
  &outbound=2026-11-10T12%3A00%3A00.000Z&inbound=null
  &adt=1&chd=0&inf=0&trip=OW&cabin=Economy
  &redemption=true
  &sort=RECOMMENDED
```

Logged out, the URL above redirected straight to the LATAM login page:

```
https://auth.latamairlines.com/u/login/identifier?state=<opaque>&ui_locales=pt
```

The page asks for "Email, CPF ou Número de cliente". The path
(`/u/login/identifier`, `state=`) is an Auth0 Universal Login. The
login domain runs its own Akamai Bot Manager sensor (a POST to an
obfuscated path, plus mPulse). No `/bff/air-offers` call was made
before the redirect. Nothing about award prices is public.

Whether a generated `redemption=true` URL works for a logged-in
session, with no form step, is **not verified**. The cash URL works
that way, so it probably does.

## 2. Where award prices probably come from (not verified)

The results app sends `redemption=false` as a query parameter on both
of its price calls (latam.md §2):

- `GET /bff/air-offers/v2/offers/search?…&redemption=false&…`
- the round-trip second call with `outOfferId=…`

So a logged-in award search most likely calls the same
`offers/search` with `redemption=true`, and returns the same structure
with miles (and taxes in BRL) in place of the cash price. The field
names for miles are unknown. The `extended=true` calendar call has no
`redemption` parameter in the cash flow. Whether the award flow has a
calendar is unknown.

Capture target, once logged in: `url_contains =
"/bff/air-offers/v2/offers/search"`, filtered to URLs containing
`redemption=true`.

## 3. Calendars

Unknown, needs a logged-in session. In the cash flow, the calendar
(`/bff/air-offers/v2/calendar`) returns 2 months of daily lowest
economy prices per call. If the award flow has a miles equivalent, it
would cover six months in 3 calls per direction.

## 4. One way, round trip, cabins, points of sale

Expected to be the same as cash (`trip=OW|RT`, `cabin=Economy|Business`,
`adt/chd/inf`). LATAM Pass accounts belong to a country, and the
Brazilian program prices in LATAM Pass miles plus BRL taxes. Searching
under a foreign point of sale (`/us/en`) with a Brazilian account was
not tested and is probably not meaningful for awards.

## 5. Bot protection

The same as the cash site, plus the login domain:

- `www.latamairlines.com`: Akamai Bot Manager (`_abck`, `bm_sz`,
  `ak_bmsc`, `bm_s*`), Akamai waiting room (`akavpau_CyberLatam`), and
  reCAPTCHA plus a signed search token on `offers/search`.
- `auth.latamairlines.com`: Auth0 login behind its own Akamai sensor.
- User's Chrome: the redirect to login happened normally, with no
  challenge.
- Headless Chromium: not tried for this program. The cash site's first
  request was refused ("Access Denied") in headless, so this one would
  be too.

## 6. Recommendation for the daemon

**Feasibility: poor for now** (blocked by two separate problems, not by
the data):

1. The daemon's headless Chromium is refused by Akamai at the edge
   (latam.md §5). Until that is solved for the cash site, award search
   cannot run either.
2. It needs a logged-in session. Decision 0024 covers this: the user
   logs in once by hand in the daemon's profile, in a visible window.
   The login is Auth0 with an Akamai sensor and possibly 2FA or email
   codes.

If both are solved, the integration is likely cheap: the same generated
URL as cash, with `redemption=true`, and the same capture on
`/bff/air-offers/v2/offers/search`. 1 page load per route × direction ×
date, plus the extra clicks for round trips (latam.md §2). The parser
could share the cash parser's types once the miles fields are known.

Risks:

- Session lifetime is unknown. If it is short, the app will ask the
  user to log in often (0024).
- Award searches by a logged-in account are tied to that account. A
  bot flag here could affect the user's real LATAM Pass account, not
  only an anonymous session. Keep award checks rare.

## Open questions

1. Next step: with the user logged in to LATAM Pass in their own
   Chrome, run one award search (GRU→LIS, GRU→MIA) and capture
   `offers/search?…redemption=true`. That gives the miles fields, a
   fixture, and whether the calendar has an award version.
2. Does a generated `redemption=true` URL load results directly when
   logged in, or does it need the form?
3. How long does a LATAM login session last in a profile that is not
   used every day?
4. Does the award search show several award brands per flight, as the
   cash search does, and does it include partner flights (such as
   Delta codeshares)?
5. Is the headless block on the user agent or on the TLS fingerprint?
   (Shared with latam.md.)

## Fixtures (`fixtures/latam-pass/`)

| File | Source |
| ---- | ------ |
| `login-redirect.txt` | The award URL, where it ended (`state` removed), the login page text, and the calls the hook saw |

No price fixture: no award search ran.

## Probe with a logged-in profile (2026-10-06)

- Method: the daemon was paused with an `open_login` request for
  `latam_pass` (request 5, done 17:56:37 UTC, "Chromium stopped").
  Then one headless Chromium 151 ran on the daemon's profile
  (`~/.local/share/travel-monitor/chromium`), with the daemon's own
  flags (`--headless=new`, `--window-size=1440,1000`,
  `--password-store=basic`, `--use-mock-keychain`, and the others in
  `tm-browser`'s `ChromiumConfig::args`). It used the default user
  agent (`HeadlessChrome/151.0.0.0`) and nothing to hide automation.
  `chrome-cdp` added an XHR/fetch hook before the navigation. The
  Chromium was closed and the daemon resumed with `close_login`
  (request 6, done 17:57:39 UTC, "Chromium is up"). The pause took
  62 seconds.
- Load count: 1 page load (the award URL in §1, GRU→LIS one way
  2026-11-10, `redemption=true`). No retry.

### Result: Akamai denied the first request

- Final URL: the award URL itself, with no redirect
  (`redirectCount: 0`). The HTTP status was **403**.
- Page: Akamai **"Access Denied"**, in Spanish, English and Portuguese,
  `Reference Code: #0.88421502.1791309413.1cb737c`. It is the same page
  that the cash site returns to headless (latam.md §5).
- No JavaScript from LATAM ran. The hook saw 0 calls. No
  `/bff/air-offers` request was made, so **no award response was
  captured**. The miles fields are still unknown.
- No login wall was shown. The edge refused the request before the app
  could check for a session.

The denial is the same as for a fresh profile. A LATAM Pass session in
the profile does not change Akamai's decision at the edge. This
supports decision 0052: LATAM Pass stays off while the daemon uses a
headless browser that it does not disguise.

### The LATAM session was no longer in the profile

The profile's history shows that the app's login window completed a
login at 17:52–17:53 UTC. The visits were `auth.latamairlines.com/u/login/identifier`,
then `/u/login/password`, `/u/mfa-login-options`, `/u/mfa-sms-challenge`
(an SMS code), `/authorize/resume`, `www.latamairlines.com/callback`, and
then `/pt-br/myaccount/`. But when the probe started, the default
context had **1 cookie** in total (`akavpau_latamairlines_BBB`, set by
the denied request). `Default/Cookies` had **0 rows** after the daemon
restarted. The session was already gone before the probe, because the
daemon had relaunched on this profile at 17:54:01. Two possible causes,
not verified:

1. The login window (`tm-app` `login.rs`) starts Chromium **without**
   `--password-store=basic --use-mock-keychain`. In this case Chromium
   encrypts cookies with a key from the desktop keyring (`Local State`
   `os_crypt` has a `portal` entry). The daemon's Chromium uses the
   basic store, so it cannot decrypt those cookies and drops them.
2. LATAM's session cookies have no expiry date, so they ended when the
   login window closed.

Either way, a manual login does not reach the daemon's headless
browser at present. Use the same `--password-store` flags in the login
window and in the daemon before trying LATAM Pass, Smiles or Azul
again. That alone does not help LATAM: the edge denial above comes
first.

### Fixtures

| File | Source |
| ---- | ------ |
| `headless-logged-in-access-denied.html` | The Akamai page that the award URL returned (403) |
| `headless-logged-in-access-denied.png` | Screenshot of that page, 1440×1000 |

No award fixture: no award search ran.

## Headed Chromium with a warmed, logged-in profile (2026-10-07)

Question: does a headed Chromium get `offers/search` where the Xvfb
probe (latam.md, "Headed Chromium in a virtual display") got 403, if
its profile is a real, lasting one that the user warmed up by hand?

Setup:

- Chromium 151.0.7922.71, the binary the daemon runs, with the
  daemon's flags (`--remote-debugging-port=9341`, `--window-size=1440,1000`,
  `--no-first-run`, `--no-default-browser-check`,
  `--disable-features=Translate,MediaRouter`, `--password-store=basic`,
  `--use-mock-keychain`, `--disable-sync`), no `--headless`, plus
  `--ozone-platform=wayland`. The window was on the user's own
  Wayland desktop, with the GPU. The CDP port was open the whole time.
- Profile: `~/.local/share/travel-monitor/chromium/miles-latam_pass`,
  the daemon's per-site profile for `miles:latam_pass` (0062). It was
  new. The user logged in to LATAM Pass in it (with the SMS code),
  accepted cookies, and ran a few searches by hand.
- No user-agent override, no stealth flags, no extensions, no
  synthetic input. The stock user agent (`X11; Linux x86_64 …
  Chrome/151.0.0.0`).
- `chrome-cdp` opened a new tab, installed an XHR/fetch hook with
  `Page.addScriptToEvaluateOnNewDocument`, and navigated. No clicks.
- Load count: 2 page loads. The GRU→LIS one-way 2026-11-10 cash URL
  (§1 of latam.md, 18:16:30 UTC), then the same URL with
  `redemption=true` (18:17:30 UTC).

### Result: both searches return prices

- **Cash:** `GET /bff/air-offers/v2/offers/search?…&redemption=false`
  returned **200**, 29 itineraries. The cheapest was LA8146 nonstop,
  LIGHT R$ 1.907,43 (STANDARD 2.136,04, FULL 2.414,35). Both calendar
  calls returned 200 as before.
- **Award:** the results page called a different path,
  **`GET /bff/air-offers/v2/offers/search/redemption?…&redemption=true`**,
  which returned **200**, 17 itineraries. There was no login wall.
- No challenge, no error page, no Akamai denial.

So the stricter check on `offers/search` passes for a headed browser
with a warmed profile and the CDP port open. What changed from the
Xvfb probe was the display (GPU-backed Wayland, not Xvfb's software
rendering) and the profile (warmed and logged in, not new). One load
of each cannot tell which of the two mattered.

### Award response

The same shape as the cash response (`content[].summary.brands[]`).
Per brand:

```json
"price":           { "currency": "LOYALTY_POINTS", "amount": 63848, "display": "63.848 milhas" },
"priceWithOutTax": { "currency": "BRL", "amount": 1838.82 },
"taxes":           { "currency": "BRL", "amount": 68.61 },
"seatsRemaining":  { "count": 9, "lowAvailability": false },
"offerId":         "…loyalty"
```

`content[].newPrices[]` repeats `{ fare, taxes, total }` per brand
(miles, BRL, miles). Economy and business brands come in the same
response ("PREMIUM BUSINESS STANDARD/FULL").

**Miles are the fare before taxes × 34.72**, on every brand of every
itinerary in this response (LIGHT 63,848 = R$ 1.838,82 × 34.72;
PREMIUM BUSINESS FULL 328,214 = R$ 9.452,55 × 34.72). That is about
R$ 28.80 per 1,000 miles. Taxes are paid in BRL on top. For every
award itinerary in this sample, the award's fare before taxes plus
taxes equals the cash price of the same brand on the same itinerary.

(Corrected on 2026-10-07. This paragraph first said that LA8084 LIGHT
cost 145,298 miles but R$ 6.982,80 in cash, and that LA8066 listed two
award fares for one brand. Both came from matching itineraries on the
first flight's number and departure time only. LA8084 and LA8066 each
start several itineraries with different connections. Matched on every
segment, the award LA8084/BA512 costs R$ 4.348,81 in cash
(R$ 4.184,57 + R$ 164,24 taxes), and LA8066's two prices belong to two
itineraries (via LA1587 and via LA1670). The R$ 6.982,80 itinerary is
LA8084/TP1357, which is not in the award results. See "Measuring cash
against miles" below for where the award price does differ from cash.)

The calendar calls on the award page are the cash calendar (BRL
amounts, no `redemption` parameter). There is no award calendar: an
award price costs one search per route × direction × date (one page
load, or one date-carousel click on a loaded page; see below).

### What this means for the daemon

- Headless stays refused at the edge. A headed Chromium works, so
  LATAM needs a display. The daemon runs as a user service with no
  window; the plan is an offscreen Wayland compositor on the GPU
  render node, run by the daemon itself (as moonshine does), with
  Chromium on `--ozone-platform=wayland`.
- The profile matters as much as the display: it must be the
  lasting, logged-in `miles-latam_pass` profile, not a new one.
- Award checks are expensive (no calendar) and run under the user's
  real account: keep them rare and targeted, for example the dates
  where the cash price is already low.

### Fixtures

| File | Source |
| ---- | ------ |
| `../latam/offers-search-ow-gru-lis-2026-11-10-br-headed-warm.json` | The cash `offers/search` response from this probe |
| `offers-search-redemption-ow-gru-lis-2026-11-10-br.json` | The award `offers/search/redemption` response |

The session IDs in `extraDetails` were replaced with `<removed>`. No
screenshot was kept: the page header shows the account holder's name.

## Measuring cash against miles (2026-10-07)

Question: how does a LATAM Pass award price relate to the cash price,
and what is the cheapest way for the daemon to find a deal in either?

Setup: the same headed Chromium 151 and logged-in `miles-latam_pass`
profile as the section above, still running on the user's desktop.
`chrome-cdp` opened its own tab and installed a hook with
`Page.addScriptToEvaluateOnNewDocument` that recorded each BFF call's
URL, status, request header names and body. Loads were 20–45 s apart.
Nothing was booked, no seat was chosen, nothing was typed.

Load count: 12 searches, no block, no challenge, no login wall.

| # | UTC | What |
| - | --- | ---- |
| 1 | 18:35:56 | Award RT GRU–MIA 2027-03-04/11, page load (outbound) |
| 2 | 18:37:54 | Click: LA8198 LIGHT on the outbound, which loads the award return leg |
| 3 | 18:38:50 | Cash RT GRU–MIA, same dates, page load (outbound) |
| 4 | 18:40:08 | Award OW GRU→MIA 2027-03-04 |
| 5 | 18:40:50 | Award OW CGH→SDU 2026-10-21 (domestic, two weeks out) |
| 6 | 18:42:00 | Cash OW CGH→SDU 2026-10-21 |
| 7 | 18:44:39 | Award OW GRU→LIS 2027-06-15 (eight months out) |
| 8 | 18:45:59 | `fetch` from the page: `calendar … extended=true&redemption=true` |
| 9 | 18:46:23 | `fetch` from the page: `offers/search/redemption` GRU→LIS 2027-06-16 |
| 10 | 18:47:01 | `fetch` from the page: the page's own search (2027-06-15) again |
| 11 | 18:47:38 | Click on 16/06 in the date carousel of the award page |
| 12 | 18:48:26 | Award OW MIA→GRU 2027-03-11 |

Every itinerary below is matched on all of its segments (flight number
and departure of each), never on the first flight alone.

### 1. Miles are the award fare × 34.72, with route discounts

Across 8 award responses (1,120 brands, every cabin), the miles price
is the brand's own `priceWithOutTax` (BRL) divided by R$ 0,0288,
rounded to a whole mile:

| Rate (miles per R$ of fare) | Brands | Where |
| --------------------------- | ------ | ----- |
| 34.72 (R$ 28,80 per 1,000 miles) | 1,042 | every response; every J and W brand; partner flights |
| 32.98 (34.72 × 0.95) | 69 | CGH→SDU, all Y brands (LIGHT, STANDARD, FULL) of all 23 flights |
| 31.95 (34.72 × 0.92) | 9 | GRU→MIA RT outbound, Y brands of the three LATAM nonstops (LA8198, LA8194, LA8190) |

- The rate held on near and far dates (October 2026 to June 2027),
  long-haul and domestic, one way and round trip, economy, premium
  economy and business.
- The two discounts are not marked anywhere in the response:
  `promotionalText.show` is false, `messages` is empty, no
  `discountPercent`. Premium economy (CGH→SDU) and business (GRU→MIA)
  on the same flights stayed at 34.72. The same nonstops priced one way
  (load 4) and on the return leg (load 2) were at 34.72. They are not
  explained by an elite tier or a co-branded card, since neither applies
  to this account. So whether another account sees the same discount is
  unknown.
- Partner and codeshare flights are priced at 34.72 too: Delta
  (GRU–MIA), Iberia, British Airways and Lufthansa connections
  (GRU–LIS). Connections on TAP, Air France, Air Europa (`UX`) and
  Vueling appear in the cash results but never in the award results.

### 2. The award fare equals the cash fare of the same fare basis

When the award and cash results offer the same fare basis on the same
itinerary, the BRL fare before taxes is identical to the cent:

| Search | Matched brands | Same fare | Different fare |
| ------ | -------------- | --------- | -------------- |
| GRU→LIS OW 2026-11-10 (both from the previous section) | 80 | 80 | 0 |
| GRU–MIA RT outbound 2027-03-04 (loads 1 and 3) | 186 | 177 | 9 |
| CGH→SDU OW 2026-10-21 (loads 5 and 6) | 92 | 80 | 12 |

The differences come from **award inventory**: the award search offers
only some booking classes.

- CGH→SDU LA3902: the cash LIGHT fare is R$ 904,90 (fare basis
  `NLEX0H1`, the cheapest seat of the day). The award search offers
  only `XJEX0N1` on that flight, R$ 1.230,90, so 40,597 miles. The cash
  fare × 32.98 would have been about 29,850 miles. On the other 22
  flights the cheapest bucket was `XJEX0N1` in both.
- GRU–MIA Delta connections: the award offers `VLX…` fares where cash
  offers `LLX…`. Some come out cheaper in miles than the cash fare ×
  34.72 (LA6346/LA6183 STANDARD 151,165 miles; cash fare R$ 4.775,97),
  others dearer.

Taxes differ on international round trips: R$ 160,98 on the award
outbound against R$ 277,27 in cash, and R$ 27,83 on the award return.
One-way GRU→LIS (R$ 68,61) and domestic CGH→SDU (R$ 62,14) charge the
same taxes in both.

The cheapest award price of a search followed the cheapest cash fare
on long-haul: GRU–MIA RT outbound, 67,644 miles = R$ 1.948,16 × 34.72,
the cheapest cash fare of the same itinerary (LA8212/LA4400). On
CGH→SDU it did not (inventory, above).

So the cash price predicts the award price closely, **but not
always**: an award can cost more than the cash fare implies (a closed
bucket) or less (a discount, or a cheaper partner bucket). Only an award
search shows which.

### 3. Round trips

`redemption=true` works with `trip=RT`. It is the same two-call flow as
cash (latam.md §2), both on `offers/search/redemption`: the first call
has `outOfferId=null`, the second (after choosing an outbound brand)
`outOfferId=<offerId ending in "loyalty">` and
`outFlightDate=<arrival>`. Round-trip fare bases end in `/XP`.

GRU–MIA, 2027-03-04/11, LATAM nonstops, LIGHT:

| | Outbound | Return | Total |
| - | -------- | ------ | ----- |
| Round trip | 69,069 (LA8198) | 75,065 (LA8199) | **144,134 miles** + R$ 188,81 |
| Two one-ways | 139,258 (load 4) | 85,418 (load 12) | 224,676 miles + R$ 188,81 |

The award round trip is 36% cheaper than the two one-ways, for the
same reason as in cash (the round-trip fare is split over its legs).
An award round trip is not the sum of one-ways; it needs its own
search and one click.

### 4. No award calendar; one page can search several dates

- `GET /bff/air-offers/v2/calendar` ignores `redemption=true`: the
  response was byte-identical to the page's own call without it (BRL
  amounts). The date carousel on the award page shows those cash
  amounts (R$ 5.749,09 per day around 2027-06-15).
- The results app's code (its `_next` bundles, read from the browser
  cache; no extra load) builds the calendar URL from `origin`,
  `destination`, `month`, `year`, `isRoundTrip` and `extended` only.
  The search box's calendar, `/bff/web-products-searchbox/v1/calendar`,
  takes the same parameters. Neither has an award variant.
- Other award paths in the bundle: `offers/search/redemption/promo/<code>`
  (used when the URL carries `promocode`; the response then carries a
  `discountPercent`; error codes `LGE12` and `LGE14` say a promotion
  can require a LATAM Pass tier or a co-branded card),
  `offers/search/multicity-redemption`, and `offers/redemption-options`
  (a POST after choosing flights: miles-plus-money splits). None gives
  several dates. Promo codes were not tested.
- **The date carousel re-searches without a page load.** Clicking
  another day (load 11) made the app fetch
  `/pt-br/flights/_next/data/<build>/pt-br/flights.json?…` (Next.js
  page data for the new date) and then `offers/search/redemption` for
  that day, which returned 200 with 13 itineraries. The URL changed by
  client-side routing; the document was not reloaded. The carousel
  shows seven days around the searched date.

### 5. Calling the BFF directly does not work

- The calendar answers a `fetch` from the page with only the plain
  `X-latam-*` headers (as in latam.md §3).
- `offers/search/redemption` called with `fetch` from a loaded award
  page, with every header the page sends (the `X-latam-*` set,
  `x-latam-device-width`, `x-latam-search-token` from sessionStorage,
  and a fresh `x-latam-captcha-token` from
  `grecaptcha.enterprise.execute(<site key>, {action: "SEARCH_OFFERS"})`,
  the action the app uses), returned **200 with no itineraries**
  (`totalElements: 0`) twice: for 2027-06-16, and for 2027-06-15, the
  page's own search, which had just returned 17. A refusal that looks
  like an empty result.
- Where the tokens come from: the Next.js page data
  (`pageProps.tokensToSearch`) carries a new `searchToken` (HS512 JWT,
  20 minutes, bound to origin and destination), `nextAuth` and
  `searchSession` for each search. The app can also ask
  `/bff/air-offers/v2/give-me-search-token` with `x-latam-next-auth`,
  `x-latam-search-session`, `x-latam-control`, `x-latam-control-bot`
  and `x-latam-csrf-token`. Reproducing that chain from outside the
  app is reproducing its bot protection, which 0052 rules out. We
  stopped after two empty answers.

So a direct API call is not a way to save page loads. A carousel click
is: it is the app's own search, triggered by the same input a person
uses.

### 6. Where the session lives

Cookies on LATAM's domains after the login (names, domains and expiry
only; `fixtures/latam-pass/session-cookies.json`):

| Cookie | Domain | Lifetime | What it is |
| ------ | ------ | -------- | ---------- |
| `_xp_session` | `www.latamairlines.com` (host-only), httpOnly, secure | 5 days, **renewed by every page load** (18:17:38 → 10-12 18:17:38; after load 12, 10-12 18:48:36) | the web session |
| `auth0`, `auth0_compat` | `auth.latamairlines.com`, httpOnly | 3 days from the login (18:15:06 → 10-10 18:15:06), not renewed by `www` loads | Auth0 SSO session |
| `did`, `did_compat` | `auth.latamairlines.com`, httpOnly | 1 year | Auth0 device id, set by any visit to the login page |
| `fpid-af`, `ps-af`, `pt-af` | `auth.latamairlines.com`, path `/u/login` | session | login-page anti-fraud |
| `thx_guid`, `tmx_guid` | `loyaltyprogram.latam.com`, httpOnly | 13 months | ThreatMetrix device ids |
| `_xp_show_login_incentive` | `www.latamairlines.com` | 1 day | set at 18:14:46, **before** the login |
| `_xp_metadata_br_pt`, `_xp_exp_id` | `www…`, `.latamairlines.com` | 7 days, 13 months | point of sale, A/B experiment |
| `_abck`, `bm_*`, `ak_bmsc`, `akavpau_*`, `AKA_A2`, `RT` | `.latamairlines.com`, `www…`, `.auth…` | hours to 1 year | Akamai Bot Manager, waiting room, mPulse |
| `_ga*`, `_gcl_au`, `__gads`, `__gpi`, `__eoi`, `fs_uid`, `fs_lua`, `kampyle*`, `__exponea_*` | `.latamairlines.com`, `www…` | up to 13 months | analytics, ads, survey, CDP |

- localStorage holds no token (`_grecaptcha`, consent, analytics).
  sessionStorage holds the per-search JWTs above
  (`latam-airline-search-token`, `-search-session-token`,
  `-next-auth-token`), which name origin and destination, not the
  user. The page learns who is logged in from the server
  (`pageProps.user`), so the session is the cookies, mainly
  `_xp_session`.
- Whether `_xp_session` keeps the login alive after `auth0` expires
  (day 3) was not tested. A daemon that loads a LATAM page at least
  every few days keeps `_xp_session` renewed.
- **The 0064 heuristic reads any LATAM visit as a login.** A
  logged-out visit leaves persistent cookies that are not on its
  tracker list (`_xp_show_login_incentive`, `_xp_metadata_br_pt`,
  `_xp_exp_id`, `kampyle_userid`, `fs_uid`, `__eoi`, `__gads`, `did`,
  `thx_guid`). For LATAM Pass the check must look for the session
  cookie itself: `_xp_session` on `www.latamairlines.com`, unexpired.
  `_xp_session` may also exist for a logged-out visitor (not tested: we
  did not log out); the first award search is the real proof, since a
  logged-out award URL redirects to `auth.latamairlines.com` (§1).

### What this means for the daemon

- **Cash** (`airline:LA`): as latam.md §6. One OW page load per route,
  direction and date gives every cabin; the cash calendar ranks dates.
- **Miles** (`miles:latam_pass`) cannot be derived from cash, but cash
  tells where to look. A cash check already gives the expected award
  price, (cash fare before taxes) × 34.72. Award searches are worth
  spending on:
  1. the dates whose cash fare is low for the series (where an award
     deal would be, if the bucket is open), and
  2. a rotating spot check of other dates, to catch the silent
     discounts (×0.95, ×0.92) and partner buckets that cash does not
     show.
- **Cost:** one award page load per route and direction, plus up to
  six carousel clicks for the neighbouring days on the same page, each
  click one search. A round trip adds one click (choose the outbound)
  per date. Space clicks like loads.
- **Deal for an award series:** miles against the series' own
  baseline (0023 keeps award and cash apart), as for cash. Each award
  observation can also carry its effective rate, miles ÷ (award fare
  in BRL), against the standard 34.72, and the cash fare of the same
  itinerary when one was read: below 34.72 is a discount, above it a
  closed bucket. Taxes are in BRL and sometimes lower than cash.
- **Login check:** `_xp_session` on `www.latamairlines.com`, not the
  generic heuristic.

### Fixtures

| File | Source |
| ---- | ------ |
| `offers-search-redemption-rt-gru-mia-2027-03-04-outbound-br.json` | Load 1, first 13 of 50 itineraries plus LA6346/LA6183 (Delta) |
| `offers-search-redemption-rt-gru-mia-2027-03-11-inbound-br.json` | Load 2, first 8 of 45 |
| `../latam/offers-search-rt-gru-mia-2027-03-04-outbound-br-2026-10-07.json` | Load 3 (cash), first 13 of 50 plus LA6346/LA6183 |
| `offers-search-redemption-ow-gru-mia-2027-03-04-br.json` | Load 4, first 8 of 50 |
| `offers-search-redemption-ow-cgh-sdu-2026-10-21-br.json` | Load 5, all 23 |
| `../latam/offers-search-ow-cgh-sdu-2026-10-21-br.json` | Load 6 (cash), all 23 |
| `offers-search-redemption-ow-gru-lis-2027-06-15-br.json` | Load 7, all 15 |
| `direct-fetch-from-page.json` | Loads 8–10: URLs, header names, statuses and bodies |
| `offers-search-redemption-ow-gru-lis-2027-06-16-br-carousel.json` | Load 11, first 6 of 13 |
| `offers-search-redemption-ow-mia-gru-2027-03-11-br.json` | Load 12, first 8 of 50 |
| `session-cookies.json` | Cookie names, domains, flags and expiry on LATAM's domains (no values) |

`extraDetails` (session and track ids) is `<removed>` in every
response. The fixtures were checked for the account's name, email,
LATAM Pass number and balance; none is in them. No screenshot was kept.
