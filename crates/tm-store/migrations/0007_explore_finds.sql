-- Destinations the Explore sweep prices outside the watchlist
-- (DECISIONS.md 0087). Additive.

-- Explore's own name, country (in words, as Google writes it) and
-- coordinates for each destination, so a find can be shown and
-- watched. NULL on every older row.
ALTER TABLE explore_hints ADD COLUMN destination_name TEXT;
ALTER TABLE explore_hints ADD COLUMN destination_country TEXT;
ALTER TABLE explore_hints ADD COLUMN destination_lat REAL;
ALTER TABLE explore_hints ADD COLUMN destination_lon REAL;

CREATE INDEX explore_hints_destination
ON explore_hints (destination_freebase_id, observed_at);

-- One row per drop of an unwatched destination's Explore price below its
-- own Explore history: the daemon notifies once per row. A row is open
-- while `ended_at` is NULL; at most one is open per origin, cabin and
-- destination. `notified_at` stays NULL until the notification server
-- took the notice.
CREATE TABLE explore_drops (
    id INTEGER PRIMARY KEY,
    origin_id INTEGER NOT NULL REFERENCES places (id),
    cabin TEXT NOT NULL,
    destination_freebase_id TEXT NOT NULL,
    destination_airport TEXT NOT NULL,
    destination_name TEXT,
    price_minor INTEGER NOT NULL,
    median_minor INTEGER NOT NULL,
    currency TEXT NOT NULL,
    departure TEXT,
    return_date TEXT,
    carrier TEXT,
    stops INTEGER,
    found_at TEXT NOT NULL,
    notified_at TEXT,
    ended_at TEXT
);
CREATE UNIQUE INDEX explore_drops_open
ON explore_drops (origin_id, cabin, destination_freebase_id)
WHERE ended_at IS NULL;
