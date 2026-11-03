//! Review gate: the migrations apply on an empty SQLite file, and re-opening applies nothing new.

mod common;

use common::TempDb;

/// Tables every migration set must contain. L2 steps add tables in their own migration files
/// (l2-contracts L2-0), so this is a floor, not the exact list.
const TABLES: [&str; 12] = [
    "book_snapshots",
    "cashouts",
    "cursors",
    "feed_events",
    "fill_allocations",
    "hedge_positions",
    "markets",
    "pascal_fills",
    "quote_legs",
    "quotes",
    "ticket_legs",
    "tickets",
];

#[tokio::test]
async fn apply_on_an_empty_file_and_again() {
    let db = TempDb::new("migrations");
    assert!(!db.0.exists());
    let pool = desk::db::open(&db.path()).await.expect("migrations apply");
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table'
           AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_sqlx_%' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for t in TABLES {
        assert!(
            tables.iter().any(|x| x == t),
            "missing table {t}: {tables:?}"
        );
    }

    // Constraints hold: a bad state and a bad venue are refused, the key is idempotent.
    let bad = sqlx::query(
        "INSERT INTO tickets (ticket, pending, user, nonce, state, n, stake_u, payout_u,
            odds_offered_u, min_payout_per_unit_u, updated_ms)
         VALUES ('T', 'P', 'U', 1, 'lost', 1, 1, 1, 1, 1, 0)",
    )
    .execute(&pool)
    .await;
    assert!(bad.is_err());
    let bad = sqlx::query(
        "INSERT INTO pascal_fills (venue, trade_id, symbol, side, price_u, size, liquidity,
            fee_u, collateral_change_u, client_order_id, seq, ts_ms, raw)
         VALUES ('nyse', 1, 'S', 'bid', 1, 1, 'taker', 0, 0, 1, 1, 1, '{}')",
    )
    .execute(&pool)
    .await;
    assert!(bad.is_err());
    desk::db::set_cursor(&pool, "fills_seq", 41).await.unwrap();
    desk::db::set_cursor(&pool, "fills_seq", 42).await.unwrap();
    assert_eq!(
        desk::db::cursor(&pool, "fills_seq").await.unwrap(),
        Some(42)
    );
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;

    // Re-open: nothing new to apply, data kept.
    let pool = desk::db::open(&db.path()).await.expect("re-open");
    let again: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(applied, again);
    assert_eq!(
        desk::db::cursor(&pool, "fills_seq").await.unwrap(),
        Some(42)
    );
    pool.close().await;
}

#[tokio::test]
async fn in_memory_database_works() {
    let pool = desk::db::open("sqlite::memory:").await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM markets")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}
