-- Desk data for the v2 screens (plan-ui2 7.0c, l7-contracts L7-1). Additive: new columns, new
-- tables, and `feed_events` rebuilt only to widen its `kind` check with 'copy' (SQLite cannot
-- alter a CHECK in place; rows, ids and indexes are carried over). Conventions as in
-- 0001_desk.sql (`*_ms` Unix ms, `*_at` Unix s).

-- Where and how fast each ticket transition landed, as the indexer saw it (`Ticket.tx.slot`,
-- `Ticket.tx.confirm_ms`). Written once, from the transaction that made the transition: a
-- resync that only finds the state leaves them NULL. `confirm_*_ms` = the indexer's own clock
-- when it applied the transaction − the block time (whole seconds, so ±1 s); NULL when the
-- block time is unknown.
ALTER TABLE tickets ADD COLUMN slot_open_pending INTEGER;
ALTER TABLE tickets ADD COLUMN slot_settle INTEGER;
ALTER TABLE tickets ADD COLUMN slot_cash_out INTEGER;
ALTER TABLE tickets ADD COLUMN confirm_open_pending_ms INTEGER;
ALTER TABLE tickets ADD COLUMN confirm_cash_out_ms INTEGER;

-- The ticker gains kind 'copy' (a minted copy of a tip; plain mints stay 'mint').
CREATE TABLE feed_events_v2 (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL
                CHECK (kind IN ('fill', 'mint', 'copy', 'resolve', 'settle', 'cashout')),
    text        TEXT NOT NULL,
    ticket      TEXT,
    payload     TEXT,
    ts_ms       INTEGER NOT NULL,
    dedupe      TEXT
);
INSERT INTO feed_events_v2 (id, kind, text, ticket, payload, ts_ms, dedupe)
    SELECT id, kind, text, ticket, payload, ts_ms, dedupe FROM feed_events;
DROP TABLE feed_events;
ALTER TABLE feed_events_v2 RENAME TO feed_events;
CREATE INDEX IF NOT EXISTS feed_events_ts ON feed_events (ts_ms);
CREATE UNIQUE INDEX IF NOT EXISTS feed_events_dedupe ON feed_events (dedupe) WHERE dedupe IS NOT NULL;

-- Social links of a profile (`PUT /profile` with `links`, signed by `wallet`): bare handles or
-- usernames checked per network by the desk, never URLs. A `links` write replaces all four.
CREATE TABLE IF NOT EXISTS profile_links (
    wallet          TEXT PRIMARY KEY,
    x               TEXT,
    youtube         TEXT,
    telegram        TEXT,
    instagram       TEXT,
    updated_ms      INTEGER NOT NULL
);

-- Who follows whom (`PUT /profile` with `follow`, signed by `follower`).
CREATE TABLE IF NOT EXISTS follows (
    follower        TEXT NOT NULL,
    followee        TEXT NOT NULL,
    created_ms      INTEGER NOT NULL,
    PRIMARY KEY (follower, followee)
);
CREATE INDEX IF NOT EXISTS follows_followee ON follows (followee);
