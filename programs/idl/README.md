`schedina.json` is the IDL of the **devnet build** (`--features devnet`), the build that gets deployed: from step 1.C it adds `faucet_usdc` (and its accounts) to what `anchor build` writes to `target/idl/`. Regenerate it from `programs/` after any program change with `anchor idl build -p schedina -o idl/schedina.json -- --features devnet`.

Backward compatibility (contract L2-4): `schedina/tests/idl_compat.rs` checks this file against the 1.C IDL (`schedina/tests/fixtures/idl_1c.json`, from commit `92ba2b1`): every 1.C instruction keeps its discriminator, accounts and args, errors are only appended, and accounts, events, types and constants only grow. Step 2.F's extra accounts travel in `remaining_accounts` (own tip `[tipster_profile (w)]` on `finalize`, `settle` and `cash_out`; copy `[orig_ticket, tipster_usdc (w)]` on `finalize`), so they do not appear here.

## Addresses

Same on localnet and devnet (every PDA depends only on the program ID); deployed on devnet on 29 Oct 2026 by `scripts/deploy.sh --cluster devnet` (step 3.B). Signers, transactions and balances are in `docs/deploy.md`.

| Account | Seeds | Address |
| --- | --- | --- |
| Program `schedina` | - | `HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW` |
| ProgramData | `[program]` (upgradeable loader) | `Dcq1tZHwpmV2NfWKGdAb8oEyHwhJimd3LMzSmjSPTVri` |
| `Config` | `["config"]` | `592biVynFdbBcUF9No2H3DNRP4gxiKwfFuzT58gA7sWt` |
| `Vault` | `["vault"]` | `6ZzbbCMydahPHQHRVB9LAVJDqbJjCqT5W3XXcYRjf8nC` |
| Vault USDC token account | `["vault_usdc"]` | `2ngo8aueRLSX4vUyQFM4F7Fd7h72CbRzuCykS3rcGyyv` |
| Escrow USDC token account | `["escrow_usdc"]` | `132YAR7FMAhpiDY7vncQ9UJJotvzC68wCLgbtn2mVoyw` |
| Mock USDC mint (6 dp, mint authority = `Config`) | `["mock","USDC"]` | `F6BLLBdH7b4QiCJWibUYvws8fK9upDMP5unCCaqvgH2N` |
| Event authority | `["__event_authority"]` | `HH5zdBAg3bggKwFCd877oSqVaSQBCqfJgovAXRTcCR7g` |
| `Pending` / `Ticket` | `["pending", user, nonce u64 LE]` / `["ticket", user, nonce u64 LE]` | per ticket |
| `TipsterProfile` | `["tipster", wallet]` | per tipster |
| `FaucetClaim` (devnet build) | `["faucet", owner]` | per owner |

## Mock AMM addresses (`mock_amm.json`)

`mock_amm.json` is the IDL of `programs/mock-amm` (step 4.D; devnet and localnet only, D-200…D-208).
Deployed on devnet on 29 Oct 2026 by `scripts/deploy.sh --cluster devnet --program mock-amm` and set
up by `crank amm-setup` (step 5.0a; transactions and SOL in `docs/deploy.md`). Every PDA depends only
on the program ID and schedina's mock USDC mint, so the same rows hold on localnet after
`amm-setup`; the lookup table's address depends on its creation slot and differs per cluster.

| Account | Seeds | Address |
| --- | --- | --- |
| Program `mock-amm` | - | `5WhsDY2XjzJoWDbe1AgJnnNgJccQwte9TgsvG4yRtHbf` |
| ProgramData | `[program]` (upgradeable loader) | `5NhW9VL6rMRdXpGYe2dT37DFuoDGxbm6esjhCs4BTYMb` |
| `AmmConfig` (admin = upgrade authority `Dg52cM5qFw38vyC1SqEUpLc1M6NEkZoJydzgvvGd3Rye`) | `["amm_config"]` | `dFXPVDAHPLzYEfpw2kttqgS2WsFhhvGMmbf1XCyE7Tn` |
| mockBONK mint (5 dp, mint authority `AmmConfig`) | `["mock","BONK"]` | `9n4tnNg5pREwfsMvkpaGNQ6hXrvq6onFvo6eCma34jjq` |
| mockBONK `MockToken` | `["token",mint]` | derived |
| mockBONK `Pool` | `["pool",mint]` | `4Aub8FuCV2zsrtvV5GuX3yk637UrHCUQqLygt2CrJB71` |
| mockBONK pool vaults (mockBONK / USDC) | `["vault",pool,mint]` | `AjK2NLDMmLJb6BMD71zyBdvoy5uUGii8GxAnNPQ6dULE` / `DZh28Z1hToMuT3x96T6gsJ3H9SjQ69rcudEPH9bjGuDA` |
| mockSOL mint (9 dp) | `["mock","SOL"]` | `A1p2y8NdtqP2sHSwzuqCTADJWKAKasHgCTGnVodGpTwA` |
| mockSOL `Pool` | `["pool",mint]` | `BDRbBjLsgHi2p6SbBEoReJspsGyN9GCrxDebG9UKsnfr` |
| mockSOL pool vaults (mockSOL / USDC) | `["vault",pool,mint]` | `FxvZbr8jEZ8wnPnrYtfoY42kWCjLnTKAu6AeWXkpFGRt` / `5K5PtQogwKH2v1daWBUHsJ4HbDTvgkAWzEacwrnvGZW9` |
| Lookup table (devnet; `AmmConfig.lookup_table`) | ALT `[authority, recent_slot]` | `BG5NZbdcjPoe8jQx4LS2VNr1c9YQoA4jpoQghdASVHG1` |
| `FaucetClaim` | `["faucet",mint,caller]` | per mint and signer |
