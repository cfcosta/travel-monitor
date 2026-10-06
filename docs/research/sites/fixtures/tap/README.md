# TAP fixtures

Captured 2026-10-06 from `booking.flytap.com` (see `../../tap.md`).
No cookies, tokens or personal data: the searches were anonymous.

| File | What |
| --- | --- |
| `availability-search.request.json` | `POST /bfm/rest/booking/availability/search` body for GRU→LIS one-way 2026-11-10, market BR |
| `availability-search.GRU-LIS.2026-11-10.oneway.json` | Its response: 35 flights, 92 offers, every cabin, the ±3-day ribbon (`outPanel`) |
| `calendar.request.json` | `POST /bfm/rest/booking/availability/calendar/` body (one month) |
| `calendar.GRU-LIS.2026-11.oneway.json` | Its response: 30 days of cached per-day economy prices (`bestPriceForDates`) |
| `calendar-lowest-monthly.GRU-LIS.json` | `GET .../calendar/prices/lowest/monthly`: 12 monthly minimums (stale) |
