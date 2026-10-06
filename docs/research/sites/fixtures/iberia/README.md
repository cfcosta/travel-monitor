# Iberia fixtures

Captured 2026-10-06 in the user's Chrome from `ibisservices.iberia.com`
(see `../../iberia.md`). No cookies, tokens or personal data: the search
was anonymous. The headless browser was blocked, so nothing here comes
from it.

| File | What |
| --- | --- |
| `availability-itinerary.request.json` | `POST /api/sse-avm/rs/v2/availability/itinerary` body for GRU→MAD one-way 2026-11-10, market BR |
| `availability-itinerary.GRU-MAD.2026-11-10.oneway.json` | Its response: 10 itineraries (`originDestinations[0].slices`), 35 offers across economy, premium economy and business, one with a 13% dynamic discount |
| `calendar-ribbon.request.json` | `POST /api/sse-avm/rs/v2/calendar/ribbon` body (±3 days) |
| `calendar-ribbon.GRU-MAD.2026-11-10.oneway.json` | Its response: 7 days of lowest prices (`calendarRibbon.outBoundRow.cells`) |
