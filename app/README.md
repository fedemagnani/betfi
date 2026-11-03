# Schedina app (shown as BetFi)

Dioxus 0.7 web app (wasm32), mobile-first at 390 px, one column centred at 480 px on wider screens (V-7),
installable as a PWA. The app is branded **BetFi** (U-6); the project keeps the name Schedina.
Standalone crate (its own `[workspace]` and `Cargo.lock`), built by `dx` from this directory.

```sh
just app-check                       # fmt --check + clippy -D warnings on wasm32-unknown-unknown
cd app && dx build --platform web    # dx 0.7.10 matches dioxus 0.7.10 (D-020 closed)
cd app && dx serve --platform web    # http://localhost:8080
```

## Configuration (build time)

| Variable | Effect |
| --- | --- |
| `SCHEDINA_DESK_URL` | Desk HTTP base, e.g. `http://127.0.0.1:3000`. Unset: **demo mode**, every screen runs on the C4 example responses in `fixtures/desk/` and a scripted `/stream` (`fixtures/desk/stream.json`). |
| `SCHEDINA_RPC_URL` | Solana RPC. Unset: devnet's public endpoint. A URL containing `mainnet` is refused (devnet is used and a banner says so); `localhost`/`127.0.0.1` is treated as localnet (WS on port 8900). Before any send, an endpoint whose genesis hash is mainnet-beta's is refused too. |
| `SCHEDINA_DEV_TOOLS` | Enables `/dev/wallet`, `/dev/kit` and `/dev/shell` in release builds (debug builds always have them). |
| `SCHEDINA_DESIGN_FIXTURES` | Set (not `0`): demo mode on `fixtures/design/`, the BetFi C2 mockup's **fictional** content (L7-2: Panathinaikos–Fenerbahçe live, Real Madrid–Olympiacos, Celtics–Knicks, Arsenal–Bayern 1X2, @lorenzo_hoops's three parlays and copiers, the ticker line, €1,240.50, SOL at €135.80), in the L7-1 shapes, for the side-by-side visual review. Never the default; a normal build does not contain these files. |

The variables are read with `option_env!`, so a change needs a rebuild (`dx serve` restarted).

## Layout

- `src/desk/types.rs`: typed C4 shapes; `src/desk/mod.rs`: the desk client (`Desk::Live` over fetch, or `Desk::Demo` on the bundled fixtures).
- `src/state.rs`: the one global `Signal<AppState>`; `src/stream.rs`: the single `/stream` socket (backoff 0.5 s doubling to 30 s with jitter, 45 s silence = dead link, `subscribe` resent on every reconnect, undecodable frames counted and ignored).
- `src/wallet.rs`: the only wallet surface, see below. `src/format.rs`: micro-units to euros and odds (truncated, never rounded up).
- `src/ui/kit/` (v2, 7.0a): the BetFi C2 component kit: `AppBar` (balance pill with the TEST chip), `LiveTicker`, `SportTabs`, `MatchCard`, `OutcomeButton` (lowest value green, highest purple, Solana gradient border when selected), `SlipPill`, `BottomSheet`, `StakeAmount`, `SummaryRows`, `PurpleCTA`, `SecondaryButton`, `CelebrationHero`, `TxBadge`, `PayoutHero`, `StatTile`, `Avatar`, `Badge`, `TeamChips`, `SentimentBar`, `SolscanLink`, `CopierRow`, `ProfileHeader`, `PickRow`, `LegCard`, `LegRow`, `OutcomeCard`, `TipCard`, `BottomNav`, Material Symbols as inline SVG (`SymIcon`); `money.rs` (€ from USDC at `/fx`, ≈ SOL at `eur_per_sol_ppm`). The L6 components stay for the screens not yet redone. Styles and tokens (DESIGN.md's colours, radii, type scale): `assets/main.css` (dark only). Font: Plus Jakarta Sans with tabular figures, bundled in `assets/fonts/` (SIL OFL, `assets/fonts/OFL.txt`); nothing is loaded from a CDN. `/dev/kit` shows the mockup's pieces rebuilt from the kit, then every component in every state.
- `src/ui/mod.rs` + `src/ui/nav.rs` (7.0a): the shell. Top-level screens: the app bar (BETFI, balance € / ≈ SOL with TEST, `+ DEPOSIT`, account) and the live ticker, the screen, the slip pill above the bottom nav (Sports, Tipsters, Account; L8-4: the bets are a section of Account, `/me` redirects to `/account#bets`). Sub-pages (event, slip, copy, receipt, confirm): a back header with the page title. The Live screen and its Twitch player are gone (V-7, guidelines §3: one live ticker).
- `scripts/visual/shoot.mjs` (7.0a): the kit's specimens next to crops of the eight C2 screens, the shell and the step shots at 390 px and at each mockup's frame (scored by pixel difference), and `out/index.html` side by side (`out/` is gitignored; the mockup stays private). `fixtures/design/gen_design.py` regenerates the design fixtures and checks every €/USDC pair.
- `src/ui/`: the L1–L5 screens (restyled from L6-1 on): Card, Tray (with "Share as a tip", `tip_toggle.rs`), Confirm, My schedine, the wallet line with the faucet, the dev-only wallet page, and the v1 screens (4.B):
  - `/ticket/<Ticket PDA>` (`ticket.rs`): the receipt, from My schedine, a tipster's page or a link: picks and prices, money, hedge fills (labelled mock-filled when `venue == mock`), results, every transaction (including each `resolve_leg`) with an explorer link, the settlement terms, and "Copy this schedina".
  - `/tipsters` and `/tipsters/<wallet>` (`tipsters.rs`, charts in `chart.rs`): the board (`GET /tipsters?sort=&all=`) and a tipster's page with the cumulative return and forecast-error charts, open and settled tips.
  - `/account` (`house.rs`, `house/`): wallet, balances, DEPOSIT test faucets, the profile editor and the house's numbers (vault, schedine by state, hedger positions, fee totals and builder revenue from `GET /house`); `/house` redirects there (V-7).
- `src/flows.rs`: the three signed flows (Gioca → `open_pending`, also for tips and copies; Incassa → `cash_out`; faucet → `faucet_usdc`), wallet and desk glue only.
- `tx/` (crate `schedina-tx`, a member of this workspace): every instruction the app sends, PDAs, and the checks that the desk's signed bytes say what the screen shows. No web dependencies; `cargo test -p schedina-tx` runs on the host against `fixtures/schedina/quote_vector.json` (byte equality) and `programs/idl/schedina.json` (account order, signer, writable, PDA seeds).
- `public/`: `manifest.webmanifest`, `sw.js` (network-first for page navigations only), icons. Copied to the site root by `dx`.
- `fixtures/desk/`: verbatim copies of the desk's C4 examples (`../fixtures/desk/`) plus the demo stream script `stream.json` built from its `stream_*.json` frames: `python3 -I fixtures/gen_fixtures.py ../fixtures/desk fixtures/desk`. The L4 routes' examples (`tipsters.json` = the `all=1` answer, `tipster.json`, `house.json`, `copy_quote.json`) were written from L4-2 by 4.B; the generator replaces each with the desk's file of the same shape once the desk has one, whatever its name. Demo mode's "now" for the tip and copy rules is the example card's `as_of_ms`, so the examples stay consistent whatever the date.

## Signed transactions

- Gioca: connect if needed, ask a fresh quote for exactly the Tray's picks and stake, refuse one that pays less than shown, check the signed bytes against the JSON (leg symbol and game-id sha256 and side in order, stake, odds, expiry, floor ≤ offered), then `[Ed25519 verify, open_pending(nonce, min_payout_per_unit_u, expires_at = now + 45, is_tip, copied_from)]`. The nonce is 53 random bits (`crypto.getRandomValues`, `schedina_tx::NONCE_MAX`), so JavaScript readers of the desk's JSON see it exactly. Progress then follows `/stream` `progress` frames for the Pending PDA `["pending", user, nonce LE]`.
- Share as a tip (Tray switch): `is_tip = true`. Live games included (D-660); off, with the reason, only while the stake is paid in another token (D-221); before signing, `schedina-tx` refuses a tip that copies (the program's `TipNotAllowed`). Its cash-out passes the tipster profile (below). On, an optional name field (`#tip-name`, L9) under the switch: the name rides on the fresh quote before signing and becomes the tip's title; "Bet" waits while a typed name is invalid.
- Copy this schedina (receipt of an open tip): `POST /ticket/{id}/copy-quote {"stake_u"}`, shown with the copy fee on its own line; Gioca re-quotes the same way, refuses a higher fee, a lower payout or other legs than the original's (the program's `CopyMismatch`), then `open_pending(copied_from = Some(original))`. The program escrows stake + fee; the crank passes the original and the tipster's USDC account at `finalize` (L2-4). Off, with the reason, unless the original is an open own tip whose games are all still to start and undecided (spec 10).
- Incassa: first tap fetches the signed offer (`/cashout-quote/:ticket`, valid 30 s) and shows its amount; the second signs `[Ed25519 verify, cash_out(msg)]`, plus the `["tipster", user]` profile as remaining account for an own tip (L2-4).
- Pay in another token (the Tray's stake row: one amount field and a token chip, shown when `GET /tokens` lists a mint besides mock USDC): the amount is that token's; `POST /swap-quote` gives `min_out_u`, the desk is quoted with `stake_u = min_out_u`, the swap fee is its own line among the fees, and Gioca signs one v0 transaction `[swap, Ed25519 verify, open_pending]` with the AMM's lookup table (`ui/token_select.rs`, `schedina_tx::swap`). Tips are staked in mock USDC only. "Get test mockBONK" claims from the AMM's faucet.
- Get test USDC: `[create the mock-USDC ATA (idempotent), faucet_usdc(1,000 USDC)]`; the program has `faucet_usdc` in `devnet` builds only.
- The quote must land within its 10 s desk TTL: approve the wallet prompt promptly. Program errors are shown in plain words (`schedina_tx::program_error_words`).

## Wallet (for 2.E)

`dioxus-solana` 0.5.1: `RpcProvider` (devnet) + `WalletProvider` at the root, `WalletButton` in the header. Screens use only
`wallet::use_wallet_api() -> Wallet` (a `Copy` handle):

```rust
async fn connect(&self) -> Result<Pubkey, WalletError>          // Phantom if present, else first Wallet Standard wallet
fn public_key(&self) -> Option<Pubkey>                           // reactive
async fn sign_and_send(&self, tx_bytes: Vec<u8>) -> Result<Signature, WalletError>
async fn latest_blockhash(&self) -> Result<Hash, WalletError>    // helper to build the tx
async fn confirm(&self, sig: &Signature) -> Result<(), WalletError>
```

`tx_bytes` is the bincode wire format of an unsigned legacy or v0 `VersionedTransaction` (signature slots zeroed). The wallet
signs; the app submits through its own RPC, so the transaction lands on the cluster its blockhash came from whatever network
the extension is set to. It refuses before opening the wallet if no wallet is connected, the bytes do not decode, the
connected key is not a required signer, or the RPC is mainnet. A wallet that changes the message while signing is logged
(the program reads the ed25519 instruction by index).

## Manual check: devnet memo signature with Phantom

1. `cd app && dx serve --platform web` and wait for "Serving your app".
2. In Chrome or Brave with the Phantom extension: Phantom → Settings → Developer Settings → **Testnet Mode on**, network
   **Solana Devnet** (so Phantom's preview simulates on devnet).
3. Open **http://localhost:8080/dev/wallet** (use `localhost`, a secure context).
4. Press **Connect** (or the header's "Select wallet" → Phantom) and approve. The page shows `State: connected (Phantom)` and the address.
5. If the account has no devnet SOL: press **Airdrop 1 SOL** (the public faucet is rate-limited; otherwise use
   https://faucet.solana.com with the address shown).
6. Press **Test signature (devnet memo)** and approve in Phantom.
7. Success: green "Memo confirmed on chain", the signature, and **Open in Solana Explorer**: the explorer (`?cluster=devnet`)
   shows a successful transaction with one Memo instruction `schedina 1.F wallet spike <ms>` signed by your address.
   A failure prints the reason in red (declined, no SOL, RPC error).
