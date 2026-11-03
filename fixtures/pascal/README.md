# Pascal read-API fixtures

Real responses from `https://data.pascal.trade` (REST) and `wss://data.pascal.trade/ws`
(WebSocket), recorded on 27-28 Oct 2026 for offline tests of `pascal-rs` (step 1.A) and the
scoring crate (1.C). Imported from an earlier Pascal client project (0.C), where they were recorded by `python3 -I scripts/record_fixtures.py --config fixtures/pascal/record_config.json`
(REST; `markets_full` in the config points at the saved full `/markets` body, which is not
committed) and `scripts/record_ws.mjs` (WebSocket); both scripts live in that repo, not this one.

**Verbatim.** Each file is one response body exactly as served: `status`, `data`, `seq`,
`state_time_ms`, `server_time_ms` (error envelopes have no `seq`/`state_time_ms`). Bodies are
re-indented for diffs; no key is added, removed or reordered, and numbers stay strings.
Long lists are bounded with the API's own `limit` parameter so pages stay verbatim. The only
post-hoc trims are `rest_markets.json` (see `markets.TRIMMED.md`) and
`ws_account_snapshot_with_orders.TRIMMED.json` (see below); both say so in their names.

All wallets below are public Pascal accounts picked by their trading pattern on the public
tape (`/api/v1/trades`, 30 days to 28 Oct 2026 00:14 UTC: 67,696 trades, 257 wallets). The
labels describe the pattern only.

## Wallets

| Label | Owner | Why |
| --- | --- | --- |
| `mm` | `EWGv5fY5VRNjjGDV7EkthQTnGSh2g94vqRhRVhomgBmR` | Almost pure maker (50/50 sampled fills MAKER, $363k lifetime maker notional) with a moderate book: 35 resting orders, 19 positions, so the snapshot is untrimmed. Input for the mark-manipulation check (orders next to books). |
| `big_mm` | `7ATdPM8uGMu3qiRSCpc6hUrSZ5VxFSUyYLfRipABG5bC` | Largest wallet on the tape ($1.5M 30-day notional, maker and taker), 52,759 resting orders and 582 positions. Used for fills (mixed MAKER/TAKER), the WebSocket `account` channel, a null bio and zero builder stats. |
| `taker` | `2o3ktZJPpS55RkGTKrdLJLTqzPC1pFXDz63bwWSTerFe` | Heavy taker ($325k taker notional in 30 days). Its history shows total PnL +$6.1k but trading PnL -$6.6k: $12.6k of referral and campaign rewards, the exact gap weekly points must ignore. Also has a bio, a registered deposit address and a transfer history with DEPOSIT, WITHDRAWAL and both REWARD reasons. |
| `small` | `Bkfzx199CCXspfGiHtADoPS41XpcmazkhiPh2LjYQBGk` | Small account: $4.93 collateral, one open position, $95 lifetime taker notional. Exercises the R_W floor and the log factor in 1.4. |
| `resolved` | `8hMX36XfTG9msfiyqEfcQwMo5mcNg8SHxyPWU3G4jfj` | 47 of its last 50 closed position instances have `held_until_resolution: true`. |
| `key_holder` | `58XdESEDojjqoKvM7SFb7BK1n9e5c1D94i3DZsL6Gr21` | Its only trading key, `pmx-keeper` (`4z3dpi2TnkkLAdZo6h6dEsSm2XbCyKA3i3jKt2AEh1Tx`), is shared by 23 owners: the "vendor key, ignored" case in 1.3. No key shared by 2 or 3 owners exists among the 257 wallets. |

No wallet among the 257 has a username (`/usernames` returns `{}` for every batch), and none
has non-zero builder stats or fills carrying `builder_pubkey`.

## REST files

Captured 2026-10-28 between 00:29:12 and 00:29:20 UTC unless noted.

| File | Request | Notes |
| --- | --- | --- |
| `rest_markets.json` | `GET /api/v1/markets` | Captured 2026-10-27 23:54:10 UTC. **Trimmed** 3,097 → 50 markets, see `markets.TRIMMED.md`. |
| `rest_markets_resolved.json` | `GET /api/v1/markets/resolved?lookback=24h&order_by=resolution_time&limit=50` | 50 markets, each with `resolution {price, seq, market_resolution_ts_ms}`. |
| `rest_markets_search.json` | `POST /api/v1/markets/search` body `{"symbols":["TX_GOV_2026.REP","US_IRAN_CFR_THRU.26OCT24","NBA_26OCT26_GSWDEN_ML.GSW"]}` | One live and two resolved markets; this is how a resolved market's topic is found after it leaves `/markets`. |
| `rest_trades_page1.json` | `GET /api/v1/trades?symbols=<120 symbols>&limit=50` | The 120 symbols are the most-traded markets of the 7 days before capture (first: `ISRAEL_PM_2026.NETANYAHU`). `next_cursor` = `00000000012110785211:0000000006`. |
| `rest_trades_page2.json` | same + `before_cursor=00000000012110785211:0000000006` | Strictly older than page 1's last item. |
| `rest_books.json` | `GET /api/v1/books?symbols=<13 symbols>` | The 8 markets where `mm` holds positions, 2 more where it rests orders, plus `TX_GOV_2026.REP`, `FED_DECISION_26OCT.NO_CHANGE` and the thin `EPL_2027_PTS_DED.MCI` (1 ask). No book carries `spec`. |
| `rest_accounts_owner_mm.json` | `GET /api/v1/accounts/{mm}` | 35 orders, 19 positions, untrimmed. |
| `rest_accounts_owner_small.json` | `GET /api/v1/accounts/{small}` | |
| `rest_accounts_owner_history.json` | `GET /api/v1/accounts/{taker}/history` | 4 timeframes: day 49 points / 30 min, week 43 / 4 h, month 61 / 12 h, all_time 57 / 24 h (first point at account creation, not aligned). Last point of every series = 2026-10-28 00:00:00 UTC, the latest completed 30-minute boundary, not a live value. |
| `rest_accounts_owner_history_small.json` | `GET /api/v1/accounts/{small}/history` | Older account: all_time has 123 points. |
| `rest_v2_accounts_owner_position_instances.json` | `GET /api/v2/accounts/{resolved}/position-instances?order_by=close_time&limit=50` | Has `next_cursor`. |
| `rest_accounts_owner_fills.json` | `GET /api/v1/accounts/{big_mm}/fills?limit=50` | MAKER and TAKER fills; each carries `counterparty`. |
| `rest_accounts_owner_transfers.json` | `GET /api/v1/accounts/{taker}/transfers?limit=50` | DEPOSIT, WITHDRAWAL, REWARD (`MIDTERMS_2026_REFERRAL_REWARD`, `MIDTERMS_2026_TRADING_REWARD`). |
| `rest_accounts_owner_trading_keys.json` | `GET /api/v1/accounts/{key_holder}/trading-keys` | |
| `rest_trading_keys_pubkey_accounts.json` | `GET /api/v1/trading-keys/4z3dpi2TnkkLAdZo6h6dEsSm2XbCyKA3i3jKt2AEh1Tx/accounts` | 23 owners. |
| `rest_usernames.json` | `GET /api/v1/usernames?owners=<the 6 wallets above>` | `{"usernames": {}}`: nobody in the sample has one. |
| `rest_accounts_owner_bio.json` | `GET /api/v1/accounts/{taker}/bio` | Bio set. |
| `rest_accounts_owner_bio_null.json` | `GET /api/v1/accounts/{big_mm}/bio` | `account_bio: null`. |
| `rest_accounts_owner_builder_stats.json` | `GET /api/v1/accounts/{big_mm}/builder-stats` | All zero (no builder found). |
| `rest_accounts_owner_deposit_address.json` | `GET /api/v1/accounts/{taker}/deposit-address` | `registered: true`. |
| `rest_error_invalid_request.json` | `GET /api/v1/trades?symbols=ISRAEL_PM_2026.NETANYAHU&limit=1000` | HTTP 422 `INVALID_REQUEST` ("limit=1000, min=1, max=500"). Deliberate, harmless; shows the error envelope. A missing `symbols` gives the same code with `deserialize_failed: … missing field \`symbols\``. |

## WebSocket files

Recorded 2026-10-28 00:37:37 to 00:47:38 UTC with
`node scripts/record_ws.mjs 7ATdPM8uGMu3qiRSCpc6hUrSZ5VxFSUyYLfRipABG5bC <dir> 600 <raw_log> --exclude-orders`:
one connection to `wss://data.pascal.trade/ws`, a single subscribe frame for `all_trades`,
`account` (`big_mm`, `exclude_orders: true`) and `market_update`, and `{"type":"ping"}` every
30 s (each answered by `{"type":"pong"}`). Each file is the first frame of its kind, verbatim
(re-indented only). In 600 s the socket delivered 1 snapshot + 16 updates on `all_trades`,
1 snapshot + 13 updates on `account` and only the snapshot on `market_update`: no listing,
display change or resolution happened in that window, so there is no `market_update` update file.

| File | Frame | Notes |
| --- | --- | --- |
| `ws_all_trades_snapshot.json` | `all_trades` snapshot, `seq` 12117113181 | 100 most recent trades, oldest first, each with `maker_reduce_only`, `taker_reduce_only`, `*_position_size_prev`. |
| `ws_all_trades_update.json` | `all_trades` update, `seq` 12117121005 | 2 trades. |
| `ws_account_snapshot.json` | `account` snapshot for `big_mm`, `seq` 12117113181 | The channel descriptor is echoed (`owner`, `exclude_orders: true`). `orders: []` because of `exclude_orders`; 587 positions (each with the undocumented `remaining_entry_notional_usd`), `rewards`, `lifetime_stats`, no `username` / `twitter_account_connection`. 411 KB. |
| `ws_account_update.json` | first `account` update carrying `fills`, `seq` 12117121005 | 1 fill; `collateral_usd`, `lifetime_stats`, changed `positions`. `orders`, `pending_deposits`, `pending_withdrawals` are present (empty) on every update, as in the docs' examples. |
| `ws_market_update_snapshot.json` | `market_update` snapshot | `data: []`, as documented: the market list comes from `GET /api/v1/markets`. |
| `ws_account_snapshot_with_orders.TRIMMED.json` | `account` snapshot for `big_mm` **without** `exclude_orders`, `seq` 12112544499 (00:13:41 UTC, separate run) | **Trimmed.** The frame was 22.3 MB: 52,759 `orders` and 582 `positions`. Both arrays are cut to their first 50; envelope and every other field verbatim. Shows why rated wallets must be subscribed with `exclude_orders: true`. |
| `ws_account_update_with_orders.json` | first `account` update without `exclude_orders`, `seq` 12112544597 | Verbatim: 90 `order_updates` (each `{order, update_reason}`) plus the same 90 orders in `orders`. That run received 3,645 such updates in about 3 minutes for this one wallet. |

A second, 30-minute recording with the same subscription (00:17:34 to 00:47:34 UTC, 31
`all_trades` updates, 27 `account` updates) also produced **no `market_update` update**; the
last resolution before it was at 23:45 UTC (`/markets/resolved?lookback=4h`). A `market_update`
update fixture has to wait for the next listing or resolution.
