-- Profiles, slip titles and the EUR/USD rate (plan step 6.0c, l6-contracts L6-2). Additive only:
-- new tables; 0001-0007 stay as applied. Conventions as in 0001_desk.sql (`*_at` Unix s,
-- `*_ms` Unix ms, `*_ppm` ratios at 1e6).
--
-- Every row of `profiles` and `tip_titles` comes from a `PUT /profile` whose ed25519 signature
-- by `wallet` the desk verified (src/profiles/). Nothing on chain knows these names.

-- One handle per wallet. `^[a-z0-9_]{3,20}$` is checked by the desk before the write; the
-- NOCASE unique index makes uniqueness case-insensitive even for a row written another way.
CREATE TABLE IF NOT EXISTS profiles (
    wallet          TEXT PRIMARY KEY,               -- base58
    handle          TEXT NOT NULL,
    handle_set_at   INTEGER NOT NULL,               -- when the current handle was set (`since`)
    created_at      INTEGER NOT NULL,               -- first handle
    updated_ms      INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS profiles_handle ON profiles (handle COLLATE NOCASE);

-- A handle given up by a change stays held for its previous wallet for 30 days, so nobody can
-- pick up a known tipster's old name the moment they rename (anti-squatting).
CREATE TABLE IF NOT EXISTS profile_released_handles (
    handle          TEXT NOT NULL,
    wallet          TEXT NOT NULL,                  -- the wallet that gave it up
    released_at     INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS profile_released_handle ON profile_released_handles
    (handle COLLATE NOCASE);

-- Signatures already accepted (replay guard over the ±300 s window); pruned after an hour.
CREATE TABLE IF NOT EXISTS profile_sigs (
    sig             TEXT PRIMARY KEY,               -- base64 as sent
    wallet          TEXT NOT NULL,
    used_ms         INTEGER NOT NULL
);

-- A slip title, set once per own tip by its tipster before the tip's first leg starts.
CREATE TABLE IF NOT EXISTS tip_titles (
    ticket          TEXT PRIMARY KEY,               -- the own tip's Ticket PDA
    wallet          TEXT NOT NULL,                  -- its tipster (= tickets.user)
    title           TEXT NOT NULL,                  -- trimmed, 1-40 characters, no control chars
    set_at          INTEGER NOT NULL
);

-- EUR per USD as fetched (frankfurter, ECB reference rate; U-4, D-303), one row per ECB date.
CREATE TABLE IF NOT EXISTS fx_rates (
    pair            TEXT NOT NULL,                  -- 'USD/EUR'
    as_of           TEXT NOT NULL,                  -- the rate's date, YYYY-MM-DD
    rate_ppm        INTEGER NOT NULL,               -- EUR per USD × 1e6
    source          TEXT NOT NULL,                  -- 'ecb'
    fetched_ms      INTEGER NOT NULL,
    raw             TEXT NOT NULL,
    PRIMARY KEY (pair, as_of)
);
