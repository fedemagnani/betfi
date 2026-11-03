//! The pause mirror (D-121): the indexer reads `Config` (and `Vault`) at every resync and after
//! every transaction carrying `ConfigUpdated` (`pause`, `set_params`), writes the `chain_config`
//! row, then sets `AppState::paused` from that row, so `POST /quote` and the copy-quote answer
//! 503 while the program is paused. The row is only replaced by a read at least as recent
//! (slot), so a slow read cannot undo a newer one, or once it is [`STALE_AFTER_MS`] old (a
//! reset ledger restarts its slots). Cash-out quotes stay available (D-063).

use solana_pubkey::Pubkey;
use std::sync::atomic::Ordering;

use crate::now_ms;
use crate::program::{ConfigAccount, Pdas, VaultAccount};
use crate::solana::rpc::RpcClient;
use crate::state::AppState;

/// A stored read this old is replaced even by a lower slot (no read stays in flight this long:
/// RPC calls time out after 30 s).
pub const STALE_AFTER_MS: u64 = 60_000;

fn i(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Reads `Config` and `Vault`, records them and mirrors `Config.paused`. A program without a
/// config yet (before `init_config`) changes nothing.
pub async fn mirror(state: &AppState, rpc: &RpcClient, pdas: &Pdas) -> Result<(), String> {
    let keys: [Pubkey; 2] = [pdas.config(), pdas.vault()];
    let (slot, accs) = rpc
        .get_multiple_accounts(&keys)
        .await
        .map_err(|e| e.to_string())?;
    let Some(Some(cfg)) = accs.first() else {
        return Ok(());
    };
    let cfg = ConfigAccount::decode(&cfg.data, &cfg.owner, &pdas.program)
        .map_err(|e| format!("config: {e}"))?;
    let vault = accs
        .get(1)
        .and_then(Option::as_ref)
        .and_then(|v| VaultAccount::decode(&v.data, &v.owner, &pdas.program).ok());
    record(state, &cfg, vault.as_ref(), slot)
        .await
        .map_err(|e| e.to_string())
}

/// Writes the `chain_config` row (when `slot` is not older than the stored one), then mirrors
/// the stored `paused` into [`AppState::paused`].
pub async fn record(
    state: &AppState,
    cfg: &ConfigAccount,
    vault: Option<&VaultAccount>,
    slot: u64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO chain_config (id, paused, copy_fee_bps, tipster_share_bps, reserve_u,
            vault_free_u, vault_locked_u, slot, updated_ms)
         VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET paused = excluded.paused,
            copy_fee_bps = excluded.copy_fee_bps, tipster_share_bps = excluded.tipster_share_bps,
            reserve_u = excluded.reserve_u,
            vault_free_u = COALESCE(excluded.vault_free_u, chain_config.vault_free_u),
            vault_locked_u = COALESCE(excluded.vault_locked_u, chain_config.vault_locked_u),
            slot = excluded.slot, updated_ms = excluded.updated_ms
         WHERE excluded.slot >= chain_config.slot
            OR excluded.updated_ms >= chain_config.updated_ms + ?",
    )
    .bind(i64::from(cfg.paused))
    .bind(i64::from(cfg.copy_fee_bps))
    .bind(i64::from(cfg.tipster_share_bps))
    .bind(i(cfg.reserve))
    .bind(vault.map(|v| i(v.free)))
    .bind(vault.map(|v| i(v.locked)))
    .bind(i(slot))
    .bind(i(now_ms()))
    .bind(i(STALE_AFTER_MS))
    .execute(&state.db)
    .await?;
    let paused: i64 = sqlx::query_scalar("SELECT paused FROM chain_config WHERE id = 1")
        .fetch_one(&state.db)
        .await?;
    let paused = paused != 0;
    let was = state.paused.swap(paused, Ordering::SeqCst);
    if was != paused {
        tracing::warn!(
            paused,
            "Config.paused changed on-chain; desk mirrors it (D-121)"
        );
    }
    metrics::gauge!("desk_paused").set(if paused { 1.0 } else { 0.0 });
    Ok(())
}
