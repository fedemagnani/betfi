-- Signed quotes (l2-contracts L2-1). Written by the quoter (2.A) before a quote leaves the
-- desk; read by the indexer (2.C) to turn the hashes of a PendingOpened event back into
-- Pascal symbols and game ids, and by the hedge desk (2.B) for the signed p_max per leg.
-- The chain stores only sha256(symbol) and sha256(game_id) (D-008).

CREATE TABLE IF NOT EXISTS quotes (
    -- hex sha256 of the exact signed message bytes (== Pending.quote_hash on-chain)
    quote_hash       TEXT    PRIMARY KEY,
    -- the signed C3 message bytes and the ed25519 signature
    msg              BLOB    NOT NULL,
    sig              BLOB    NOT NULL,
    quote_signer     TEXT    NOT NULL,
    n                INTEGER NOT NULL,
    stake_u          INTEGER NOT NULL,
    odds_offered_u   INTEGER NOT NULL,
    odds_fair_u      INTEGER NOT NULL,
    payout_u         INTEGER NOT NULL,
    fee_u            INTEGER NOT NULL,
    min_payout_per_unit_u INTEGER NOT NULL,
    quote_expires_at INTEGER NOT NULL,      -- Unix s
    copied_from      TEXT,                  -- original Ticket PDA (v1), else NULL
    -- JSON array of api::types::QuoteLeg in ticket order (symbol, game_id, side, pick,
    -- start_ms, price_u, p_max_u, m1_ppm, overlaps)
    legs_json        TEXT    NOT NULL,
    created_ms       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS quotes_created ON quotes (created_ms);

-- One row per leg, so a hash can be resolved without parsing legs_json.
CREATE TABLE IF NOT EXISTS quote_legs (
    quote_hash   TEXT    NOT NULL REFERENCES quotes (quote_hash),
    idx          INTEGER NOT NULL,
    symbol       TEXT    NOT NULL,
    symbol_hash  TEXT    NOT NULL,          -- hex sha256(symbol)
    game_id      TEXT    NOT NULL,
    game_id_hash TEXT    NOT NULL,          -- hex sha256(game_id)
    PRIMARY KEY (quote_hash, idx)
);

CREATE INDEX IF NOT EXISTS quote_legs_symbol_hash ON quote_legs (symbol_hash);
