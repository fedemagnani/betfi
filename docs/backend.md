# Backend

Rust, on `tokio` and `axum`. State in SQLite (`sqlx`).

## Dependencies

| Crate | Use |
| --- | --- |
| `tokio`, `axum`, `tower-http` | async runtime, HTTP and WebSocket server |
| `sqlx` (SQLite) | tickets, quotes, signed orders, profiles |
| `reqwest`, `tokio-tungstenite` | Pascal REST and WebSocket |
| `rust_decimal` | prices without float error |
| `ed25519-dalek` | quote, cash-out and order signatures |
| `solana-*` | transactions for the crank |
| `pascal-rs` (ours) | Pascal client, order signing with our builder code, `pascal-cli` |
| `pricing` (ours) | pure math: book walks, margin, floor, finalize, cash-out, deltas |

## Desk

One binary (`crates/desk`) serving the app:

- **catalogue**: Pascal's markets every 60 s, turned into the day's card.
- **quoter**: walks the books, prices, signs quotes (10 s) and cash-out offers (30 s).
- **hedge desk**: roll, ladder, pre-hedge, unwind. Stores every signed order before sending it.
- **indexer**: reads program events and accounts.
- **crank**: signs with the operator's key, the only one the program accepts for `finalize`.
  Sends `finalize` once the hedge fills, `resolve_leg` from Pascal's results, `settle`, and
  refunds on a timer.
- **relay**: co-signs users' transactions with the sponsor key and pays their fees.
- **tipsters, profiles, sentiment, fx**: the social side, and euros and SOL prices.

The modules share one process and talk over an in-process event bus (`PendingOpened`,
`HedgeFilled`, `TicketMinted`…).

The `crank` binary in the same crate is the operator's command line, not a service:
`init-config`, `fund-vault`, `faucet`, `pause`, and `amm-peg`, which keeps the mockSOL pool at
the real SOL price.

## pascal-mock

Pascal has no devnet. `pascal-mock` speaks Pascal's exact API, so the desk can't tell the
difference.

- Reads real catalogue, books and results from Pascal (`--live`), or replays a recorded night
  (`--tape`).
- Fills the desk's signed orders against those books, and checks every signature first.
- Forces a result on demand (`/mock/resolve`), so a demo doesn't wait for the game.

No order has ever reached Pascal: no Pascal account is funded. Going live is configuration
(`PASCAL_TRADE_URL`), not code. [How to verify a signed batch](demo.md#signed-not-sent).
