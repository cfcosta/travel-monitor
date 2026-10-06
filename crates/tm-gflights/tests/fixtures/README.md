# Captured Google Flights RPCs

Captured on 2026-10-05/06 with chrome-cdp from www.google.com/travel
(gl=BR, curr=BRL, hl=en). Each pair is one XHR: the form body the page
sent (`*.request.txt`, `f.req=…&at=…`) and the raw `rt=c` response
(`*.response.txt`). See docs/research/google-flights.md.

- `roundtrip_sao_lis_GetCalendarGraph_*`: round-trip price graph, 7-day
  trips, SAO→LIS, three pages (21 days each).
- `roundtrip_sao_lis_GetCalendarGrid_*`: round-trip date grid.
- `oneway_sao_lis_GetCalendarGrid_*`, `oneway_sao_lis_GetCalendarGraph_*`:
  one-way grid and price graph (first page ~60 days, then 21).
- `explore_sao_economy_GetExploreDestinations_*`: the Explore sweep from
  SAO, economy (72 destinations).
- `tfs_reference.py`: the Python tfs decoder/encoder used in the probes.

The `at` token in request bodies is a short-lived XSRF token; it is
kept only as captured.

## Added on 2026-10-06 (results and booking pages)

Same method (an XHR hook installed with
`Page.addScriptToEvaluateOnNewDocument`; the hook is
`tm_gflights::page::XHR_CAPTURE_HOOK_JS`), SAO→LIS, Nov 10–17, economy.

- `roundtrip_sao_lis_GetShoppingResults_0`: the departing flights
  (11 itineraries; six streamed payloads, the last is complete).
- `roundtrip_sao_lis_GetShoppingResults_1`: the returning flights after
  picking TP84 (5 itineraries; prices are round-trip totals).
- `roundtrip_sao_lis_GetBookingResults_0`: the booking page for TP84 +
  TP89 under gl=BR (11 sellers, all quoted in BRL).
- `roundtrip_sao_lis_us_GetBookingResults_0`: the same itinerary reached
  through the gl=US search page (18 sellers, most with a USD quote,
  e.g. Booking.com R$3,709 / $742).
- `roundtrip_sao_lis_us_direct_GetBookingResults_0`: the gl=BR booking
  URL (with its BR `tfu`) loaded directly under gl=US: only the airline.
- `explore_sao_business_GetExploreDestinations_0`: Explore from SAO with
  " business class" in `q=` (request field 5 = 3; 68 destinations in the
  first payload, 4 more streamed one per payload).
- `roundtrip_sao_lis_initdata_ds1.txt`, `oneway_sao_lis_initdata_ds1.txt`:
  the `AF_initDataCallback({key: 'ds:1', …})` scripts of results pages.
  A page loaded from a URL renders its first results from this embedded
  data; no `GetShoppingResults` RPC is made for them.
- `resolve_rome_dataServiceRequests.txt`: the `AF_dataServiceRequests`
  script of `q=Flights to Rome`: the destination resolved to `/m/06c62`.
- `tfs_urls.txt`: the search and booking URLs seen (with selected
  flights), plus two generated in code that the page loaded, one of them
  the passenger-kind probe.
