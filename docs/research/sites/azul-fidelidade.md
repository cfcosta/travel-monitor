# Azul Fidelidade (Azul's miles program): award search

- Date: 2026-10-06
- Method: the same session as [azul.md](azul.md): `chrome-cdp` against the
  user's Chrome, a headless Chromium 151 from the dev shell, plus DNS
  lookups and public sources.
- Result: **not probed in a browser.** The award search runs on
  `www.voeazul.com.br`, which returned 403 to both browsers on their
  first load (see [azul.md](azul.md#what-happened)). No award search
  ran, nothing was captured, and we could not tell whether the user's
  Chrome is logged in. No login was attempted.

## Where the program lives

TudoAzul was renamed Azul Fidelidade. The old and obvious hostnames no
longer serve it:

| Host | Result (2026-10-06) |
| ---- | ------------------- |
| `www.azulfidelidade.com.br`, `azulfidelidade.com.br` | NXDOMAIN (the user's Chrome showed `ERR_NAME_NOT_RESOLVED`) |
| `www.tudoazul.com.br` | 302 → `https://tudoazul.voeazul.com.br/` |
| `tudoazul.voeazul.com.br` | NXDOMAIN |
| `www.voeazul.com.br/br/pt/programa-fidelidade…` | Program pages (found by search; not loaded, same blocked host) |
| `passagens.voeazul.com.br/pt/buscador-de-pontos` | Promotional points-fare finder (found by search; not loaded; same Akamai edge) |
| `compradepontos.voeazul.com.br` | Buy-points shop (irrelevant) |

So the program has no separate site to search. Award search is the
ordinary Azul booking flow in points.

## Search entry (third party, not verified)

Source: [eduard0vieira/award-tool, docs/recon/azul.md](https://github.com/eduard0vieira/award-tool/blob/main/docs/recon/azul.md)
(recon dated 2026-08-18).

The award search uses the cash deep link with `cc=PTS`:

```
https://www.voeazul.com.br/br/pt/home/selecao-voo
  ?c[0].ds=VCP&c[0].std=11/10/2026&c[0].as=SSA
  &p[0].t=ADT&p[0].c=1&p[0].cp=false&f.dl=3&f.dr=3&cc=PTS
```

According to that source it **works with no login** on a blank profile
and lands directly on the results. If so, award prices are public on
Azul, and decision 0024's login step is unnecessary here (it may still
show member or tier discounts; see below).

## Where prices come from (third party, not verified)

`POST https://b2c-api.voeazul.com.br/tudoAzulReservationAvailability/api/tudoazul/reservation/availability/v6/availability`
with an anonymous token from `/authentication/api/authentication/v1/token`,
`ocp-apim-subscription-key`, `device: novosite`, `culture: pt-BR`.
The body is the same as the cash search (`criteria[]`, `passengers[]`,
`flexibleDays`, `currencyCode: "BRL"`).

```
data.trips[]                      one per date
  .std
  .fareInformation                {lowestPoints, highestPoints}   ← the day's award minimum
  .journeys[]
    .identifier                   {carrierCode, flightNumber, std, sta, duration, connections}
    .fares[]                      available, classOfService, cabin, productClass{code,category,name}
      .paxPoints[]                amountLevel 1..5   (1 = all points; 2+ = points + cash)
        .levels[]                 points{amount, discount{applied, restriction}, discountedAmount},
                                  taxesAndFees, convenienceFee, totalMoney
    .segments[].legs[].legInfo    capacity, sold, remainingSeats (physical seats, not award seats)
```

Points to settle before storing anything (0023 stores points plus
taxes in BRL):

- Use `amountLevel` 1 (all points) as the series. Other levels mix
  points and cash.
- `points.amount` vs `points.discountedAmount`: discounts such as
  `DiscountForContactPax`, and tier discounts in the "Resgate Especial"
  programme, depend on who is logged in. Record the undiscounted
  `amount`, or record both.
- The taxes are `taxesAndFees + convenienceFee`. Store the sum and keep
  both parts.
- `cabin` was `null` on a domestic route. International cabins are
  unknown.
- Interline / partner awards (TAP and others) were sold through the old
  TudoAzul site. Whether `selecao-voo?cc=PTS` shows them is unknown.

## Calendars (third party, not verified)

There is no points calendar like Smiles' `calendarDayList`.
`flexibleDays` had no effect. `criteria` accepts up to 6 dates per
call, but only if the request body is changed, which this project will
not do. Passive capture gives about one date per direction per page
load.

## One-way, round trip, point of sale

- One-way is one `c[0]` leg. Round trip adds `c[1]`. Award prices are
  per leg, so one-ways are enough.
- Award prices are in points plus BRL taxes. No point-of-sale dimension
  is relevant.

## Bot protection

The same Akamai Bot Manager as Azul cash, on the same hosts
(`www.voeazul.com.br`, `b2c-api.voeazul.com.br`). The user's Chrome got
the per-IP "unusual activity" 403 and headless got Akamai "Access
Denied", each on its first load. The third-party test hit the per-IP
block after 8 navigations in 47 s, and it lasted more than 30 min.
Fixtures: `fixtures/azul/` (shared with [azul.md](azul.md)).

## Recommendation for the daemon

**Feasibility: poor** (for the same reasons as Azul cash).

- **Mechanism, if Azul becomes reachable:** the generated
  `selecao-voo?…&cc=PTS` URL, one leg, plus capture with
  `url_contains = "tudoazul/reservation/availability"`. Read
  `fareInformation.lowestPoints` and the level-1 `paxPoints` per
  journey. One page load gives one route × direction × day.
- **Shared budget:** cash and award searches hit the same IP limit,
  which is about 8 loads per burst. Award checks should be few and
  targeted: dates already found interesting by the cash series, a
  handful per day at most, with a hard stop at the first 403.
- **Login:** apparently not needed (unverified). If it turns out to be
  needed, the 0024 manual login in a visible window applies. Do not
  mix logged-in and anonymous readings in one series, because of the
  member and tier discounts.

## Open questions

1. Does `cc=PTS` really work anonymously? Does logging in change
   `points.amount` or only `discountedAmount`?
2. Are partner/interline awards (TAP to LIS) searchable on
   `selecao-voo`, or only Azul-operated flights?
3. Does `passagens.voeazul.com.br/pt/buscador-de-pontos` (the
   promotional points finder) expose a multi-date or multi-destination
   feed? It could act as a calendar.
4. The same as Azul cash: how long does the IP block last, and can
   headless get through at all?
