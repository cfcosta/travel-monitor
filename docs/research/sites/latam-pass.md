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
