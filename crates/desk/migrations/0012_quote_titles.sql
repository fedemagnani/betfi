-- A tip's name chosen in the bet drawer ("Share as a tip"), sent with the app's last
-- `POST /quote` before signing (`title` + `wallet`). The indexer copies it into `tip_titles`
-- when an own tip opened with that quote by that wallet lands; nothing else reads it.
-- Keyed by wallet too: a quote's bytes carry no user, so two wallets can hold the same quote.
CREATE TABLE IF NOT EXISTS quote_titles (
    quote_hash      TEXT NOT NULL,                  -- quotes.quote_hash
    wallet          TEXT NOT NULL,                  -- base58, the wallet that asked
    title           TEXT NOT NULL,                  -- trimmed, 1-40 characters, no control chars
    created_ms      INTEGER NOT NULL,
    PRIMARY KEY (quote_hash, wallet)
);
