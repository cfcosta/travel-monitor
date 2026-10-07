-- The live database's share of DECISIONS.md 0086 (2026-10-07): 47
-- destinations join the watchlist, and ten metro areas with several
-- airports become cities. Run once at redeploy, with the daemon
-- stopped:
--
--     sqlite3 ~/.local/share/travel-monitor/tm.db < docs/ops/2026-10-07-destinations.sql
--
-- Idempotent and additive: a second run changes nothing. It only
-- touches `places` and `destinations`; their triggers bump
-- `meta.config_version`, so the daemon re-syncs routes and families at
-- start-up (0035), exactly as for a destination added in the app: one
-- route per origin, and its families from `sync_families`. It does not
-- depend on any migration newer than 0006.

BEGIN IMMEDIATE;

------------------------------------------------------------------------
-- Widened codes. The place keeps its id, so its routes keep their ids,
-- families, observations, deals and scheduler state; from now on their
-- searches cover every airport Google groups under the city's Freebase
-- ID (seen priced in Explore sweeps, 2026-10-07).
------------------------------------------------------------------------

-- MCO -> ORL: MCO + SFB.
UPDATE places SET code = 'ORL', kind = 'city', freebase_id = '/m/0ply0'
WHERE code = 'MCO' AND kind = 'airport';
-- ORD -> CHI: ORD + MDW.
UPDATE places SET code = 'CHI', kind = 'city', freebase_id = '/m/01_d4'
WHERE code = 'ORD' AND kind = 'airport';
-- IAH -> HOU: IAH + HOU (Hobby).
UPDATE places SET code = 'HOU', kind = 'city', freebase_id = '/m/03l2n'
WHERE code = 'IAH' AND kind = 'airport';
-- YYZ -> YTO: YYZ + YTZ.
UPDATE places SET code = 'YTO', kind = 'city', freebase_id = '/m/0h7h6'
WHERE code = 'YYZ' AND kind = 'airport';
-- YUL -> YMQ: YUL + YHU.
UPDATE places SET code = 'YMQ', kind = 'city', freebase_id = '/m/052p7'
WHERE code = 'YUL' AND kind = 'airport';
-- DFW, now a city: DFW + DAL.
UPDATE places SET code = 'DFW', kind = 'city', freebase_id = '/m/0f2rq'
WHERE code = 'DFW' AND kind = 'airport';
-- BKK, now a city: BKK + DMK.
UPDATE places SET code = 'BKK', kind = 'city', freebase_id = '/m/0fn2g'
WHERE code = 'BKK' AND kind = 'airport';
-- MEX, now a city: MEX + NLU.
UPDATE places SET code = 'MEX', kind = 'city', freebase_id = '/m/04sqj'
WHERE code = 'MEX' AND kind = 'airport';
-- IST, now a city: IST + SAW.
UPDATE places SET code = 'IST', kind = 'city', freebase_id = '/m/09949m'
WHERE code = 'IST' AND kind = 'airport';
-- DXB, now a city: DXB + SHJ.
UPDATE places SET code = 'DXB', kind = 'city', freebase_id = '/m/01f08r'
WHERE code = 'DXB' AND kind = 'airport';

------------------------------------------------------------------------
-- New destinations, as `seed_defaults` creates them: a place (an
-- airport, or a city with its known Freebase ID) and an active
-- destination of weight 1. A place or destination that already exists
-- is left as it is (a later removal or weight from the app stays).
------------------------------------------------------------------------

INSERT INTO places (code, name, country, kind, freebase_id, lat, lon)
VALUES
    ('VCE', 'Venice', 'IT', 'airport', NULL, 45.44, 12.32),
    ('FLR', 'Florence', 'IT', 'airport', NULL, 43.77, 11.26),
    ('NCE', 'Nice', 'FR', 'airport', NULL, 43.70, 7.27),
    ('EDI', 'Edinburgh', 'GB', 'airport', NULL, 55.95, -3.19),
    ('BUD', 'Budapest', 'HU', 'airport', NULL, 47.50, 19.04),
    ('CPH', 'Copenhagen', 'DK', 'airport', NULL, 55.68, 12.57),
    ('MLA', 'Malta', 'MT', 'airport', NULL, 35.90, 14.51),
    ('JTR', 'Santorini', 'GR', 'airport', NULL, 36.39, 25.46),
    ('FNC', 'Madeira', 'PT', 'airport', NULL, 32.65, -16.91),
    ('PDL', 'Azores', 'PT', 'airport', NULL, 37.74, -25.67),
    ('SVQ', 'Seville', 'ES', 'airport', NULL, 37.39, -5.98),
    ('DBV', 'Dubrovnik', 'HR', 'airport', NULL, 42.65, 18.09),
    ('OSA', 'Osaka', 'JP', 'city', '/m/0dqyw', 34.69, 135.50),
    ('TPE', 'Taipei', 'TW', 'city', '/m/0ftkx', 25.03, 121.57),
    ('SGN', 'Ho Chi Minh City', 'VN', 'airport', NULL, 10.82, 106.63),
    ('HAN', 'Hanoi', 'VN', 'airport', NULL, 21.03, 105.85),
    ('HKT', 'Phuket', 'TH', 'airport', NULL, 7.88, 98.39),
    ('KUL', 'Kuala Lumpur', 'MY', 'airport', NULL, 3.15, 101.69),
    ('DEL', 'Delhi', 'IN', 'airport', NULL, 28.61, 77.21),
    ('BOM', 'Mumbai', 'IN', 'airport', NULL, 19.08, 72.88),
    ('MLE', 'Maldives', 'MV', 'airport', NULL, 4.18, 73.51),
    ('MRU', 'Mauritius', 'MU', 'airport', NULL, -20.16, 57.50),
    ('SEZ', 'Seychelles', 'SC', 'airport', NULL, -4.62, 55.45),
    ('HNL', 'Honolulu', 'US', 'airport', NULL, 21.31, -157.86),
    ('YVR', 'Vancouver', 'CA', 'airport', NULL, 49.28, -123.12),
    ('SEA', 'Seattle', 'US', 'airport', NULL, 47.61, -122.33),
    ('SAN', 'San Diego', 'US', 'airport', NULL, 32.72, -117.16),
    ('DEN', 'Denver', 'US', 'airport', NULL, 39.74, -104.99),
    ('MSY', 'New Orleans', 'US', 'airport', NULL, 29.95, -90.07),
    ('YYC', 'Calgary', 'CA', 'airport', NULL, 51.05, -114.07),
    ('TQO', 'Tulum', 'MX', 'airport', NULL, 20.21, -87.47),
    ('SJD', 'Los Cabos', 'MX', 'airport', NULL, 23.06, -109.70),
    ('SJU', 'San Juan', 'PR', 'airport', NULL, 18.47, -66.11),
    ('NAS', 'Nassau', 'BS', 'airport', NULL, 25.05, -77.36),
    ('MBJ', 'Montego Bay', 'JM', 'airport', NULL, 18.47, -77.92),
    ('PNT', 'Puerto Natales', 'CL', 'airport', NULL, -51.73, -72.51),
    ('LEC', 'Lençóis', 'BR', 'airport', NULL, -12.56, -41.39),
    ('TLV', 'Tel Aviv', 'IL', 'airport', NULL, 32.09, 34.78),
    ('AMM', 'Amman', 'JO', 'airport', NULL, 31.95, 35.93),
    ('CAI', 'Cairo', 'EG', 'airport', NULL, 30.04, 31.24),
    ('NBO', 'Nairobi', 'KE', 'airport', NULL, -1.29, 36.82),
    ('ZNZ', 'Zanzibar', 'TZ', 'airport', NULL, -6.17, 39.20),
    ('VFA', 'Victoria Falls', 'ZW', 'airport', NULL, -17.93, 25.84),
    ('MEL', 'Melbourne', 'AU', 'airport', NULL, -37.81, 144.96),
    ('ZQN', 'Queenstown', 'NZ', 'airport', NULL, -45.03, 168.66),
    ('PPT', 'Tahiti', 'PF', 'airport', NULL, -17.54, -149.57),
    ('NAN', 'Fiji', 'FJ', 'airport', NULL, -17.80, 177.42)
ON CONFLICT (code) DO NOTHING;

INSERT INTO destinations (place_id, weight, active)
SELECT id, 1.0, 1 FROM places
WHERE code IN (
    'VCE', 'FLR', 'NCE', 'EDI', 'BUD', 'CPH', 'MLA', 'JTR', 'FNC',
    'PDL', 'SVQ', 'DBV', 'OSA', 'TPE', 'SGN', 'HAN', 'HKT', 'KUL',
    'DEL', 'BOM', 'MLE', 'MRU', 'SEZ', 'HNL', 'YVR', 'SEA', 'SAN',
    'DEN', 'MSY', 'YYC', 'TQO', 'SJD', 'SJU', 'NAS', 'MBJ', 'PNT',
    'LEC', 'TLV', 'AMM', 'CAI', 'NBO', 'ZNZ', 'VFA', 'MEL', 'ZQN',
    'PPT', 'NAN'
)
ON CONFLICT (place_id) DO NOTHING;

COMMIT;

-- What the coordinator should see: 163 active destinations (116 + 47),
-- the ten widened cities and the two new ones (OSA, TPE).
SELECT 'active destinations', COUNT(*) FROM destinations WHERE active = 1;
SELECT code, kind, freebase_id FROM places
WHERE code IN ('ORL', 'CHI', 'HOU', 'YTO', 'YMQ', 'DFW', 'BKK', 'MEX', 'IST',
               'DXB', 'OSA', 'TPE')
ORDER BY code;
