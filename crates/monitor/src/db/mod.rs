use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use rusqlite_migration::{Migrations, M};

use seeds::Seed;

pub mod queries;

const MIGRATIONS_SQL: &[&str] = &[
    include_str!("../../migrations/001_initial.sql"),
    include_str!("../../migrations/002_alerting.sql"),
    include_str!("../../migrations/003_fixed_seeds.sql"),
];

pub struct Db {
    pub conn: Mutex<Connection>,
}

impl Db {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        let mut conn =
            Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        Self::apply_pragmas(&conn)?;
        let migrations = Migrations::new(MIGRATIONS_SQL.iter().map(|s| M::up(s)).collect());
        migrations
            .to_latest(&mut conn)
            .context("running migrations")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        Self::apply_pragmas(&conn)?;
        let migrations = Migrations::new(MIGRATIONS_SQL.iter().map(|s| M::up(s)).collect());
        migrations
            .to_latest(&mut conn)
            .context("running migrations")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn apply_pragmas(conn: &Connection) -> Result<()> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.pragma_update(None, "mmap_size", 268_435_456i64)?;
        Ok(())
    }

    /// Insert any seeds that are not already in the table.
    pub fn seed_catalog(&self, seeds: &[Seed]) -> Result<()> {
        let now = Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "INSERT OR IGNORE INTO seeds (hostname, network, operator, added_at)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for s in seeds {
            let net = match s.network {
                seeds::Network::Mainnet => "mainnet",
                seeds::Network::Testnet => "testnet",
                seeds::Network::Signet => "signet",
            };
            stmt.execute(params![s.hostname, net, s.operator, now])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_and_seeding_run_on_memory() {
        let db = Db::open_in_memory().unwrap();
        db.seed_catalog(seeds::all()).unwrap();
        let conn = db.conn.lock().unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM seeds", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n as usize, seeds::all().len());
        // Run twice — should still be the same count.
        drop(conn);
        db.seed_catalog(seeds::all()).unwrap();
        let conn = db.conn.lock().unwrap();
        let n2: i64 = conn
            .query_row("SELECT count(*) FROM seeds", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, n2);
    }
}
