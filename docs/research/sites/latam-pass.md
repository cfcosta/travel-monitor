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
R$ 28.80 per 1,000 miles. Taxes are paid in BRL on top. For most
itineraries the award's fare before taxes plus taxes equals the cash
price of the same brand, so the award price follows the cash price.
Some itineraries do not: LA8084 LIGHT costs 145,298 miles (a fare of
R$ 4.184,57 before taxes) but R$ 6.982,80 in cash, and LA8066 lists two
award fares for one brand. So the award series cannot be fully derived
from the cash one.

The calendar calls on the award page are the cash calendar (BRL
amounts, no `redemption` parameter). There is no award calendar: an
award price costs one page load per route × direction × date.

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
