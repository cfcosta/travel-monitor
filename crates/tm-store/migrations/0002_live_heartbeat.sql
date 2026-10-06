-- The heartbeat says what the daemon is doing right now (DECISIONS.md
-- 0057): the daemon writes it when a check starts and when it ends, and
-- the app shows the check in progress and counts down to the next one.
--
-- Every column is nullable and the old ones are unchanged, so a daemon
-- built before this migration keeps writing its heartbeat (its upsert
-- names only the old columns). The app trusts the check columns only
-- while `state` is 'checking'.

-- When the daemon expects to look for work again.
ALTER TABLE heartbeat ADD COLUMN next_check_at TEXT;

-- The check in progress: its family and query log row, why it runs
-- (QueryReason as JSON) and when it started. All NULL while idle.
ALTER TABLE heartbeat ADD COLUMN check_family_id INTEGER;
ALTER TABLE heartbeat ADD COLUMN check_query_id INTEGER;
ALTER TABLE heartbeat ADD COLUMN check_reason_json TEXT;
ALTER TABLE heartbeat ADD COLUMN check_started_at TEXT;
