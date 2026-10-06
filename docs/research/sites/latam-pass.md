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
