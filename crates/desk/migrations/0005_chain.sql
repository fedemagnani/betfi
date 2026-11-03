-- Chain columns and crank bookkeeping (step 2.C: crank and indexer). Additive only (L2-0):
-- new columns on 1.E's tables, new tables of 2.C's own.

-- Ticket columns the indexer reads from the Pending / Ticket accounts.
ALTER TABLE tickets ADD COLUMN quote_hash TEXT;             -- hex sha256 of the signed quote (L2-1)
ALTER TABLE tickets ADD COLUMN is_tip INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tickets ADD COLUMN refund_u INTEGER;            -- S - rS returned at finalize
ALTER TABLE tickets ADD COLUMN odds_u INTEGER;              -- P / rS once minted (Ticket.odds_u)
ALTER TABLE tickets ADD COLUMN chain_fee_u INTEGER;         -- Ticket.fee_u = rS * m(n), for the books
-- 0 when a leg's symbol or game id could not be named from the quote store or the catalogue
-- (L2-1): the legs then carry 'sha256:<hex>' and the desk does not hedge the pending.
ALTER TABLE tickets ADD COLUMN names_resolved INTEGER NOT NULL DEFAULT 1;

ALTER TABLE ticket_legs ADD COLUMN symbol_hash TEXT;         -- hex sha256(symbol), as on-chain
ALTER TABLE ticket_legs ADD COLUMN game_id_hash TEXT;        -- hex sha256(game_id)

-- Replay-safe feed items: one row per (kind, ticket[, leg]) key, written with INSERT OR IGNORE.
ALTER TABLE feed_events ADD COLUMN dedupe TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS feed_events_dedupe ON feed_events (dedupe) WHERE dedupe IS NOT NULL;

-- Pascal resolutions the crank has seen (search poll, resolved sweep, market_update), one row
-- per symbol: the first insert is what emits MarketResolved, so a restart never announces twice
-- and still sends resolve_leg for legs left open.
CREATE TABLE IF NOT EXISTS market_resolutions (
    symbol          TEXT PRIMARY KEY,
    price_u         INTEGER NOT NULL,                -- resolution.price (1_000_000 = long side won)
    resolved_ms     INTEGER NOT NULL,                -- market_resolution_ts_ms
    seq             INTEGER,
    source          TEXT NOT NULL CHECK (source IN ('search', 'resolved', 'market_update')),
    seen_ms         INTEGER NOT NULL
);

-- Every transaction the crank sends or gives up on (finalize, refund, resolve, settle), so a
-- FinalizeRejected on the bus always has a row behind it, and ops can see what the crank did.
CREATE TABLE IF NOT EXISTS crank_ops (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    op          TEXT NOT NULL CHECK (op IN ('finalize', 'refund', 'resolve', 'settle')),
    target      TEXT NOT NULL,                       -- Pending or Ticket PDA
    leg_idx     INTEGER,
    status      TEXT NOT NULL CHECK (status IN ('confirmed', 'rejected', 'failed')),
    signature   TEXT,
    error       TEXT,
    ts_ms       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS crank_ops_target ON crank_ops (target, op);
