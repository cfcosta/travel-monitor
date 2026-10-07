# Buying miles: where the prices are, how often they drop, and how the daemon could track them

- Date: 2026-10-07
- Scope: buy offers for the three programs we track (LATAM Pass,
  Smiles, Azul Fidelidade), directly and through Livelo, Esfera, bank
  programs and the programs' clubs. Marketplaces that resell miles
  (HotMilhas, MaxMilhas and similar) are context only (see the end).
- Method: web search and fetches of public pages and blog posts; the
  WordPress REST API and RSS feeds of the blogs, over plain HTTP; then
  `chrome-cdp` against the user's Chrome: one new tab, a fetch/XHR hook
  installed with `Page.addScriptToEvaluateOnNewDocument` before the
  first navigation, logged-out public pages only. Nothing was clicked
  and nothing was typed. The tab was closed after the last load.
- Load count (user's Chrome, 2026-10-07 UTC): 7 page loads, at least
  20 s apart.

  | # | Time | Site | URL | Result |
  | - | ---- | ---- | --- | ------ |
  | 1 | 20:51 | LATAM Pass | `latampass.com/facilidades/compra-milhas/` | public calculator, price JSON captured |
  | 2 | 20:52 | Smiles | `smiles.com.br/comprar-milhas` | redirected to today's campaign, terms in the HTML |
  | 3 | 20:53 | Azul | `compradepontos.voeazul.com.br/` | login form only |
  | 4 | 20:53 | Livelo | `livelo.com.br/compra-de-pontos/produto/LIVCompraDePontos` | public, list price JSON captured |
  | 5 | 20:54 | Esfera | `esfera.com.vc/p/compra-de-pontos/e000100033` | public, list price in the page data |
  | 6 | 20:57 | Azul | `voeazul.com.br/br/pt/ofertas/facilidades` | public, campaign terms in AEM JSON |
  | 7 | 21:01 | Azul | `voeazul.com.br/br/pt/programa-fidelidade/comprar-pontos` | public explainer, R$ 0,07 per point |

  The blog reads were 9 WordPress REST requests and 7 RSS requests,
  outside the browser.
- Result: **every program publishes its base price and some of its
  campaigns on public pages, but none shows on one public page the
  price the user would actually pay.** Discounts depend on club plan,
  co-branded card, status tier and club tenure, and Livelo and Esfera
  apply theirs as coupons in the cart after login. The best single
  source for *when* an offer exists, and for its history, is a blog
  that posts every campaign. Passageiro de Primeira has one category
  per offer type in a public WordPress JSON API.

## 1. The channels and their prices

All prices are in BRL per 1,000 miles or points of the program named.
"Base" is the price with no promotion.

### LATAM Pass, direct purchase

- Page: `https://latampass.com/facilidades/compra-milhas/` (redirects
  to `/lp?pt=pp`). Public calculator: choose a profile (Clube + Cartão,
  Cliente Clube, Cartão LATAM Pass Itaú, Cliente LATAM Pass) and a
  quantity, "1.000 a 999.000 por ano". Buying needs a login.
- **Price JSON (public):** the page sends GraphQL
  `POST https://latampass.com/facilitiesbff`, operation
  `FreeQuotation`, variables
  `{quotation: {points: "1000", productType: "PointsPurchase", ffn: ""}}`.
  The answer has the base price (`valuePerThousandWithoutDiscount`:
  7000, in centavos), the price for anyone (`valuePerThousand`), the
  campaigns that apply to anyone (`promotions[]`: `name`, `discount`,
  `description`, `endsAt`, regulation PDF URL), and one entry per
  customer segment in `othersOptionsPromotions[]`: `segments`
  (`SE_SOCIOS_ATIVOS_CLUBE_MPLUS`, `SE_ATIVOS_COBRANDED_LATAM_MPLUS`),
  `netAmount`, `discount`, `endsAt`. Today it showed no campaign, only
  the permanent discounts: club 30% (R$ 49,00), Itaú card 30%
  (R$ 49,00), both 35% (R$ 45,50), each with `endsAt`
  `3001-01-01T02:59:59Z`. During a campaign, `promotions` should carry
  it with its end date (not seen; nothing was running).
- The logged-in page uses `Quotation` (same variables plus the member
  number) and
  `UserQuery { user(productType: "PointsPurchase") { quota { quote quoteUsed freeQuote endsAt promotionalQuota { quote quoteUsed startsAt endsAt } } } }`:
  the user's own yearly limit, what is used, and the campaign quota with
  its start and end. This came from the page bundle; it was not called.
- Base price R$ 70,00. Miles valid 3 years, credited within 48 h, up
  to 10 installments.
- Campaigns in 2026, all "até X% OFF", with the Itaú card getting the
  deepest discount (sources: Passageiro de Primeira posts, Melhores
  Destinos):

  | Dates | Best ("até", Itaú card) | Others | Limit per CPF |
  | ----- | --------- | ------ | ------------- |
  | 2026-04-07 – 04-10 | 63% (R$ 25,90) | — | — |
  | 2026-05-25 – 05-29 | 63% (R$ 25,90) | — | — |
  | 2026-06-24 – 06-30 | 60% (R$ 28,00) | — | — |
  | 2026-07-21 – 07-27 (extended) | 63% (R$ 25,90) | — | — |
  | 2026-08-25 – 08-28 | 65% (R$ 24,50) | 62% (R$ 26,60) | 350,000 |
  | 2026-09-29 – 10-02 (program's anniversary) | 65% (R$ 24,50) | 62% (R$ 26,60) | 200,000 |
  | 2025-11 (Black Friday, app only) | 66% (R$ 23,80) | 64% (R$ 25,20) | 200,000 |

  So: **about one campaign a month, 4–7 days long, usually in the last
  week of the month**, 60–66% off. Campaign discounts do not add to the
  club and card discounts.
- Bot protection on `latampass.com`: Akamai Bot Manager (`_abck`,
  `bm_sz`, `bm_sv`, `bm_mi`, sensor on an obfuscated path), reCAPTCHA
  Enterprise and ThreatMetrix loaded on the page. The logged-out page
  loaded normally in the user's Chrome.

### Smiles, direct purchase

- Page: `https://www.smiles.com.br/comprar-milhas` **redirects to the
  campaign that is running**:
  `/portal/campanhas/comprademilhas-<max bonus>-<start YYYYMMDD>`.
  Today: `comprademilhas-375-20261007`. The page is server-rendered
  (Liferay) and its regulation is in the HTML: dates and times, the
  price per mile, the bonus per club plan, limits and validity. Public;
  buying needs a login. The URL alone already says the top bonus and the
  start date. What `/comprar-milhas` shows with no campaign is unknown.
- Structure: the price is fixed, **R$ 0,08 per mile (R$ 80 per 1,000)**,
  and campaigns give bonus miles instead of a discount, only to Clube
  Smiles members and Diamante/Magno. Today's campaign (2026-10-07 10:00
  to 2026-10-16 21:00 by the regulation; the banner says 2026-10-12
  21:00): base bonus 315% plus 10% (plan 1.000) to 60% (plan 20.000,
  Diamante, Magno), so 325% to 375%. Others get no bonus. 1,000 to
  300,000 miles a year, in lots of 1,000; bought and bonus miles valid
  **12 months**; up to 5 installments, 12 with the GOL Smiles card.
- Club plans are public JSON:
  `GET https://apigw.smiles.com.br/cs/plan/plans/available/no-member?promotionChannel=CS_novabaupromo`
  (plan name, monthly cost, sign-up and promotional miles, recurrence
  bonuses). The page also reads its layout from Liferay's
  `api-cms-portal-prd.smiles.com.br/o/headless-delivery/…` (no prices
  in it).
- Campaigns in 2026 (Passageiro de Primeira, 20 posts since
  2026-03-13): bonus campaigns run **almost back to back**, two or three
  a month, usually 5–14 days: up to 355% (until 04-01), 365% (04-08),
  365% (05-04), 300% (05-13), 350% (05-19), 365% (06-19), 375%
  (07-15), 365% (07-21), 300% (08-04), 375% (08-10), 355% (08-19), 300%
  (09-02), 380% (09-14 – 09-28), a coupon for 80% off sent by e-mail
  (09-22), 300% (until 10-05), 375% (10-07). A 380% bonus is
  R$ 80 / 4.8 = **R$ 16,67**; 300% is R$ 20,00.
- Bot protection: Akamai Bot Manager sensor and mPulse. The page loaded
  normally in the user's Chrome.

### Azul Fidelidade, direct purchase

- The shop, `https://compradepontos.voeazul.com.br/`, shows only a
  login form (the old TudoAzul login). No price logged out.
- Public pages on `www.voeazul.com.br` (Adobe AEM; each page's content
  is public JSON at `<page>.model.json`):
  - `/br/pt/programa-fidelidade/comprar-pontos`: **R$ 0,07 per point**
    (R$ 70 per 1,000), at most 100,000 points in 12 months outside
    campaigns, Brazilian credit card in the buyer's CPF, validity by
    tier.
  - `/br/pt/ofertas/facilidades` (and its `facilidades.model.json`):
    the running campaigns for buying, transferring and renewing
    points, with their full terms and "Vigência" dates. Today it had
    no buy campaign; it had a 312% bonus on transfers between Azul
    accounts and 212% on renewing expired points (both 2026-10-05 to
    2026-10-12, 100,000 points per CPF, Azul Itaú card 10% off). A
    transfer between accounts is paid, so it is another way to buy;
    its fee was not found.
- Two campaign shapes: a **discount** (2026-09-23 to 09-27: Clube Azul
  70% → R$ 21,00; Topázio and above 65% → R$ 24,50; others 60% →
  R$ 28,00; Azul Itaú card another 10% → R$ 18,90; up to 2,000,000
  points in the campaign) or a **bonus by club tenure** (2026-06-03 to
  06-07: up to 340% for more than 60 months, 310% for 6–11 months →
  from R$ 70 × 0.9 / 4.4 = **R$ 14,32**).
- Campaigns in 2026: 310% (until 03-16), 340% (04-06), 340% (06-03 –
  06-07), 70% off (06-22), 317% (08-03, the program's 17th birthday),
  70% off (09-23 – 09-27). **About every one to two months.**
- Bot protection: Akamai Bot Manager on both hosts. On 2026-10-06 the
  user's Chrome got Azul's IP block on its first load (azul.md); today
  three loads went through. Decision 0052 keeps Azul off.

### Livelo (points, then transfer)

- Page: `https://www.livelo.com.br/compra-de-pontos/produto/LIVCompraDePontos`,
  public. Product JSON:
  `GET https://apis.pontoslivelo.com.br/bff-liv-points-purchase/v1/products/LIVCompraDePontos`
  → `listPrice` and `salePrice` 0.07 per point, 1,000 to 2,000,000 in
  lots of 1,000. Akamai Bot Manager, Dynatrace.
- Discounts are **club-only coupons applied in the cart** after login,
  so the public JSON shows R$ 70 during a club campaign. Discounts grow
  with quantity or with club tenure, Pix takes 2–5% more off. Example,
  until 2026-09-30: 53% up to 50,000 points, 55% to 100,000, 57% to
  400,000 → R$ 32,90 / 31,50 / 30,10, Pix R$ 29,50; 400,000 per
  campaign; 12 installments.
- Frequency: **40 posts since 2026-03-13, a campaign almost every week**,
  often for one to three days ("Só hoje"), 52–58% off (R$ 28,81 to
  R$ 33,60). Also "Turbo Livelo" (multiply club points, R$ 29,40–31)
  and club sign-up offers (from about R$ 19–23 per 1,000).
- **Pontos + dinheiro:** in a transfer to an airline, Livelo lets the
  member pay cash for all but 1% of the points, at a set price (club
  members, 2026-09: R$ 27,34–31,00 per 1,000 Livelo points, less with
  Pix). It is a purchase and a transfer in one step, and the cheapest
  way to most programs.

### Esfera (Santander; points, then transfer)

- Page: `https://www.esfera.com.vc/p/compra-de-pontos/e000100033`,
  public; its product data is in the page (`commerceData.product`):
  `listPrice`/`salePrice` 0.07, 1,000 minimum, 200,000 a month
  (2,000,000 for club members), valid 24 months. No Akamai cookies
  seen; a ClearSale device-fingerprint script loads.
- Discounts are coupons for Clube Esfera members, by quantity. Example,
  until 2026-09-30: 45% up to 199,999 points, 50% to 299,999, 52% from
  300,000 → R$ 38,50 / 35,00 / 33,60.
- Frequency: 13 posts since 2026-03-13, **about once or twice a month**,
  45–55% off.

### Transfer bonuses (Livelo, Esfera and banks to the airlines)

Transfers are 1:1 from Livelo and Esfera to all three programs (bank
programs vary). The airline gives the bonus, by its own club or tier
and sometimes the points program's club. Posts since 2026-03-13 in
Passageiro de Primeira's transfer category:

| Target | Posts | Usual bonus | Notes |
| ------ | ----- | ----------- | ----- |
| Azul Fidelidade | 100 | 100–130% for club members (80% plus 5–30% by tenure), 50% for others | one is running almost every day, from some partner |
| Smiles | 56 | 70–80% for Clube Smiles, Diamante or the points club; 40–50% for others | several a month; bonus capped (e.g. 300,000 bonus miles) |
| LATAM Pass | 50 | **20–25%**, often for all members | LATAM does not give big transfer bonuses |

### Clubs

Monthly subscriptions that credit miles every month, and are the
condition for the deep campaigns above.

- Clube LATAM Pass (public table, `web.latampass.latam.com/pt_br/clube/precos-e-planos`):
  R$ 40,90 for 1,000 a month (R$ 40,90 per 1,000) up to R$ 356,80 for
  10,000 (R$ 35,68); 30% off buying miles (35% with the Itaú card). On
  its own, never a cheap way to get miles.
- Clube Smiles: plan 1.000 at R$ 46,00 a month, R$ 496,80 a year paid
  at once, 21,000 miles in 12 months with today's sign-up bonus
  (R$ 23,66 per 1,000 on its own). Sign-up bonuses rotate weekly (43
  posts).
- Clube Azul: sign-up offers with 175–300% bonus over a few months are
  posted as "CPM a partir de R$ 10,23–13,40" (21 posts).
- Clube Livelo and Clube Esfera: needed for their purchase discounts;
  sign-up offers posted at about R$ 19–29 per 1,000.

A club's own cost per 1,000 is its fee over the miles it credits; its
real value is that it unlocks the campaign prices.

### Marketplaces (context only)

HotMilhas, MaxMilhas and similar buy miles from members and sell
tickets issued with them. They do not sell miles to the user, prices
are per ticket, and selling miles breaks the programs' rules. Not a
source.

## 2. Comparison

Best observed prices in 2026, per 1,000 miles of the target program.
"Promo floor" is the best price posted for the channel; it assumes the
best club and card.

| Program | Channel | Where the price is | Logged in? | API/JSON | Base | Typical promo | Promo floor 2026 | How often |
| ------- | ------- | ------------------ | ---------- | -------- | ---- | ------------- | ---------------- | --------- |
| LATAM Pass | direct | latampass.com calculator | no for prices; yes to buy | `facilitiesbff` GraphQL `FreeQuotation` | R$ 70 | 60–65% off | R$ 24,50 (R$ 23,80 Nov 2025) | ~monthly, 4–7 days |
| LATAM Pass | Livelo pontos + dinheiro, 25% | Livelo, logged in | yes | — | — | — | R$ 23,28–23,58 | several a month |
| LATAM Pass | club | public table | no | — | R$ 35,68–40,90 | — | — | always |
| Smiles | direct (club only) | campaign page HTML | no for terms | Liferay HTML; plans JSON | R$ 80 | 300–380% bonus | R$ 16,67 | 2–3 a month, nearly continuous |
| Smiles | Livelo/Esfera, 80% | blog; Livelo logged in | yes | — | — | — | R$ 14,72–15,54 | several a month |
| Smiles | club sign-up | plans JSON | no | `cs/plan/plans/available/no-member` | — | — | R$ 10,80 (with promo) | weekly |
| Azul | direct | voeazul AEM page JSON | no for terms; yes to buy | `facilidades.model.json` | R$ 70 | 70% off or 310–340% bonus | R$ 14,32 | every 1–2 months |
| Azul | Livelo/Esfera pontos + dinheiro, 100–130% | blog; Livelo/Esfera logged in | yes | — | — | — | R$ 11,01–12,76 | nearly always one running |
| Livelo | direct (club only) | cart coupon | yes | product JSON shows only R$ 70 | R$ 70 | 52–58% off | R$ 28,81 | weekly |
| Esfera | direct (club only) | cart coupon | yes | page data shows only R$ 70 | R$ 70 | 45–55% off | R$ 31,50 | 1–2 a month |

Melhores Destinos' "target prices" for 2026 (July update, from
January–July's best channels): **Azul R$ 13, LATAM Pass R$ 26, Smiles
R$ 16** (2025: 14, 25, 16).

## 3. Effective cost per 1,000 miles

Let `B` be the base price per 1,000 (BRL), `d` a discount, `c` a card
discount on top, `b` a bonus, and `t` a transfer bonus.

- **Direct with a discount** (LATAM Pass, Azul, Livelo, Esfera):
  `p = B × (1 − d) × (1 − c)`.
  LATAM Pass 2026-09-29, not an Itaú client: 70 × 0.38 = **R$ 26,60**.
  Azul 2026-09-23, Clube Azul with the Azul Itaú card:
  70 × 0.30 × 0.90 = **R$ 18,90**.
- **Direct with a bonus** (Smiles, sometimes Azul): you pay for `q`
  miles and receive `q × (1 + b)`, so `p = B × (1 − c) / (1 + b)`.
  Smiles today, plan 1.000: 80 / 4.25 = **R$ 18,82**; plan 20.000:
  80 / 4.75 = **R$ 16,84**. Azul June, 340%, Itaú card:
  70 × 0.9 / 4.4 = **R$ 14,32**.
- **Through a points program**: buy points at `p_pts` per 1,000, then
  transfer at ratio `r` (1 for Livelo and Esfera) with bonus `t`:
  `p = p_pts / (r × (1 + t))`.
  Livelo at 57% with Pix (R$ 29,50) into Smiles at 80%:
  29.50 / 1.8 = **R$ 16,39**; into LATAM Pass at 25%:
  29.50 / 1.25 = **R$ 23,60**; into Azul at 110%:
  29.50 / 2.1 = **R$ 14,05**.
- **Pontos + dinheiro**: the same formula, with `p_pts` the transfer's
  cash price. Livelo → Smiles, 2026-09-17, 100,000 points:
  R$ 2.797,07 for 180,000 miles = **R$ 15,54**. Livelo → Azul,
  2026-09-23, 80% + 30% tenure: R$ 2.734,70 for 210,000 points =
  **R$ 12,76**. Livelo → LATAM Pass, 2026-09-21, 25%:
  R$ 2.947,55 for 125,000 = **R$ 23,58**.
- Caps make the price depend on quantity: tiers by quantity (Livelo,
  Esfera), a cap on bonus miles (Smiles transfers, Azul 300,000 per
  CPF) and yearly limits per CPF. A price holds only up to the
  campaign's cap.

### Against the value of a mile (0082, 0083)

For a LATAM Pass award at the standard rate, miles = fare before taxes
× 34.72, so the miles cost, at a buy price `p`, fare × p / 28.80. Paid
with bought miles, an award costs

`bought = miles × p / 1000 + award taxes`

and it saves `fare × (1 − p / 28.80) + (cash taxes − award taxes)`
against cash. At the September campaign (R$ 26,60) that is **7.6% of
the fare**; at the Livelo route (R$ 23,58) 18%. An award with a route
discount (×0.92) is worth R$ 31,30 per 1,000, so R$ 26,60 saves 15%.
**For LATAM Pass, the break-even buy price is the award's measured
value of a mile**, R$ 28,80 at the standard rate. Smiles and Azul have
no measured value yet (no award reader runs).

## 4. Seasonality

From the dates above (2026-03-13 to 2026-10-07) and the 2025 posts:

- LATAM Pass: one direct campaign a month, mostly in the last week;
  the deepest at the end of September (the program's anniversary) and
  on Black Friday (66% in November 2025).
- Smiles: some bonus campaign almost all the time; the top level
  (375–380%) came in July, August, September and October.
- Azul: campaigns around the program's birthday (early August) and
  every one or two months otherwise.
- Livelo and Esfera: weekly or monthly flash discounts; transfer bonuses
  peak around Black Friday (100% or more expected) and the blogs'
  anniversaries ("PP 15 anos", July 2026).
- Transfer bonuses to Azul run nearly every day from one partner or
  another; to Smiles several times a month; to LATAM Pass at 20–25%
  several times a month.

## 5. Blogs as the source of offers

| Blog | Feed | Structure | Notes |
| ---- | ---- | --------- | ----- |
| Passageiro de Primeira | `https://passageirodeprimeira.com/feed/` (RSS, 30 items) and the WordPress REST API | categories per offer type: **Compra de pontos (id 3905, 137 posts)**, **Transferência de pontos (3904, 247)**, **Clube de pontos (3906, 189)**, all since 2026-03-13 | `GET /wp-json/wp/v2/posts?categories=3905&per_page=100&_fields=id,date,link,title`; `X-WP-Total` gives the count; `content.rendered` has the full post with its tables |
| Pontos pra Voar | `https://pontospravoar.com/feed/` (RSS, 20 items) | each item tagged with program names (Livelo, Azul Fidelidade, …) | WordPress REST answers; no buy category found |
| Melhores Destinos | `https://www.melhoresdestinos.com.br/feed` (RSS) | — | `/milhas/feed` returned old items out of order; keeps the yearly "valor do milheiro" post |
| Melhores Cartões, Alta Renda | RSS | mixed with shopping deals | — |

Post titles carry most of what is needed: program, kind ("compra",
"transferências", "Clube"), the percentage, often "milheiro a partir
de R$ X", and the status ("Só hoje", "Últimas horas", "Último dia",
"Prorrogado", "Segue valendo"). A title is a lead to be confirmed, not
a price.

## 6. Recommendation for the daemon

Track in two layers.

1. **Detection and history from the blog (no browser).** Every 2 hours,
   one plain HTTP GET per Passageiro de Primeira category (3 requests,
   about 36 a day), from the newest post back to the last one seen. A
   new post about LATAM Pass, Smiles, Azul, Livelo or Esfera is parsed
   from its title, and from `content.rendered` (one more request) for
   the tables: kind, channel, bonus or discount, prices by tier and
   audience, dates. Pontos pra Voar's RSS is the backup. This is its
   own site in the budget (0060), for example `feed:passageirodeprimeira`
   with 100 requests a day. The 7 months already published are the
   history to start from (fixtures).
2. **Confirmation from the program's own public page**, logged out,
   once a day and again when the blog reports a campaign:
   - **LATAM Pass**: one page load of
     `latampass.com/facilidades/compra-milhas/`, reading the page's
     own `facilitiesbff` `FreeQuotation` answer (not calling it: 0082
     found that LATAM refuses calls the page did not make). It gives
     the base price, the segment prices and a campaign's `discount`
     and `endsAt`. It is an Akamai site of the LATAM group, so it
     belongs in the shared LATAM profile and the offscreen compositor
     (0081, 0084), counted in `miles:latam_pass`'s budget: 1–2 of its
     40 loads a day. Whether the daemon's browser is served this page
     needs the usual probe (0050).
   - **Smiles**: one page load of `smiles.com.br/comprar-milhas`; the
     redirect URL names the top bonus and start date, and the
     regulation in the HTML gives the rest. The Smiles site is not
     enabled and needs its probe first (0050).
   - **Azul**: stays off (0052). If it is enabled one day,
     `facilidades.model.json` is the page to read. Until then the blog
     is the only source.
   - **Livelo and Esfera**: their public pages show only the R$ 70 list
     price; the club coupons are not public. Blog only. A weekly check
     of the product JSON would catch a change of the base price.

No logged-in read is needed to find offers. Logged in, LATAM Pass's
`Quotation` and `user.quota` would give the user's own price and
remaining yearly limit; that touches the real account (latam-pass.md
§6, risks) and should wait for the user's consent.

Costs: about 40 HTTP requests a day to the blog, and 1–2 page loads a
day each for LATAM Pass and Smiles once they are enabled. Promotions
last 1 to 14 days, so a daily check misses none; "Só hoje" flashes are
caught by the 2-hour blog poll.

## 7. A proposed data model

One row per price, per audience, per offer. A campaign with three
audiences (Itaú card, club, others) is three rows of one offer.

```sql
CREATE TABLE buy_offers (
  id              INTEGER PRIMARY KEY,
  offer_key       TEXT NOT NULL,   -- groups the rows of one campaign
  target_program  TEXT NOT NULL,   -- latam_pass | smiles | azul_fidelidade
  channel         TEXT NOT NULL,   -- direct | transfer | points_plus_cash | club_signup | coupon
  source_program  TEXT,            -- livelo | esfera | itau | c6 | …; NULL when direct
  audience        TEXT NOT NULL,   -- all | club | club_plan:20000 | cobranded_card | tier:diamante | club_tenure_months>=60 | …
  base_minor      INTEGER,         -- base price per 1,000 source units, BRL minor units
  discount_bp     INTEGER,         -- basis points off the base (6200 = 62%)
  card_discount_bp INTEGER,        -- extra off with the co-branded card
  bonus_bp        INTEGER,         -- purchase or transfer bonus (37500 = 375%)
  ratio           REAL NOT NULL DEFAULT 1.0, -- target units per source unit before the bonus
  price_minor     INTEGER NOT NULL, -- effective BRL per 1,000 target miles, derived from the above
  min_qty         INTEGER,         -- per purchase
  max_qty         INTEGER,         -- per purchase or per campaign
  cap_per_cpf     INTEGER,         -- and the period it applies to:
  cap_period      TEXT,            -- campaign | month | year
  tier_min_qty    INTEGER,         -- quantity band this price applies to (Livelo/Esfera tiers)
  tier_max_qty    INTEGER,
  miles_valid_months INTEGER,      -- 12 for Smiles; 36 for LATAM Pass
  valid_from      TEXT,            -- UTC
  valid_until     TEXT,            -- UTC; NULL for a permanent price
  source          TEXT NOT NULL,   -- blog:passageirodeprimeira | page:latam_pass_bff | page:smiles_campaign | …
  source_url      TEXT NOT NULL,
  title           TEXT,
  confirmed       INTEGER NOT NULL DEFAULT 0, -- 1 once the program's own page shows it
  first_seen_at   TEXT NOT NULL,
  observed_at     TEXT NOT NULL    -- last time a source showed it
);
```

The user's side goes in `miles_programs` and a new table for points
programs: club plan and tenure, co-branded card, status tier, and
which of Livelo, Esfera and the banks they use. The user's price for an
offer is the cheapest row whose audience they match. The permanent
prices (LATAM club 30%, Smiles R$ 80, Azul R$ 70) are rows with no
`valid_until`, so "no campaign" still has a price.

## 8. Judging a good time to buy

For each target program, take the cheapest current price the user is
eligible for, `p`, and check three things:

1. **Against its own history**: below the lowest 10% of the program's
   prices for that user over the last 365 days (any channel), or the
   lowest of the year so far. The blog history gives a year from day
   one; the Melhores Destinos target prices (Azul 13, LATAM 26, Smiles
   16) are a sanity check.
2. **Against the value of a mile** (0083): the program's usual rate,
   the 30-day median of its award checks. `p` below it means an award
   paid with bought miles usually costs less than its cash fare; the
   saving is `1 − p / value`.
3. **Against watched trips**: for each current award observation with
   its cash price, `miles × p / 1000 + award taxes` against the cash
   total. List the trips where buying saves at least the deal margin,
   and only if the miles needed fit under the offer's cap and are used
   before they expire (12 months at Smiles).

A notification is worth sending only for 3 (a watched trip saves money
now) or for a record low in 1. Everything else is a line in the app.
The user's existing balance is unknown to the daemon, so the app should
say "if you bought all the miles".

## Open questions

1. Which clubs, co-branded cards and tiers does the user have (Clube
   LATAM Pass, LATAM Pass Itaú, Clube Smiles plan, GOL Smiles card,
   Clube Azul and tenure, Azul Itaú, Clube Livelo, Clube Esfera)? They
   decide which row is the user's price.
2. Which points and bank programs does the user use (Livelo, Esfera,
   Itaú, Nubank, C6, Inter, BB)? Only those are worth tracking.
3. Is a third-party blog acceptable as a source of offers, confirmed
   where possible by the program's page?
4. Does `FreeQuotation` show a running campaign in `promotions`, with
   `endsAt`? Check during the next LATAM campaign (likely the last week
   of October or Black Friday).
5. What does `/comprar-milhas` at Smiles show between campaigns?
6. Are `latampass.com` and the Smiles campaign page served to the
   daemon's browser (headless, or on the offscreen compositor)? Not
   tried (0050 probe).
7. Is the LATAM Pass login on `www.latamairlines.com` also a session on
   `latampass.com` (for `user.quota`)?
8. What does Azul charge for a transfer between accounts? With a 312%
   bonus it may be the cheapest way to buy Azul points.

## Fixtures

| File | Source |
| ---- | ------ |
| `fixtures/latam-pass/buy-freequotation-1000-2026-10-07.json` | Load 1: `facilitiesbff` `FreeQuotation` answer, with the request shape from the bundle |
| `fixtures/smiles/buy-campaign-comprademilhas-375-20261007.txt` | Load 2: the campaign page's text and regulation |
| `fixtures/smiles/club-plans-no-member-2026-10-07.json` | Load 2: `cs/plan/plans/available/no-member` |
| `fixtures/azul/buy-pages-2026-10-07.txt` | Loads 3, 6, 7: the login wall, the campaign terms from `facilidades.model.json`, the buy-points FAQ |
| `fixtures/livelo/buy-product-2026-10-07.json` | Load 4: `bff-liv-points-purchase` product |
| `fixtures/esfera/buy-product-2026-10-07.json` | Load 5: the product's price and limit fields from the page data |
| `fixtures/miles-buying/passageiro-de-primeira-compra-de-pontos-2026-10-07.json` | Category 3905, 137 posts (date, title, link) |
| `fixtures/miles-buying/passageiro-de-primeira-transferencia-de-pontos-2026-10-07.json` | Category 3904, 247 posts |
| `fixtures/miles-buying/passageiro-de-primeira-clube-de-pontos-2026-10-07.json` | Category 3906, 189 posts |

No fixture holds cookies, a session or personal data; every page was
logged out. The Smiles page also loads a public feature-flag
configuration (`ldrelay-green.smiles.com.br`) that contains client
credentials; it was not saved.

## Sources

- Melhores Destinos: [valor do milheiro 2026](https://www.melhoresdestinos.com.br/milhas/valor-milheiro-azul-latam-smiles-2026),
  [LATAM Pass 65% (Sep 2026)](https://www.melhoresdestinos.com.br/milhas/latam-pass-milhas-65-off-set26),
  [LATAM Pass 65% (Aug 2026)](https://www.melhoresdestinos.com.br/milhas/latam-pass-milhas-65-off-ago26),
  [LATAM Pass Black Friday 66% (Nov 2025)](https://www.melhoresdestinos.com.br/milhas/latam-pass-compra-milhas-66-nov25).
- Passageiro de Primeira: [Azul 70%](https://passageirodeprimeira.com/azul-fidelidade-oferece-ate-70-de-desconto-na-compra-de-pontos-5/),
  [Livelo 57%](https://passageirodeprimeira.com/livelo-oferece-ate-57-de-desconto-na-compra-de-pontos-milheiro-a-partir-de-r-2950/),
  [Esfera 52%](https://passageirodeprimeira.com/esfera-oferece-ate-52-de-desconto-na-compra-de-pontos-para-assinantes-do-clube/),
  [Livelo → LATAM Pass pontos + dinheiro](https://passageirodeprimeira.com/milheiro-latam-a-partir-r-2358-transfira-pontos-livelo-latam-pass-usando-pontos-dinheiro/),
  [Livelo → Smiles pontos + dinheiro](https://passageirodeprimeira.com/gere-o-milheiro-smiles-a-partir-de-r-1548-transferindo-pontos-livelo-dinheiro/),
  [Livelo → Azul pontos + dinheiro](https://passageirodeprimeira.com/transfira-pontos-livelo-dinheiro-para-a-azul-milheiro-a-partir-de-r-1276/).
- Melhores Cartões: [Smiles 380%](https://www.melhorescartoes.com.br/smiles-compra-milhas-380-bonus-set26.html).
- Alta Renda: [LATAM Pass anniversary 65%](https://altarendablog.com.br/2026/09/29/%F0%9F%8E%89-aniversario-latam-pass-compre-milhas-com-ate-65-off-veja-o-custo-do-milheiro/).
- Smiles: [campaign 380% regulation](https://www.smiles.com.br/portal/campanhas/comprademilhas-380-20260914).
- LATAM Pass: [Clube prices and plans](https://web.latampass.latam.com/pt_br/clube/precos-e-planos).
