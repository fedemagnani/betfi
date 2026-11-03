-- Schedina desk schema, MVP tables of spec section 6 (step 1.E). SQLite at DESK_DB (D-017).
--
-- Conventions (from the sibling's 0001_ingestion.sql, adapted to SQLite):
--   *_u      micro-units INTEGER: USDC amounts, prices and probabilities (1.0 = 1_000_000), never
--            rounded on the way in (l1-contracts C1)
--   *_ppm    ratios and rates at the same scale (taker fee 0.02 = 20_000)
--   *_ms     Unix milliseconds INTEGER (off-chain times); *_ts / *_at Unix seconds (on-chain times)
--   seq      Pascal's matching-engine sequence of the data a row came from
--   raw      the JSON object as served (TEXT holding JSON), beside the typed columns
--   booleans INTEGER 0/1
-- Writers use idempotent upserts keyed as below, so restarts and replays never duplicate a row.
--
-- Migration slots: 0001_* belongs to step 1.E. 2.A adds 0002_quoter.sql, 2.B 0003_hedge.sql,
-- 2.C 0004_chain.sql. A later step only adds files (new tables, new columns); it never edits an
-- applied migration, since sqlx checks each file's checksum at start.

-- NBA moneylines from GET /markets (catalogue, every 60 s). Resolution columns are written by the
-- crank (2.C) from POST /markets/search; the catalogue's upsert never touches them.
CREATE TABLE IF NOT EXISTS markets (
    symbol                  TEXT PRIMARY KEY,
    game_id                 TEXT NOT NULL,          -- display_attributes.game_page.game_id (D-008)
    sport                   TEXT NOT NULL,          -- 'nba'
    event                   TEXT NOT NULL,          -- symbol before the '.', e.g. NBA_26OCT28_PHIIND_ML
    event_desc              TEXT NOT NULL,
    market_desc             TEXT NOT NULL,
    away                    TEXT NOT NULL,
    away_abbr               TEXT NOT NULL,
    home                    TEXT NOT NULL,
    home_abbr               TEXT NOT NULL,
    long_team               TEXT NOT NULL CHECK (long_team IN ('away', 'home')),  -- spec `outcome`
    start_ms                INTEGER NOT NULL,       -- expected_event_start_time_ms
    expected_resolution_ms  INTEGER NOT NULL,
    tick_size_min_u         INTEGER NOT NULL,       -- D-009: tick and fee parameters from /markets
    tick_sig_figs           INTEGER NOT NULL,
    taker_fee_rate_ppm      INTEGER NOT NULL,
    maker_rebate_share_ppm  INTEGER NOT NULL,
    polymarket_slug         TEXT,
    mark_price_u            INTEGER,
    best_bid_u              INTEGER,
    best_ask_u              INTEGER,
    open_interest           INTEGER,
    live                    INTEGER NOT NULL DEFAULT 1,  -- in the latest GET /markets list
    resolved_price_u        INTEGER,                -- resolution.price (1_000_000 = long side won)
    resolution_seq          INTEGER,
    resolved_ms             INTEGER,
    seq                     INTEGER,
    first_seen_ms           INTEGER NOT NULL,
    updated_ms              INTEGER NOT NULL,
    raw                     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS markets_game ON markets (game_id);
CREATE INDEX IF NOT EXISTS markets_open ON markets (start_ms) WHERE resolved_price_u IS NULL;

-- Book tops and depth for symbols with open interest (quoter, every 10 s). Depth is in contracts
-- over the price window `depth_window_u` from the touch (D-021: 1-2 cents, fixed by 1.B / 2.A).
CREATE TABLE IF NOT EXISTS book_snapshots (
    symbol          TEXT NOT NULL,
    ts_ms           INTEGER NOT NULL,
    best_bid_u      INTEGER,
    best_ask_u      INTEGER,
    bid_depth       INTEGER,
    ask_depth       INTEGER,
    depth_window_u  INTEGER,
    seq             INTEGER,
    raw             TEXT,
    PRIMARY KEY (symbol, ts_ms)
);

-- One row per schedina, keyed by the Ticket PDA ["ticket", user, nonce], which is derivable from
-- the Pending ["pending", user, nonce] as soon as it is opened: the row exists from state
-- 'pending' and keeps its key through mint, settle, cash-out or refund (indexer, 2.C).
CREATE TABLE IF NOT EXISTS tickets (
    ticket                  TEXT PRIMARY KEY,       -- Ticket PDA, base58
    pending                 TEXT NOT NULL UNIQUE,   -- Pending PDA, base58
    user                    TEXT NOT NULL,
    nonce                   INTEGER NOT NULL,
    state                   TEXT NOT NULL
                            CHECK (state IN ('pending', 'open', 'settled', 'cashed_out', 'refunded')),
    n                       INTEGER NOT NULL,
    stake_u                 INTEGER NOT NULL,       -- S escrowed by open_pending
    rs_u                    INTEGER,                -- r * S taken at finalize
    r_ppm                   INTEGER,
    payout_u                INTEGER NOT NULL,       -- offered at open_pending, final after finalize
    odds_offered_u          INTEGER NOT NULL,
    odds_fair_u             INTEGER,
    fee_u                   INTEGER,                -- fair payout - offered payout
    min_payout_per_unit_u   INTEGER NOT NULL,
    copy_fee_u              INTEGER NOT NULL DEFAULT 0,
    tipster                 TEXT,
    copied_from             TEXT,
    quote_expires_at        INTEGER,
    expires_at              INTEGER,                -- the Pending's expiry, Unix s
    opened_at               INTEGER,
    minted_at               INTEGER,
    settled_at              INTEGER,
    paid_u                  INTEGER,
    tx_open_pending         TEXT,
    tx_finalize             TEXT,
    tx_settle               TEXT,
    tx_refund               TEXT,
    tx_cash_out             TEXT,
    slot                    INTEGER,                -- slot of the last on-chain change applied
    updated_ms              INTEGER NOT NULL,
    UNIQUE (user, nonce)
);
CREATE INDEX IF NOT EXISTS tickets_user ON tickets (user, nonce);
CREATE INDEX IF NOT EXISTS tickets_state ON tickets (state);

CREATE TABLE IF NOT EXISTS ticket_legs (
    ticket          TEXT NOT NULL REFERENCES tickets (ticket) ON DELETE CASCADE,
    idx             INTEGER NOT NULL,
    symbol          TEXT NOT NULL,
    game_id         TEXT NOT NULL,
    side            TEXT NOT NULL CHECK (side IN ('long', 'short')),
    pick            TEXT NOT NULL CHECK (pick IN ('away', 'home')),
    start_ms        INTEGER NOT NULL,
    p_quote_u       INTEGER NOT NULL,               -- ask-side quote price signed in the quote
    p_fill_u        INTEGER,                        -- achieved average fill, or p_quote_u if unhedged
    m1_ppm          INTEGER NOT NULL,
    overlaps        INTEGER NOT NULL DEFAULT 0,
    hedged          INTEGER NOT NULL DEFAULT 0,
    delta_0         INTEGER,                        -- contracts targeted at inception (signed)
    status          TEXT NOT NULL DEFAULT 'open'
                    CHECK (status IN ('open', 'won', 'lost', 'half', 'void')),
    resolution_u    INTEGER,                        -- r of resolve_leg (Pascal resolution.price)
    resolved_ms     INTEGER,
    tx_resolve      TEXT,
    PRIMARY KEY (ticket, idx)
);
CREATE INDEX IF NOT EXISTS ticket_legs_symbol ON ticket_legs (symbol, status);

-- Hedger fills (desk, 2.B), from the order response, the account channel or /fills replay. Mock
-- and Pascal trade ids live in different id spaces, hence the venue in the key (D-007).
CREATE TABLE IF NOT EXISTS pascal_fills (
    venue                TEXT NOT NULL CHECK (venue IN ('pascal', 'mock')),
    trade_id             INTEGER NOT NULL,
    symbol               TEXT NOT NULL,
    side                 TEXT NOT NULL CHECK (side IN ('bid', 'ask')),
    price_u              INTEGER NOT NULL,
    size                 INTEGER NOT NULL,
    liquidity            TEXT NOT NULL CHECK (liquidity IN ('maker', 'taker')),
    fee_u                INTEGER NOT NULL,           -- fee_usd (negative = rebate)
    builder_fee_u        INTEGER NOT NULL DEFAULT 0, -- builder_fee_paid_usd
    collateral_change_u  INTEGER NOT NULL,           -- collateral_change_usd
    realized_pnl_u       INTEGER,
    position_size_prev   INTEGER,
    order_id             INTEGER,
    client_order_id      INTEGER NOT NULL,
    reduce_only          INTEGER,
    seq                  INTEGER NOT NULL,
    ts_ms                INTEGER NOT NULL,           -- trade_ts_ms
    signed_payload       TEXT,                       -- the signed permit the order carried (hex)
    raw                  TEXT NOT NULL,
    PRIMARY KEY (venue, trade_id)
);
CREATE INDEX IF NOT EXISTS pascal_fills_seq ON pascal_fills (venue, seq);
CREATE INDEX IF NOT EXISTS pascal_fills_order ON pascal_fills (client_order_id);

-- Each fill split pro-rata over the ticket legs its order served (desk, 2.B). No foreign keys:
-- the desk may record a fill before the indexer has written the ticket row.
CREATE TABLE IF NOT EXISTS fill_allocations (
    venue       TEXT NOT NULL,
    trade_id    INTEGER NOT NULL,
    ticket      TEXT NOT NULL,
    leg_idx     INTEGER NOT NULL,
    contracts   INTEGER NOT NULL,                    -- signed: + buys long exposure
    PRIMARY KEY (venue, trade_id, ticket, leg_idx)
);
CREATE INDEX IF NOT EXISTS fill_allocations_ticket ON fill_allocations (ticket, leg_idx);

-- Desk target vs held per symbol at each rebalance (desk, 2.B).
CREATE TABLE IF NOT EXISTS hedge_positions (
    symbol      TEXT NOT NULL,
    ts_ms       INTEGER NOT NULL,
    target      INTEGER NOT NULL,
    held        INTEGER NOT NULL,
    reason      TEXT NOT NULL,
    PRIMARY KEY (symbol, ts_ms)
);

CREATE TABLE IF NOT EXISTS cashouts (
    ticket           TEXT PRIMARY KEY,
    c_u              INTEGER NOT NULL,
    unresolved_legs  INTEGER NOT NULL,
    ts               INTEGER NOT NULL,              -- signed quote ts, Unix s
    tx               TEXT,
    updated_ms       INTEGER NOT NULL
);

-- The money-flow ticker (`/stream` feed frames), written by whichever service emits the item.
CREATE TABLE IF NOT EXISTS feed_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL CHECK (kind IN ('fill', 'mint', 'resolve', 'settle', 'cashout')),
    text        TEXT NOT NULL,
    ticket      TEXT,
    payload     TEXT,
    ts_ms       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS feed_events_ts ON feed_events (ts_ms);

-- Resume points of the background jobs (last processed Pascal seq, last Solana slot, ...), one
-- row per job name, so each L2 service restarts where it stopped without a table of its own.
CREATE TABLE IF NOT EXISTS cursors (
    name        TEXT PRIMARY KEY,
    value       INTEGER NOT NULL,
    updated_ms  INTEGER NOT NULL
);
