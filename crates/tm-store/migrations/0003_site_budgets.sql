-- Each website has its own daily budget (DECISIONS.md 0060), and sites
-- are checked in parallel, each in its own browser (0062).

-- One budget per site (`Source::site()`: 'google_flights', 'airline:TP',
-- ...). `start_budget` is where the budget starts and what the
-- configuration last set (Google Flights: the settings' daily budget,
-- from `--budget`; other sites: 300 or `--site-budget`); a change of it
-- resets `daily_budget`. `daily_budget` is the budget in force, raised
-- 10% per block-free day up to `max_daily_budget` and halved by a block
-- (0039). `loads_today` counts the loads of `day`; `last_ramp_at` starts
-- the current block-free stretch. A site without a row starts from its
-- defaults.
CREATE TABLE site_budgets (
    site TEXT NOT NULL PRIMARY KEY,
    start_budget INTEGER NOT NULL CHECK (start_budget > 0),
    daily_budget INTEGER NOT NULL CHECK (daily_budget > 0),
    max_daily_budget INTEGER NOT NULL CHECK (max_daily_budget > 0),
    loads_today INTEGER NOT NULL DEFAULT 0,
    day TEXT NOT NULL,
    last_ramp_at TEXT
);

-- The single budget row so far was Google Flights' in all but name
-- (TAP's loads were counted in it too): it becomes Google Flights' row.
-- Its start is the settings' daily budget, so an unchanged `--budget`
-- keeps the ramped budget.
INSERT INTO site_budgets (site, start_budget, daily_budget,
    max_daily_budget, loads_today, day, last_ramp_at)
SELECT 'google_flights',
       MAX(1, COALESCE(s.daily_budget, b.daily_budget)),
       MAX(1, b.daily_budget),
       MAX(1, b.daily_budget, COALESCE(s.max_daily_budget, b.daily_budget)),
       b.loads_today,
       b.day,
       b.last_ramp_at
FROM budget b LEFT JOIN settings s ON s.id = 1
WHERE b.id = 1;

DROP TABLE budget;

-- What each site's worker is doing right now (0057, 0062), next to the
-- global heartbeat (which keeps the process's pid, state and totals).
-- `state` is 'idle', 'checking', 'paused for login: <program>',
-- 'cooling down', 'browser unavailable' or 'stopped'. The check columns
-- describe the check in progress and are NULL otherwise; the app trusts
-- them only while `state` is 'checking'. The daemon empties the table
-- when it starts.
CREATE TABLE site_status (
    site TEXT NOT NULL PRIMARY KEY,
    updated_at TEXT NOT NULL,
    state TEXT NOT NULL,
    next_check_at TEXT,
    check_family_id INTEGER,
    check_query_id INTEGER,
    check_reason_json TEXT,
    check_started_at TEXT
);
