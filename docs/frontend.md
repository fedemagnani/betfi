# Frontend

**BetFi**: a phone-first web app in Rust, compiled to wasm. Dark only.

## Dependencies

| Crate | Use |
| --- | --- |
| `dioxus` 0.7 | UI, routing, state |
| `dioxus-solana` | wallet connection (any Wallet Standard wallet) |
| `gloo-net` | desk HTTP and the live WebSocket feed |
| `schedina-tx` (ours) | builds every transaction, tested against the program's IDL |
| `solana-*`, `sha2` | transaction types, quote hash checks |

## Rules

- Every number is the desk's or the chain's. The app never computes a price.
- Before signing, the app checks the desk's signed quote against what the screen shows.
- Euros = USDC × the ECB rate. "≈ SOL" uses the live SOL price.

## Screens

Sportsbook, bet drawer, event detail, tipster profile, copy, My Bets, Account. Successes get a
celebration screen with a Solscan link.
