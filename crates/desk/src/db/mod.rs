//! SQLite through `sqlx` (D-017). Queries are runtime-checked (`sqlx::query`, never `query!`), so
//! the crate builds with no database. Migrations live in `crates/desk/migrations/` and are
//! embedded at compile time: `0001_desk.sql` is 1.E's; 2.A adds `0002_quoter.sql`, 2.B
//! `0003_hedge.sql`, 2.C `0004_chain.sql`. Applied migrations are never edited (sqlx checks
//! their checksums and refuses to start).

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

/// The desk's migrations.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Opens (creating if needed) the database at `path` and applies pending migrations. `path` is a
/// file path, a `sqlite:` URL, or `sqlite::memory:` (one connection, so every query sees the
/// same in-memory database).
pub async fn open(path: &str) -> Result<SqlitePool, sqlx::Error> {
    let memory = path.contains(":memory:");
    let opts = if path.starts_with("sqlite:") {
        SqliteConnectOptions::from_str(path)?
    } else {
        if let Some(dir) = Path::new(path)
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
        {
            std::fs::create_dir_all(dir)?;
        }
        SqliteConnectOptions::new().filename(path)
    };
    let opts = opts
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let opts = if memory {
        opts
    } else {
        opts.journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
    };
    let pool = SqlitePoolOptions::new()
        .max_connections(if memory { 1 } else { 8 })
        .connect_with(opts)
        .await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// Reads a job cursor (`cursors` table).
pub async fn cursor(db: &SqlitePool, name: &str) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT value FROM cursors WHERE name = ?")
        .bind(name)
        .fetch_optional(db)
        .await
}

/// Writes a job cursor.
pub async fn set_cursor(db: &SqlitePool, name: &str, value: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO cursors (name, value, updated_ms) VALUES (?, ?, ?)
         ON CONFLICT (name) DO UPDATE SET value = excluded.value, updated_ms = excluded.updated_ms",
    )
    .bind(name)
    .bind(value)
    .bind(i64::try_from(crate::now_ms()).unwrap_or(i64::MAX))
    .execute(db)
    .await?;
    Ok(())
}

/// A ticket nonce as stored in `tickets.nonce` (SQLite INTEGER is i64): the same 64 bits, so
/// every u64 the program accepts round-trips and `(user, nonce)` stays unique. A saturating
/// conversion stored every nonce above `i64::MAX` as `i64::MAX`, so `/tickets` served a nonce
/// whose Ticket PDA was not the ticket's and the app refused to cash it out (3.C: the app
/// draws random 64-bit nonces, so about half of them).
pub fn nonce_to_db(nonce: u64) -> i64 {
    i64::from_ne_bytes(nonce.to_ne_bytes())
}

/// The inverse of [`nonce_to_db`].
pub fn nonce_from_db(v: i64) -> u64 {
    u64::from_ne_bytes(v.to_ne_bytes())
}

#[cfg(test)]
mod nonce_tests {
    use super::*;

    #[test]
    fn nonces_round_trip_all_64_bits() {
        for n in [
            0,
            1,
            i64::MAX as u64,
            1 << 63,
            (1 << 63) + 1,
            u64::MAX - 1,
            u64::MAX,
        ] {
            assert_eq!(nonce_from_db(nonce_to_db(n)), n);
        }
        assert_ne!(nonce_to_db(u64::MAX), nonce_to_db(u64::MAX - 1));
        assert_eq!(nonce_to_db(7), 7);
    }
}
