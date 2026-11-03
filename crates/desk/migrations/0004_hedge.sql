-- Hedge desk (plan step 2.B). Conventions as in 0001_desk.sql.
--
-- Every order the hedge desk signs is written here BEFORE it is sent (write-ahead), with the exact
-- signed JSON request, the permit bytes and the signature: this is what mode M shows on screen
-- (spec section 1 "Signing"), and what lets a restarted desk map a fill replayed from
-- /accounts/{hedger}/fills back to the ticket legs its order served (spec section 5, idempotency).

CREATE TABLE IF NOT EXISTS hedge_orders (
    client_order_id     INTEGER PRIMARY KEY,           -- desk-wide u64 counter (fits i64)
    venue               TEXT NOT NULL CHECK (venue IN ('pascal', 'mock')),
    kind                TEXT NOT NULL,                 -- inception, excess, roll_post, roll_ioc,
                                                       -- unwind, rebalance, flatten
    reason              TEXT NOT NULL,                 -- why (pending_opened, lost_leg, cash_out, ...)
    pending             TEXT,                          -- Pending PDA for inception / refund unwinds
    symbol              TEXT NOT NULL,
    side                TEXT NOT NULL CHECK (side IN ('bid', 'ask')),
    price_u             INTEGER NOT NULL,              -- limit price on the symbol (long side)
    size                INTEGER NOT NULL,
    tif                 TEXT NOT NULL,                 -- gtc, gtt, ioc
    post_only           INTEGER NOT NULL DEFAULT 0,
    reduce_only         INTEGER NOT NULL DEFAULT 0,
    builder_pubkey      TEXT,
    builder_fee_rate_u  INTEGER,
    request_json        TEXT NOT NULL,                 -- the signed POST /orders element
    permit_hex          TEXT NOT NULL,                 -- the 232 signed permit bytes
    signature           TEXT NOT NULL,                 -- base58
    status              TEXT NOT NULL DEFAULT 'signed' -- signed, placed, rejected, error, cancelled
                        CHECK (status IN ('signed', 'placed', 'rejected', 'error', 'cancelled')),
    error               TEXT,
    order_id            INTEGER,
    size_filled         INTEGER,
    notional_filled_u   INTEGER,
    placed_ms           INTEGER NOT NULL,
    updated_ms          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS hedge_orders_symbol ON hedge_orders (symbol, placed_ms);
CREATE INDEX IF NOT EXISTS hedge_orders_pending ON hedge_orders (pending);

-- The persisted order_map: which ticket legs an order served and how many signed contracts each
-- asked for. Fills are split pro-rata over these rows into fill_allocations.
CREATE TABLE IF NOT EXISTS hedge_order_serves (
    client_order_id  INTEGER NOT NULL,
    ticket           TEXT NOT NULL,
    leg_idx          INTEGER NOT NULL,
    contracts        INTEGER NOT NULL,                 -- signed: + buys long exposure
    PRIMARY KEY (client_order_id, ticket, leg_idx)
);
CREATE INDEX IF NOT EXISTS hedge_order_serves_ticket ON hedge_order_serves (ticket);
