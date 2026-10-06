-- The travel monitor's database (DECISIONS.md 0020, 0035, 0046).
--
-- Conventions:
-- * enums are stored as their serde snake_case names (`economy`,
--   `premium_economy`, `outbound`, `candidate` ...);
-- * money is an integer amount in minor units plus a currency column;
-- * timestamps are RFC 3339 text in UTC (as sqlx encodes
--   `DateTime<Utc>`), dates are `YYYY-MM-DD`;
-- * booleans are 0/1 integers.

-- One row: the seed marker and a counter bumped by every configuration
-- change, so the daemon can notice edits made in the app (0035).
CREATE TABLE meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    config_version INTEGER NOT NULL DEFAULT 0,
    seeded_at TEXT
);
INSERT INTO meta (id) VALUES (1);

------------------------------------------------------------------------
-- Configuration (0035)
------------------------------------------------------------------------

CREATE TABLE places (
    id INTEGER PRIMARY KEY,
    -- IATA airport or city code, upper case.
    code TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    country TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('airport', 'city')),
    -- Cities only; learned with a `q=` search (0041).
    freebase_id TEXT CHECK (freebase_id IS NULL OR kind = 'city'),
    lat REAL NOT NULL,
    lon REAL NOT NULL
);

-- A place is an origin while it has an active row here.
CREATE TABLE origins (
    place_id INTEGER PRIMARY KEY REFERENCES places (id),
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
);

-- A place is a destination while it has an active row here.
CREATE TABLE destinations (
    place_id INTEGER PRIMARY KEY REFERENCES places (id),
    -- The scheduler's weight for routes to it.
    weight REAL NOT NULL DEFAULT 1.0 CHECK (weight >= 0),
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
);

-- Derived: every active origin x active destination (sync_routes).
-- Rows are never deleted, so history survives a removal and a re-added
-- route continues the same series.
--
-- `sweep = 1` marks an origin's Explore pseudo-route: its destination is
-- the origin itself (0025, FamilyKind::Explore).
CREATE TABLE routes (
    id INTEGER PRIMARY KEY,
    origin_id INTEGER NOT NULL REFERENCES places (id),
    destination_id INTEGER NOT NULL REFERENCES places (id),
    sweep INTEGER NOT NULL DEFAULT 0 CHECK (sweep IN (0, 1)),
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    -- Round-trip discount learning and the additive stop rule (0042).
    rt_discount_ewma REAL NOT NULL DEFAULT 0.0,
    rt_discount_n INTEGER NOT NULL DEFAULT 0,
    additive_streak INTEGER NOT NULL DEFAULT 0,
    additive INTEGER NOT NULL DEFAULT 0 CHECK (additive IN (0, 1)),
    UNIQUE (origin_id, destination_id),
    CHECK ((sweep = 1) = (origin_id = destination_id))
);

-- Points of sale (0033).
CREATE TABLE points_of_sale (
    id INTEGER PRIMARY KEY,
    country TEXT NOT NULL UNIQUE,
    currency TEXT NOT NULL,
    is_home INTEGER NOT NULL DEFAULT 0 CHECK (is_home IN (0, 1)),
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
);
CREATE UNIQUE INDEX points_of_sale_one_home ON points_of_sale (is_home)
WHERE is_home = 1;

-- tm_domain::Settings, one typed row.
CREATE TABLE settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    adults INTEGER NOT NULL,
    children INTEGER NOT NULL,
    infants_in_seat INTEGER NOT NULL,
    infants_on_lap INTEGER NOT NULL,
    trip_min_days INTEGER NOT NULL,
    trip_max_days INTEGER NOT NULL,
    round_trip_days INTEGER NOT NULL,
    horizon_days INTEGER NOT NULL,
    iof REAL NOT NULL,
    card_spread REAL NOT NULL,
    daily_budget INTEGER NOT NULL,
    max_daily_budget INTEGER NOT NULL,
    max_interval_hours INTEGER NOT NULL,
    min_interval_hours INTEGER NOT NULL,
    random_share REAL NOT NULL,
    deal_threshold REAL NOT NULL,
    min_history_weeks INTEGER NOT NULL,
    additive_checks_to_stop INTEGER NOT NULL,
    language TEXT NOT NULL
);

-- Airline sites (0012).
CREATE TABLE airline_sources (
    code TEXT NOT NULL PRIMARY KEY,
    name TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
);

-- The destinations each airline's site is searched for.
CREATE TABLE airline_destinations (
    airline_code TEXT NOT NULL REFERENCES airline_sources (code)
    ON DELETE CASCADE,
    place_id INTEGER NOT NULL REFERENCES places (id),
    PRIMARY KEY (airline_code, place_id)
);

-- Miles programs (0023, 0024).
CREATE TABLE miles_programs (
    program TEXT NOT NULL PRIMARY KEY,
    name TEXT NOT NULL,
    login_state TEXT NOT NULL DEFAULT 'logged_out'
    CHECK (login_state IN ('logged_in', 'logged_out', 'expired')),
    last_checked TEXT,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
);

------------------------------------------------------------------------
-- Query families and their scheduler state (0028)
------------------------------------------------------------------------

CREATE TABLE families (
    id INTEGER PRIMARY KEY,
    -- Canonical text of the FamilyKey, for uniqueness.
    key TEXT NOT NULL UNIQUE,
    route_id INTEGER NOT NULL REFERENCES routes (id),
    cabin TEXT NOT NULL
    CHECK (cabin IN ('economy', 'premium_economy', 'business', 'first')),
    source TEXT NOT NULL
    CHECK (source IN ('google_flights', 'airline', 'miles')),
    -- Airline code or miles program; NULL for Google Flights.
    source_code TEXT
    CHECK ((source = 'google_flights') = (source_code IS NULL)),
    pos_id INTEGER NOT NULL REFERENCES points_of_sale (id),
    kind TEXT NOT NULL CHECK (kind IN ('leg', 'round_trip', 'explore')),
    leg TEXT CHECK ((kind = 'leg') = (leg IS NOT NULL)),
    days INTEGER CHECK ((kind = 'round_trip') = (days IS NOT NULL)),
    -- The rate-limit bucket (Source::site).
    site TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    -- Gamma posterior on the change rate, in changes per day.
    -- New families get FamilyStateRecord::default() (the scheduler's
    -- priors), bound at insert.
    chg_a REAL NOT NULL,
    chg_b REAL NOT NULL,
    -- Beta posterior on P(change is a drop).
    deal_a REAL NOT NULL,
    deal_b REAL NOT NULL,
    -- Beta posterior on Explore hint precision.
    prec_a REAL NOT NULL,
    prec_b REAL NOT NULL,
    n_hints INTEGER NOT NULL DEFAULT 0,
    last_check TEXT,
    last_min_price_minor INTEGER,
    checks INTEGER NOT NULL DEFAULT 0,
    -- EWMA of page loads per check.
    cost_loads REAL NOT NULL
);
CREATE INDEX families_route ON families (route_id, cabin);

------------------------------------------------------------------------
-- Query log (0028, 0031)
------------------------------------------------------------------------

CREATE TABLE queries (
    id INTEGER PRIMARY KEY,
    family_id INTEGER NOT NULL REFERENCES families (id),
    -- QueryReason::label.
    reason TEXT NOT NULL,
    score REAL,
    runner_up REAL,
    reason_json TEXT NOT NULL,
    -- Passengers::key of the party searched (0036).
    passengers TEXT NOT NULL,
    scheduled_at TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    outcome TEXT CHECK (
        outcome IN ('ok', 'blocked', 'unexpected', 'logged_out', 'error')
    ),
    outcome_detail TEXT,
    outcome_json TEXT,
    page_loads INTEGER,
    jev_calls INTEGER
);
CREATE INDEX queries_family ON queries (family_id, outcome, finished_at);
CREATE INDEX queries_started ON queries (started_at);

------------------------------------------------------------------------
-- Observations, append-only (0020)
------------------------------------------------------------------------

CREATE TABLE observations (
    id INTEGER PRIMARY KEY,
    query_id INTEGER NOT NULL REFERENCES queries (id),
    family_id INTEGER NOT NULL REFERENCES families (id),
    observed_at TEXT NOT NULL,
    departure TEXT NOT NULL,
    return_date TEXT,
    price_minor INTEGER,
    currency TEXT,
    miles INTEGER,
    carrier TEXT,
    stops INTEGER,
    duration_minutes INTEGER,
    seller TEXT,
    -- Copied from the query (0036).
    passengers TEXT NOT NULL,
    CHECK ((price_minor IS NULL) = (currency IS NULL))
);
CREATE INDEX observations_family ON observations (family_id, observed_at);
CREATE INDEX observations_query ON observations (query_id);

CREATE TRIGGER observations_no_update BEFORE UPDATE ON observations
BEGIN
    SELECT RAISE(ABORT, 'observations are append-only');
END;

CREATE TRIGGER observations_no_delete BEFORE DELETE ON observations
BEGIN
    SELECT RAISE(ABORT, 'observations are append-only');
END;

-- Explore results: hints, not series (0041).
CREATE TABLE explore_hints (
    id INTEGER PRIMARY KEY,
    query_id INTEGER NOT NULL REFERENCES queries (id),
    origin_id INTEGER NOT NULL REFERENCES places (id),
    cabin TEXT NOT NULL,
    pos_id INTEGER NOT NULL REFERENCES points_of_sale (id),
    observed_at TEXT NOT NULL,
    destination_freebase_id TEXT NOT NULL,
    destination_airport TEXT NOT NULL,
    price_minor INTEGER NOT NULL,
    currency TEXT NOT NULL,
    departure TEXT,
    return_date TEXT,
    stops INTEGER,
    duration_minutes INTEGER,
    carrier TEXT,
    passengers TEXT NOT NULL
);
CREATE INDEX explore_hints_time ON explore_hints (observed_at);

------------------------------------------------------------------------
-- Deals (0030, 0031, 0034, 0040)
------------------------------------------------------------------------

CREATE TABLE deals (
    id INTEGER PRIMARY KEY,
    route_id INTEGER NOT NULL REFERENCES routes (id),
    cabin TEXT NOT NULL,
    pos_id INTEGER NOT NULL REFERENCES points_of_sale (id),
    departure TEXT NOT NULL,
    return_date TEXT NOT NULL,
    price_minor INTEGER NOT NULL,
    currency TEXT NOT NULL,
    effective_minor INTEGER NOT NULL,
    effective_currency TEXT NOT NULL,
    baseline_minor INTEGER,
    baseline_currency TEXT,
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
    CHECK ((baseline_minor IS NULL) = (baseline_currency IS NULL))
);
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
CREATE INDEX seller_quotes_deal ON seller_quotes (deal_id);

------------------------------------------------------------------------
-- App -> daemon requests (0046)
------------------------------------------------------------------------

CREATE TABLE requests (
    id INTEGER PRIMARY KEY,
    -- The RequestKind tag; `payload` is the whole RequestKind as JSON.
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
    CHECK (status IN ('pending', 'claimed', 'done', 'failed')),
    created_at TEXT NOT NULL,
    claimed_at TEXT,
    finished_at TEXT,
    detail TEXT
);
CREATE INDEX requests_status ON requests (status, id);

------------------------------------------------------------------------
-- Daemon bookkeeping (0038, 0039)
------------------------------------------------------------------------

CREATE TABLE budget (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    daily_budget INTEGER NOT NULL,
    loads_today INTEGER NOT NULL,
    day TEXT NOT NULL,
    last_ramp_at TEXT
);

CREATE TABLE blocks (
    id INTEGER PRIMARY KEY,
    site TEXT NOT NULL,
    at TEXT NOT NULL,
    detail TEXT NOT NULL,
    loads_last_hour INTEGER NOT NULL
);
CREATE INDEX blocks_time ON blocks (at);

CREATE TABLE exchange_rates (
    id INTEGER PRIMARY KEY,
    currency TEXT NOT NULL,
    brl_per_unit REAL NOT NULL CHECK (brl_per_unit > 0),
    at TEXT NOT NULL
);
CREATE INDEX exchange_rates_currency ON exchange_rates (currency, at);

CREATE TABLE heartbeat (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    pid INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    state TEXT NOT NULL,
    next_check TEXT,
    loads_today INTEGER NOT NULL,
    budget INTEGER NOT NULL
);

------------------------------------------------------------------------
-- Configuration version triggers
------------------------------------------------------------------------

CREATE TRIGGER places_ins AFTER INSERT ON places
BEGIN UPDATE meta SET config_version = config_version + 1; END;
CREATE TRIGGER places_upd AFTER UPDATE ON places
BEGIN UPDATE meta SET config_version = config_version + 1; END;

CREATE TRIGGER origins_ins AFTER INSERT ON origins
BEGIN UPDATE meta SET config_version = config_version + 1; END;
CREATE TRIGGER origins_upd AFTER UPDATE ON origins
BEGIN UPDATE meta SET config_version = config_version + 1; END;

CREATE TRIGGER destinations_ins AFTER INSERT ON destinations
BEGIN UPDATE meta SET config_version = config_version + 1; END;
CREATE TRIGGER destinations_upd AFTER UPDATE ON destinations
BEGIN UPDATE meta SET config_version = config_version + 1; END;

CREATE TRIGGER points_of_sale_ins AFTER INSERT ON points_of_sale
BEGIN UPDATE meta SET config_version = config_version + 1; END;
CREATE TRIGGER points_of_sale_upd AFTER UPDATE ON points_of_sale
BEGIN UPDATE meta SET config_version = config_version + 1; END;

CREATE TRIGGER settings_ins AFTER INSERT ON settings
BEGIN UPDATE meta SET config_version = config_version + 1; END;
CREATE TRIGGER settings_upd AFTER UPDATE ON settings
BEGIN UPDATE meta SET config_version = config_version + 1; END;

CREATE TRIGGER airline_sources_ins AFTER INSERT ON airline_sources
BEGIN UPDATE meta SET config_version = config_version + 1; END;
CREATE TRIGGER airline_sources_upd AFTER UPDATE ON airline_sources
BEGIN UPDATE meta SET config_version = config_version + 1; END;
CREATE TRIGGER airline_sources_del AFTER DELETE ON airline_sources
BEGIN UPDATE meta SET config_version = config_version + 1; END;

CREATE TRIGGER airline_destinations_ins AFTER INSERT ON airline_destinations
BEGIN UPDATE meta SET config_version = config_version + 1; END;
CREATE TRIGGER airline_destinations_del AFTER DELETE ON airline_destinations
BEGIN UPDATE meta SET config_version = config_version + 1; END;

CREATE TRIGGER miles_programs_ins AFTER INSERT ON miles_programs
BEGIN UPDATE meta SET config_version = config_version + 1; END;
CREATE TRIGGER miles_programs_upd AFTER UPDATE OF active, name
ON miles_programs
BEGIN UPDATE meta SET config_version = config_version + 1; END;

-- The additive flag decides which round-trip families exist (0042).
CREATE TRIGGER routes_additive AFTER UPDATE OF additive ON routes
WHEN old.additive != new.additive
BEGIN UPDATE meta SET config_version = config_version + 1; END;
