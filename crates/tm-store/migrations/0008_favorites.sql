-- Favorite destinations (DECISIONS.md 0094). Additive.

-- A favorite place: every route to it, in every cabin, cash and miles,
-- from any origin, is checked about twice as often and is a deal at a
-- smaller drop. Starring or unstarring is an update of `destinations`,
-- so `destinations_upd` bumps `meta.config_version` and the daemon
-- reloads, as for any other configuration change (0035).
ALTER TABLE destinations ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0
CHECK (favorite IN (0, 1));

-- The deal threshold for favorites, next to the global `deal_threshold`
-- (0.20): 10% under the series' usual price.
ALTER TABLE settings ADD COLUMN favorite_deal_threshold REAL NOT NULL
DEFAULT 0.10 CHECK (favorite_deal_threshold BETWEEN 0 AND 1);
