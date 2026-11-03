-- Tipsters, copies, House and the pause mirror (plan step 4.A). Additive only: new tables and
-- indexes; 0001-0005 stay as applied. Conventions as in 0001_desk.sql.
--
-- Tipster metrics (spec section 6) are computed on request from `tickets` / `ticket_legs`
-- (src/tipsters/metrics.rs): own tips are rows with `tipster = user AND copied_from IS NULL`,
-- copies rows with `copied_from IS NOT NULL` (their `tipster` is the original's user, 2.F).
-- These indexes serve those scans.
CREATE INDEX IF NOT EXISTS tickets_tipster ON tickets (tipster, state);
CREATE INDEX IF NOT EXISTS tickets_copied_from ON tickets (copied_from);

-- The tipster's share of each copy fee (`TipsterPaid`, emitted by `finalize` of a copy), one row
-- per copy ticket. `source = 'event'` comes from the authenticated event; `'derived'` is the
-- resync fallback when the listener missed the finalize transaction: floor(Ticket.copy_fee_u x
-- Config.tipster_share_bps / 1e4), the program's own rounding (D-111). An event row replaces a
-- derived one, never the other way round.
CREATE TABLE IF NOT EXISTS tipster_paid (
    ticket      TEXT PRIMARY KEY,               -- the copy's Ticket PDA
    tipster     TEXT NOT NULL,                  -- paid wallet (the original's user)
    amount_u    INTEGER NOT NULL,
    source      TEXT NOT NULL CHECK (source IN ('event', 'derived')),
    tx          TEXT,                           -- the finalize signature (event rows)
    updated_ms  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS tipster_paid_tipster ON tipster_paid (tipster);

-- The latest `Config` and `Vault` the indexer read (one row, id = 1): at every resync and after
-- every transaction carrying `ConfigUpdated`. `paused` is mirrored into `AppState::paused`
-- after this row is written (D-121); `/house` falls back to the vault numbers here when a fresh
-- read fails.
CREATE TABLE IF NOT EXISTS chain_config (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    paused              INTEGER NOT NULL,
    copy_fee_bps        INTEGER NOT NULL,
    tipster_share_bps   INTEGER NOT NULL,
    reserve_u           INTEGER NOT NULL,
    vault_free_u        INTEGER,
    vault_locked_u      INTEGER,
    slot                INTEGER NOT NULL,
    updated_ms          INTEGER NOT NULL
);
