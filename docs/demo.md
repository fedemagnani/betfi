# How to demo it

Needs Rust, the Solana and Anchor CLIs, `dx` 0.7, and Node with Playwright for the browser
checks. Key paths go in the gitignored `.env`.

| Command | What it does |
| --- | --- |
| `just demo` | The whole stack on devnet: mock, desk, app on `http://localhost`, opened in your browser once it is up (`--no-open` to skip). Every game Pascal lists that isn't over, grouped by day; falls back to the recorded 28 Oct night only if no game (NBA or soccer) is left to start. Ctrl-C stops it. |
| `just demo --replay` | Same, on the recorded 28 Oct NBA night at 60× speed. |
| `just demo --localnet` | Same, on a private local validator instead of devnet. |
| `just demo --e2e` | Starts everything, clicks through a full bet in a browser, checks every number, stops. |
| `just e2e` | The full lifecycle on localnet, no browser: two tickets, one cashed out, one settled. |
| `just sign-demo` | Signs one hedge batch for Pascal, verifies it, sends it only to the mock. |
| `just check` | Format, lint and tests for backend, programs and app. |
| `just phone-gate` | Every screen at phone and desktop width in a headless browser. |
| `just visual` | Screens next to the design mockup, with a difference score. |
| `just deploy-devnet` | Builds and deploys `schedina` to devnet. Refuses mainnet. |
| `just crank-status-devnet` | Reads the vault, pending bets and tickets on devnet. |
| `just docs` | Builds this book and opens it. |

## Resolving a game

`just demo` prints one command per game. It forces the result on the mock:

```sh
curl -s -X POST http://127.0.0.1:<mock port>/mock/resolve \
  -H 'content-type: application/json' -d '{"symbol": "NBA_26OCT29_CLEATL_ML.CLE", "price": "1"}'
```

`"1"` means the symbol's side won (for an NBA winner, the away team). The crank then settles
every ticket holding it.

## Paying in SOL

Peg the mockSOL pool to the real price first. Repeat until it prints `nothing sent`:

```sh
cargo run -q --locked -p desk --bin crank -- --url devnet \
  --authority "$AUTHORITY_KEYPAIR_PATH" \
  amm-peg --from-fx --desk http://127.0.0.1:<desk port> --yes
```

## Signed, not sent

[`signed-batch.json`](signed-batch.json) is a real hedge batch the desk signed for Pascal.
Check it:

```sh
cargo run -q -p pascal-rs --bin pascal-cli -- verify-order docs/signed-batch.json \
  --signer 5ggBcWJNqYzf4jPNQu2tsEQWNDak2v3b3Vtgmsb4dQ5S \
  --builder 7FJuR9h5skiCySUZTXvfqgUoZcmpu6wQy9oSzYJ24gC5 --builder-fee-rate 0.001
# 3 request(s), 3 verified, 0 failed
```
