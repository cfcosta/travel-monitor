# United Airlines fixtures

Captured 2026-10-06 in the user's Chrome with an XHR/fetch hook. Session
identifiers (`cartId`, `CartId`, `lastResultId`, `LastResultId`) are
replaced with placeholders. No cookies, headers or tokens are stored.

| File | What |
| ---- | ---- |
| `fetch-sse-nested-flights.request.json` | Body of `POST /api/flight/FetchSSENestedFlights`, one way GRU→EWR 2026-11-10, Brazil point of sale (`/pt/br/`). |
| `fetch-sse-nested-flights.oneway.GRU-EWR.2026-11-10.sse.txt` | Its `text/event-stream` response, every event kept except `flightOption` (only the first 3 of 29 kept). Prices in BRL. |
| `fetch-sse-nested-flights.roundtrip.GRU-EWR.2026-11-10_17.pos-US.sse.txt` | Round trip 2026-11-10/17, US point of sale (`/en/us/`), first 2 of 29 `flightOption` events. Prices in USD, total round trip. |
| `fetch-flexible-calendars.request.json` | Body of `POST /api/flight/FetchFlexibleCalendars` (one way; sent when "Mostrar calendário" is opened). |
| `fetch-flexible-calendars.oneway.GRU-EWR.2026-11.json` | Its response: 30 days (2026-11-10 to 2026-12-09), lowest one-way fare per day, BRL. |
| `fetch-flexible-calendars.roundtrip-los7.GRU-EWR.pos-US.json` | Request and response of the round-trip calendar: 30 departure days, length of stay 7, USD. |
