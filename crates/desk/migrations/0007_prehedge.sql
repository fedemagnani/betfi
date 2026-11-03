-- Drift pre-hedge (plan step 4.C, spec section 3 "Rebalance triggers", D-029). Conventions as in
-- 0001_desk.sql. Additive only: a new table of 4.C's own.
--
-- One row per ticket leg that the pre-hedge trigger added to the hedged set (an unhedged later
-- leg whose side got dearer by more than 5 points since the quote). Written BEFORE the order that
-- buys it, so a restarted desk keeps the leg in the hedged set (its target is not forgotten) and
-- never fires the trigger for it again (it is not repeated).

CREATE TABLE IF NOT EXISTS hedge_prehedges (
    ticket           TEXT NOT NULL,                  -- Ticket PDA
    leg_idx          INTEGER NOT NULL,
    symbol           TEXT NOT NULL,
    side             TEXT NOT NULL CHECK (side IN ('long', 'short')),
    quote_price_u    INTEGER NOT NULL,               -- p_i of the signed quote (user's side)
    trigger_price_u  INTEGER NOT NULL,               -- user's side mid that fired it
    source           TEXT NOT NULL CHECK (source IN ('ws', 'rest')),  -- book the trigger read
    ts_ms            INTEGER NOT NULL,               -- desk clock, Unix ms
    PRIMARY KEY (ticket, leg_idx)
);
CREATE INDEX IF NOT EXISTS hedge_prehedges_symbol ON hedge_prehedges (symbol);
