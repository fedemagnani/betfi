-- Quoter (plan step 2.A). Additive only: the 0001 / 0002 tables are frozen (l2-contracts L2-0).

-- The venue's clock of each snapshot. `ts_ms` is the desk's receive time (the 10 s cadence);
-- this is Pascal's `state_time_ms` of the /books response (the tape time in mode M, D-033).
ALTER TABLE book_snapshots ADD COLUMN state_time_ms INTEGER;

-- Every signed cash-out quote (GET /cashout-quote/:ticket), written before the response leaves,
-- so a `cash_out` the program accepts can be matched to what the desk signed.
CREATE TABLE IF NOT EXISTS cashout_quotes (
    quote_hash    TEXT    PRIMARY KEY,      -- lowercase hex sha256 of the 99 signed bytes
    ticket        TEXT    NOT NULL,         -- Ticket PDA, base58
    c_u           INTEGER NOT NULL,
    ts            INTEGER NOT NULL,         -- signed ts, Unix s (valid 30 s on-chain)
    msg           BLOB    NOT NULL,
    sig           BLOB    NOT NULL,
    quote_signer  TEXT    NOT NULL,
    created_ms    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS cashout_quotes_ticket ON cashout_quotes (ticket, ts);
