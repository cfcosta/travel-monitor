-- Each site runs several checks at once (DECISIONS.md 0072): the check
-- in progress moves out of `site_status` into a table of its own, one
-- row per running check, each with its row in the query log (0057).
--
-- Nothing is copied over: the daemon clears every site's status when
-- it starts.
CREATE TABLE site_checks (
    site TEXT NOT NULL,
    family_id INTEGER NOT NULL,
    query_id INTEGER,
    reason_json TEXT NOT NULL,
    started_at TEXT NOT NULL
);
CREATE INDEX site_checks_site ON site_checks (site);

ALTER TABLE site_status DROP COLUMN check_family_id;
ALTER TABLE site_status DROP COLUMN check_query_id;
ALTER TABLE site_status DROP COLUMN check_reason_json;
ALTER TABLE site_status DROP COLUMN check_started_at;
