-- "No flights" is an outcome of its own (DECISIONS.md 0067): the
-- `queries.outcome` CHECK gains 'no_flights'.
--
-- SQLite cannot alter a CHECK, so the table is rebuilt (SQLite's
-- "making other kinds of table schema changes" procedure), keeping every
-- row and id, its indexes, and the foreign keys of `observations` and
-- `explore_hints` that point at it. `queries` has no triggers or views.
--
-- sqlx runs a migration in a transaction, where `PRAGMA foreign_keys`
-- cannot change. Instead the foreign keys are deferred to the commit:
-- dropping `queries` counts each observation and Explore hint whose
-- query is gone, and inserting the same ids into the new `queries`
-- counts them back. Were any row lost, the commit would fail and the
-- migration would not apply. Renaming a new table over the old name
-- would not count them back, hence the copy out and back in.
PRAGMA defer_foreign_keys = ON;

CREATE TABLE queries_0004 AS SELECT * FROM queries;

DROP TABLE queries;

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
        outcome IN (
            'ok', 'blocked', 'unexpected', 'no_flights', 'logged_out', 'error'
        )
    ),
    outcome_detail TEXT,
    outcome_json TEXT,
    page_loads INTEGER,
    jev_calls INTEGER
);

INSERT INTO queries (id, family_id, reason, score, runner_up, reason_json,
    passengers, scheduled_at, started_at, finished_at, outcome,
    outcome_detail, outcome_json, page_loads, jev_calls)
SELECT id, family_id, reason, score, runner_up, reason_json,
       passengers, scheduled_at, started_at, finished_at, outcome,
       outcome_detail, outcome_json, page_loads, jev_calls
FROM queries_0004
ORDER BY id;

DROP TABLE queries_0004;

CREATE INDEX queries_family ON queries (family_id, outcome, finished_at);
CREATE INDEX queries_started ON queries (started_at);
