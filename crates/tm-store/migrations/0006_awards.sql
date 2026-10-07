-- Award prices: miles plus taxes, their own series (DECISIONS.md 0023,
-- 0083).
--
-- Additive except for `deals`, which is rebuilt so that `return_date`
-- may be NULL (a one-way award deal). Every existing row keeps its id
-- and reads as a cash deal.

------------------------------------------------------------------------
-- Observations: what an award has that a cash price does not
------------------------------------------------------------------------

-- `miles` and `price_minor`/`currency` (the taxes) already hold an
-- award's price. These hold the program's fare before taxes for the
-- brand (BRL; the implied value of a mile is it over the miles), the
-- brand, the fare basis it is priced on (awards are offered on some
-- booking classes only), the seats left at that price and the
-- program's own "few left" flag. `flights` is the itinerary's key,
-- every segment's flight and local departure
-- (`LA4776@2026-11-10T00:05/LA8150@2026-11-10T05:55`), for cash and
-- award observations alike, so an award can be matched with its own
-- itinerary's cash price. All NULL on cash observations and on every
-- older row.
ALTER TABLE observations ADD COLUMN fare_minor INTEGER;
ALTER TABLE observations ADD COLUMN fare_currency TEXT;
ALTER TABLE observations ADD COLUMN brand TEXT;
ALTER TABLE observations ADD COLUMN fare_basis TEXT;
ALTER TABLE observations ADD COLUMN seats INTEGER;
ALTER TABLE observations ADD COLUMN low_availability INTEGER;
ALTER TABLE observations ADD COLUMN flights TEXT;

------------------------------------------------------------------------
-- Settings: when an award's rate is unusual
------------------------------------------------------------------------

ALTER TABLE settings ADD COLUMN unusual_rate REAL NOT NULL DEFAULT 0.15;

------------------------------------------------------------------------
-- Miles programs: the user's value of a mile, the price the program
-- sells them at, and which dates it is asked for
------------------------------------------------------------------------

-- BRL minor units per 1,000 miles; NULL until the user sets one.
ALTER TABLE miles_programs ADD COLUMN value_minor INTEGER
CHECK (value_minor IS NULL OR value_minor > 0);
ALTER TABLE miles_programs ADD COLUMN buy_minor INTEGER
CHECK (buy_minor IS NULL OR buy_minor > 0);
-- When the program's login session ends, as read from its profile's
-- cookies (LATAM Pass: `_xp_session`, renewed on every load, and
-- `auth0`, not renewed); NULL when unknown.
ALTER TABLE miles_programs ADD COLUMN session_until TEXT;
-- tm_domain::AwardDates.
ALTER TABLE miles_programs ADD COLUMN dates TEXT NOT NULL
DEFAULT 'cheap_trips_and_weekly'
CHECK (dates IN ('cheap_trips_and_weekly', 'cheap_trips', 'every'));

------------------------------------------------------------------------
-- Deals: a series, and one-way award deals
------------------------------------------------------------------------

-- SQLite cannot drop a NOT NULL, so `deals` is rebuilt as 0004 rebuilt
-- `queries`: copied out, dropped, created and copied back with the same
-- ids, foreign keys deferred to the commit. `seller_quotes` points at
-- `deals` with ON DELETE CASCADE, and dropping `deals` would delete its
-- rows, so it is copied out and back too (dropped first, so nothing
-- cascades).
PRAGMA defer_foreign_keys = ON;

CREATE TABLE seller_quotes_0006 AS SELECT * FROM seller_quotes;
CREATE TABLE deals_0006 AS SELECT * FROM deals;

DROP TABLE seller_quotes;
DROP TABLE deals;

CREATE TABLE deals (
    id INTEGER PRIMARY KEY,
    route_id INTEGER NOT NULL REFERENCES routes (id),
    cabin TEXT NOT NULL,
    pos_id INTEGER NOT NULL REFERENCES points_of_sale (id),
    -- The miles program of an award deal; NULL for cash.
    program TEXT,
    -- The leg of a one-way award deal; NULL for a round trip (cash, or an
-- award round trip, which LATAM Pass prices below its two one-ways).
    leg TEXT CHECK (leg IS NULL OR leg IN ('outbound', 'return')),
    departure TEXT NOT NULL,
    -- NULL only for a one-way deal.
    return_date TEXT CHECK ((return_date IS NULL) = (leg IS NOT NULL)),
    -- For an award: its taxes.
    price_minor INTEGER NOT NULL,
    currency TEXT NOT NULL,
    effective_minor INTEGER NOT NULL,
    effective_currency TEXT NOT NULL,
    baseline_minor INTEGER,
    baseline_currency TEXT,
    -- An award's miles and the award series' bucket median.
    miles INTEGER CHECK ((miles IS NULL) = (program IS NULL)),
    baseline_miles INTEGER,
    brand TEXT,
    seats INTEGER,
    low_availability INTEGER,
    -- The cash fare of the same leg and date, for scale (display only).
    cash_minor INTEGER,
    cash_currency TEXT,
    reason TEXT NOT NULL CHECK (reason IN ('below_median', 'all_time_low')),
    change REAL,
    estimated INTEGER NOT NULL CHECK (estimated IN (0, 1)),
    status TEXT NOT NULL
    CHECK (status IN ('candidate', 'confirmed', 'rejected', 'expired')),
    found_at TEXT NOT NULL,
    confirmed_at TEXT,
    rejected_at TEXT,
    expired_at TEXT,
    seen INTEGER NOT NULL DEFAULT 0 CHECK (seen IN (0, 1)),
    carrier TEXT,
    seller TEXT,
    url TEXT,
    CHECK ((baseline_minor IS NULL) = (baseline_currency IS NULL)),
    CHECK ((cash_minor IS NULL) = (cash_currency IS NULL))
);

INSERT INTO deals (id, route_id, cabin, pos_id, departure, return_date,
    price_minor, currency, effective_minor, effective_currency,
    baseline_minor, baseline_currency, reason, change, estimated, status,
    found_at, confirmed_at, rejected_at, expired_at, seen, carrier, seller,
    url)
SELECT id, route_id, cabin, pos_id, departure, return_date,
       price_minor, currency, effective_minor, effective_currency,
       baseline_minor, baseline_currency, reason, change, estimated, status,
       found_at, confirmed_at, rejected_at, expired_at, seen, carrier,
       seller, url
FROM deals_0006
ORDER BY id;

CREATE INDEX deals_status ON deals (status, seen, found_at);

CREATE TABLE seller_quotes (
    id INTEGER PRIMARY KEY,
    deal_id INTEGER NOT NULL REFERENCES deals (id) ON DELETE CASCADE,
    pos_id INTEGER NOT NULL REFERENCES points_of_sale (id),
    seller TEXT NOT NULL,
    is_airline INTEGER NOT NULL CHECK (is_airline IN (0, 1)),
    price_minor INTEGER NOT NULL,
    currency TEXT NOT NULL,
    quoted_minor INTEGER,
    quoted_currency TEXT,
    CHECK ((quoted_minor IS NULL) = (quoted_currency IS NULL))
);

INSERT INTO seller_quotes (id, deal_id, pos_id, seller, is_airline,
    price_minor, currency, quoted_minor, quoted_currency)
SELECT id, deal_id, pos_id, seller, is_airline, price_minor, currency,
       quoted_minor, quoted_currency
FROM seller_quotes_0006
ORDER BY id;

CREATE INDEX seller_quotes_deal ON seller_quotes (deal_id);

DROP TABLE seller_quotes_0006;
DROP TABLE deals_0006;
