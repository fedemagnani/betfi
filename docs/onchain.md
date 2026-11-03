# On-chain

Two Anchor 1.2.1 programs on Solana devnet.

## schedina

The bookmaker: escrow, vault, tickets.

| Instruction | Who | What |
| --- | --- | --- |
| `open_pending` | user | Escrows the stake. Checks the desk's signed quote on chain (Ed25519). |
| `finalize` | crank | Mints the ticket at the hedge's fills, locks the payout, refunds the unfilled part. |
| `refund_pending` | anyone | Returns the stake 15 s after an unfinalised bet expires. |
| `resolve_leg` | crank | Sets one leg's result. |
| `settle` | anyone | Pays the winner once every leg is resolved. |
| `cash_out` | user | Pays a desk-signed offer, valid 30 s. |

Invariant: the vault always holds every open ticket's payout.

Program [`HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW`](https://explorer.solana.com/address/HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW?cluster=devnet).

## mock-amm

Constant-product pools: mockSOL and mockBONK against mock USDC. 0.5% fee. Faucets for test
money. `crank amm-peg` keeps the mockSOL pool at the real SOL price.

Program [`5WhsDY2XjzJoWDbe1AgJnnNgJccQwte9TgsvG4yRtHbf`](https://explorer.solana.com/address/5WhsDY2XjzJoWDbe1AgJnnNgJccQwte9TgsvG4yRtHbf?cluster=devnet).

## Mock aggregator

Stands in for Jupiter. The desk's `/tokens` and `/swap-quote` answer in Jupiter's shape, from
the mock-amm pools. A bet in SOL is one transaction: `[swap, Ed25519 verify, open_pending]`.
The swap and the stake land together or not at all. On mainnet, Jupiter replaces the mock.
