# Scheduling price checks: what the literature says

- Date: 2026-10-05
- Scope: how a daemon with a fixed daily budget of page loads should
  decide what to check next, so it notices price drops quickly. Browser
  handling is out of scope.

Four searches ran in parallel: crawl revisit scheduling, fare-cache
refresh in the travel industry, airfare price dynamics, and budgeted
monitoring (restless bandits, Age of Information). Every arXiv ID below
was fetched to confirm that it exists. Papers that are not on arXiv are
cited by venue.

## The short version

The four areas reach the same design on their own:

1. **Score each possible check by the value it is expected to find, per
   unit of cost, then run the checks with the highest score until the
   budget is spent.** The value grows with the time since the last check
   and with how often that source changes. The crawl literature, the
   Age-of-Information Whittle indices and the Amadeus and Agoda fare
   caches all use this greedy pattern.
2. **Do not chase the most volatile entries.** Under a fixed budget, the
   best policy checks entries that change faster than we can follow
   *less*, not more (Cho & Garcia-Molina; Amadeus patent US 9,582,536).
3. **Learn each source's change rate from "did it change since the last
   check?"** (one bit per check, with an irregular gap between checks).
   Use a maximum-likelihood estimate, not a raw count of changes.
4. **Treat cheap signals as hints.** The daily Explore sweep tells us
   that a destination *may* have moved. Each hint is counted as extra
   elapsed time, weighted by how often past hints were right
   (Busa-Fekete et al. 2025).
5. **Airfares change about once a day, and less often far from
   departure.** Daily checks capture most of the signal. More than about
   four checks a day per route and date is wasted.
6. **Confirm before alerting.** A single query often shows a short-lived
   blip.

## Airfare dynamics (what we are sampling)

| Finding | Source |
| --- | --- |
| Fares changed up to 7×/day, but 63% of changes re-priced *all* of an airline's flights on a route at once (about 1–2×/day). Sampling every 3 hours captured almost all of it. | Etzioni et al., "To Buy or Not to Buy" (Hamlet), KDD 2003 |
| About 10 changes per itinerary over 60 days. The median day-over-day change is $0. Jumps at the 21/14/7/3-day advance-purchase cutoffs. More drops on routes with competition. | Williams, Econometrica 2022 (NBER w28989) |
| Fares roughly flat from 90 to 21 days out, then up more than 70%. | Betancourt et al. 2022 (NBER w30347) |
| International routes: fares barely move until about 100 days out. 25% of fares end more than 25% *below* the first quote. Premium cabins rise late. | Aryal, Murry, Williams, REStud 2024, arXiv:2102.05751 |
| Time-of-day effect is about 0.65–1.3%. Not worth scheduling around. | Escobari, Rupp, Meskey 2019 |
| Weekend purchases are about 5% cheaper, or show more spread at the same mean. A small effect. | Puller & Taylor 2012; Mantin & Koo 2010 |
| About 22% of cached fares (90 days × stays of 1–15 days) change from one day to the next. Nearby entries change together. | Vanchinathan et al. (Amadeus), KDD 2015, arXiv:1506.00935 |
| Brazil: departures on the eve of a holiday cost 12.1% more (GRU/CGH data). | Póvoa & Oliveira 2013, arXiv:2402.07124 |
| Brazil: an incumbent's pricing over time flattens after a low-cost carrier enters. | Varella, Frazão, Oliveira, TR-E 2017 |
| Brazil: seasonal patterns changed after the pandemic. | Oliveira 2024, arXiv:2402.13789 |
| No price discrimination by cookie or profile across 25 airlines and 130k queries. Differences between countries came from taxes and currency (for example, Argentina's 20% card tax). Flights flickered in and out of results about 1 minute apart. | Vissers et al., "Crying Wolf?", HotPETs 2014 |

Gaps: there is no peer-reviewed measurement of how long a deal lasts. A
rate of about 6–10 changes per itinerary over 60–90 days suggests most
low fares last a day or more. Flash sales and mistake fares are only
documented by industry sources. There is no academic work on how award
(miles) prices move, so award prices are a separate series with their
own baseline.

## Scheduling policies

### Crawl revisit scheduling

- **Cho & Garcia-Molina, ACM TOIT and TODS 2003.** Change-rate estimator
  from changed/unchanged bits:
  `r̂ = −ln((n − X + 0.5)/(n + 0.5)) / I`. Here `n` is the number of
  checks, `X` the number that saw a change, and `I` the interval between
  checks. Also shows that sources changing too fast should get *fewer*
  checks.
- **Azar, Horvitz, Lubetzky, Peres, Shahaf, PNAS 2018.** Optimal rates in
  O(n log n). The value of checking source *i* after time τ is
  `V = (μᵢ/Δᵢ)·(1 − e^{−Δᵢτ}(1 + Δᵢτ))`, where μ is importance and Δ the
  change rate.
- **Kolobov, Lubetzky, Peres, Horvitz, SIGIR 2019.** Optimal rates under
  a per-host rate limit (one limit for Google Flights, one per airline,
  one per miles program).
- **Kolobov, Peres, Lu, Horvitz, NeurIPS 2019 (LambdaCrawl).** Penalty =
  H(number of missed changes), so no source is ever starved. Fits Δ by
  maximum likelihood over the gaps between checks:
  `Σ_changed a/(e^{aΔ} − 1) = Σ_unchanged a`.
- **Upadhyay et al., "Learning to Crawl", AAAI 2020, arXiv:1905.12781.**
  Explore, then commit, for unknown change rates. Supports a short
  uniform start-up phase.
- **Avrachenkov, Patil, Thoppe, arXiv:2004.02167 and arXiv:2009.08142.**
  Online change-rate estimators that update in O(1) per check.
- **Busa-Fekete et al. (Google) 2025, arXiv:2502.02430.** Greedy: each
  slot, check the source with the highest `V(τ_eff)`. Noisy
  change-hints add effective time: `τ_eff = τ + β·n_hints`,
  `β = −ln(ν/γ)/α`, where ν/γ is the share of false hints and α the rate
  of changes no hint reports. Keeps the check rate constant and adapts
  when the budget changes.
- **Mahmoody, Kornaropoulos, Upfal 2015, arXiv:1509.02487.** When the
  goal is detecting *events* quickly, not freshness, check each source
  in proportion to **√(event rate)**.
- **Avrachenkov & Borkar, arXiv:1503.08558.** A Whittle index for
  content that loses value as it ages (relevant to short-lived fare
  sales).
- **Olston & Pandey, WWW 2008.** Do not chase changes that revert before
  anyone looks.

### Fare caches in the travel industry

- **Vanchinathan et al. (Amadeus), arXiv:1506.00935.** GP-Select:
  predicts with a Gaussian process which cached fares have changed and
  refreshes those first. Features: date, origin, destination, days to
  departure, stay length, price.
- **Zhang et al. (Agoda), arXiv:2008.02087, "PriceAggregator".** The
  closest ready-made design.
  - SmartTTL learns a refresh interval per group of searches (grouped by
    the features that drive price changes).
  - Searches are ranked by expected value per request, and the top M
    that fit the daily budget are spread evenly in time.
  - +10–30% bookings in A/B tests.
- **Amadeus, US 9,582,536 B2.**
  - Validity model: `P(still valid) = a·e^{−λt}`, where `a` is how
    accurate the price was when computed and `λ` how fast that entry
    changes.
  - Ranking only by "probability changed × value" spends too much on
    very volatile entries. Fix: add a term for how often an entry gets
    refreshed.
- **Amadeus, US 9,235,620 B2.** Priority from accuracy × popularity ×
  age, with the age term preventing starvation. When related entries
  re-price together, their priority is raised.
- **Kayak, US 8,874,492 B2.**
  - Classifies cached fares as trusted / validate / re-price / live
    query.
  - Features: days to departure, weekday, time of day, carrier, stops,
    fare class, trip length, the route's historical price stability.
  - Outcomes include "no longer findable", kept separate from "price
    changed".
- **ITA (Google Flights), US 2004/0249798 A1.** An offline table of
  P(cached result is wrong | age), with a threshold to trigger a
  refresh.

### Budgeted monitoring (bandits and Age of Information)

- **Tripathi & Modiano 2019, arXiv:1908.10438.**
  - Whittle index for any age penalty f:
    `W(h) = h·f(h+1) − Σ_{j≤h} f(j)`.
  - With `f(h) = 1 − e^{−qh}` (P(a deal has appeared and is still
    unseen)), it becomes the Azar value above.
- **Kriouile & Assaad 2021, arXiv:2102.03245.** Age of *incorrect*
  information: the penalty applies only while our view is wrong.
- **Liu & Zhao 2008, arXiv:0810.4658.** Two-state arm ("deal present"
  or not) that we only see when checked. A closed-form index.
- **Dance & Silander 2017, arXiv:1703.10010.** Observing a time series
  when its posterior variance passes a threshold is optimal. Covers a
  cheap noisy query next to an expensive precise one.
- **Koley & Singh 2024, arXiv:2404.12468.** Whittle index with a cost
  per check (supports dividing the score by query cost).
- **Cohen & Zhao 2014, arXiv:1403.1023; Gopalan et al. 2021,
  arXiv:2107.10492; Kartzman et al. 2026, arXiv:2601.22561.** Quickest
  detection of a change across many streams: keep checking the stream
  that looks most suspicious, with a small, decaying random share.
- **Kandasamy et al. 2016, arXiv:1610.09726.** Multi-fidelity bandit:
  cheap checks rule out arms, and expensive checks go to promising ones.
- **Jung & Tewari 2019, arXiv:1905.12673.** Thompson sampling for
  restless bandits: sample parameters, then run the index policy.
- **Kolobov, Bubeck, Zimmert 2020, arXiv:2002.12014.** A fallback for
  sources that do not change like a Poisson process. Too complex to
  start with.
- **Niño-Mora 2026, arXiv:2601.13045.** A survey of restless bandits.

## Proposed scheduler

**The unit of work.** One *query* is one page load we pay for: one
Google Flights date grid, one airline search, one miles search or one
Explore sweep. A query covers many *cells* (route × cabin × departure
date × source). A query's value is the sum of the value of the cells it
covers.

**State per query family** (route × cabin × source):

- `last_check`: when it was last checked.
- `chg_a, chg_b`: a Gamma posterior on the change rate Δ. Increment
  `chg_a` when the minimum price over the window moved by more than 5%.
  Add the elapsed time to `chg_b`.
- `deal_a, deal_b`: a Beta posterior on P(a change is a drop). Pooled
  across routes as the prior.
- `n_hints`: Explore signals for this destination since the last check.
- `prec_a, prec_b`: a Beta posterior on how often Explore hints are
  right. Global prior.
- `w`: user weight. `c`: expected cost (page loads, seconds, failure
  rate).

**Every tick:**

```
Δ   ~ Gamma(chg_a, chg_b)            // Thompson sample, or the mean
q   = max(Δ · deal_a/(deal_a+deal_b), ε)   // rate at which deals appear
p   = prec_a / (prec_a + prec_b)
τ   = now − last_check
τe  = τ + n_hints · (−ln(1 − p)) / q
x   = q · τe
score = w · (1 − e^{−x}(1 + x)) / (q · c)
```

Run the highest-scoring queries until this tick's share of the daily
budget is spent, with these guards:

- **Per-site rate limit.** Google Flights, each airline and each miles
  program has its own limit.
- **A minimum interval between checks** of the same family, so very
  volatile entries are not chased (Amadeus, Cho).
- **A maximum interval** (`τ_max`). A family not checked within `τ_max`
  is checked regardless of its score, so nothing is starved.
- **About 10% random checks**, flagged as such, so the change-rate
  estimates stay unbiased.
- **Days to departure.** Departures more than about 90 days out need
  fewer checks than those 21–90 days out. In the formula this comes from
  learning Δ per days-to-departure bucket, not from a hand-written rule.
- **Probe, then fan out.** When a query shows a drop, raise the score of
  nearby dates, the other cabins on the same route, and the same route
  on airline and miles sources.

**Confirm before alerting.** A deal candidate triggers an immediate
re-query (and, where it exists, a check on the airline's booking page)
before a notification is sent.

## What this means for decisions already made

- **[0010](../../DECISIONS.md#0010-explore-runs-daily-the-watchlist-weekly)
  (watchlist weekly).** Weekly is too slow. Fares change about daily,
  and most low fares last about a day or more. With the date grid, a
  route costs only a few page loads, so a daily budget run by the
  scheduler above is affordable (about 67 families × about 3 grid
  loads). "Weekly" becomes the `τ_max` guard, not the schedule.
- **[0022](../../DECISIONS.md#0022-a-deal-is-20-under-the-60-day-median-or-an-all-time-low)
  (20% under the 60-day median).** A plain median across all departure
  dates labels every fare far from departure as a deal and misses deals
  close to departure. Compare against a baseline for the same route ×
  cabin × days-to-departure bucket (edges at 3/7/14/21/45/90/180 days).
  Keep holiday and peak departures separate, and use a robust spread
  (percentiles or median absolute deviation) instead of a flat 20%.
- **Multiple points of sale (Mullvad).** Vissers et al. found no
  discrimination by cookie or profile, and differences between countries
  came from taxes and currency. A fixed headless profile is fine. Fares
  from a foreign point of sale must be compared as an effective BRL cost,
  including IOF and the card spread.

## What to log from day one

- **One row per query attempt:**
  - identity: family, source, site, point of sale;
  - why it ran: scheduled / random / hint / fan-out / confirmation /
    manual;
  - the score it had, and the runner-up's score;
  - scheduled and actual time;
  - outcome: ok / blocked / CAPTCHA / error;
  - cost: seconds and Jev calls.
- **One row per observed cell:**
  - what it describes: route, cabin, trip length, departure date, days
    to departure, departure weekday;
  - the price: price, currency, taxes, miles plus taxes where relevant;
  - the flight: availability, carrier, stops, flight numbers.
- **The full extracted data per query**, not just the minimum price, so
  "change" and "drop" can be redefined later.
- **Explore observations**, linked to the families they hint at.
- **Occasional ground-truth bursts:** a few families checked every
  15–30 minutes for a day. They show how many changes we miss, how long
  drops last, and whether changes behave like a Poisson process.
