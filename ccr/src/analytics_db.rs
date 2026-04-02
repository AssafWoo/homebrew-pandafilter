//! SQLite-backed analytics store.
//! Replaces the JSONL file at `~/.local/share/ccr/analytics.jsonl`.
//!
//! On first open, if `analytics.jsonl` exists and the DB is empty, all JSONL
//! records are migrated automatically (the source is renamed to `.jsonl.bak`).

use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::PathBuf;

use ccr_core::analytics::Analytics;

// ── Paths ──────────────────────────────────────────────────────────────────────

/// Returns the path to the SQLite database file.
pub fn db_path() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("ccr").join("analytics.db"))
}

fn jsonl_path() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("ccr").join("analytics.jsonl"))
}

// ── Open / schema ─────────────────────────────────────────────────────────────

/// Open (or create) the analytics database and initialize the schema.
pub fn open() -> Result<Connection> {
    let path = db_path().ok_or_else(|| anyhow::anyhow!("cannot locate data directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&path)?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS records (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp_secs  INTEGER NOT NULL,
            command         TEXT,
            subcommand      TEXT,
            input_tokens    INTEGER NOT NULL,
            output_tokens   INTEGER NOT NULL,
            savings_pct     REAL    NOT NULL,
            duration_ms     INTEGER,
            cache_hit       INTEGER NOT NULL DEFAULT 0,
            agent           TEXT,
            project_path    TEXT    NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_project_ts ON records(project_path, timestamp_secs);
        "#,
    )?;
    Ok(conn)
}

// ── JSONL migration ───────────────────────────────────────────────────────────

/// One-time migration: if the DB is empty and `analytics.jsonl` exists, import
/// all records and rename the JSONL to `.jsonl.bak`. Silent — no user output.
fn maybe_migrate(conn: &Connection) -> Result<()> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))?;
    if count > 0 {
        return Ok(());
    }

    let Some(jsonl) = jsonl_path() else { return Ok(()) };
    if !jsonl.exists() {
        return Ok(());
    }

    let content = match std::fs::read_to_string(&jsonl) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    let mut inserted = 0usize;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(a) = serde_json::from_str::<Analytics>(line) {
            let _ = conn.execute(
                "INSERT INTO records (timestamp_secs, command, subcommand, input_tokens, output_tokens, savings_pct, duration_ms, cache_hit, agent, project_path) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, '')",
                params![
                    a.timestamp_secs as i64,
                    a.command,
                    a.subcommand,
                    a.input_tokens as i64,
                    a.output_tokens as i64,
                    a.savings_pct as f64,
                    a.duration_ms.map(|d| d as i64),
                    a.cache_hit as i32,
                    a.agent,
                ],
            );
            inserted += 1;
        }
    }

    if inserted > 0 {
        let bak = jsonl.with_extension("jsonl.bak");
        let _ = std::fs::rename(&jsonl, &bak);
    }

    Ok(())
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Detect current project path: git toplevel → cwd fallback.
pub fn current_project_path() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        })
}

/// Append one analytics record to the database.
/// Also runs auto-cleanup (records older than 365 days) on every call.
pub fn append(analytics: &Analytics, project_path: &str) -> Result<()> {
    let conn = open()?;
    maybe_migrate(&conn)?;

    conn.execute(
        "INSERT INTO records (timestamp_secs, command, subcommand, input_tokens, output_tokens, savings_pct, duration_ms, cache_hit, agent, project_path) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            analytics.timestamp_secs as i64,
            analytics.command,
            analytics.subcommand,
            analytics.input_tokens as i64,
            analytics.output_tokens as i64,
            analytics.savings_pct as f64,
            analytics.duration_ms.map(|d| d as i64),
            analytics.cache_hit as i32,
            analytics.agent,
            project_path,
        ],
    )?;

    // Auto-cleanup: cheap with the index
    let _ = cleanup_old(&conn, 365);

    Ok(())
}

/// Load all analytics records. If `project_path` is `Some`, filter to that project only.
pub fn load_all(project_path: Option<&str>) -> Result<Vec<Analytics>> {
    let conn = open()?;
    maybe_migrate(&conn)?;

    let mut out = Vec::new();

    if let Some(proj) = project_path {
        let mut stmt = conn.prepare(
            "SELECT timestamp_secs, command, subcommand, input_tokens, output_tokens, savings_pct, \
             duration_ms, cache_hit, agent \
             FROM records WHERE project_path = ?1 ORDER BY timestamp_secs ASC",
        )?;
        let mut rows = stmt.query(params![proj])?;
        while let Ok(Some(row)) = rows.next() {
            if let Some(a) = row_to_analytics(row) {
                out.push(a);
            }
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT timestamp_secs, command, subcommand, input_tokens, output_tokens, savings_pct, \
             duration_ms, cache_hit, agent \
             FROM records ORDER BY timestamp_secs ASC",
        )?;
        let mut rows = stmt.query([])?;
        while let Ok(Some(row)) = rows.next() {
            if let Some(a) = row_to_analytics(row) {
                out.push(a);
            }
        }
    }

    Ok(out)
}

fn row_to_analytics(row: &rusqlite::Row<'_>) -> Option<Analytics> {
    let ts: i64 = row.get(0).ok()?;
    let command: Option<String> = row.get(1).ok().flatten();
    let subcommand: Option<String> = row.get(2).ok().flatten();
    let input_tokens: i64 = row.get(3).ok()?;
    let output_tokens: i64 = row.get(4).ok()?;
    let savings_pct: f64 = row.get(5).ok()?;
    let duration_ms: Option<i64> = row.get(6).ok().flatten();
    let cache_hit: i32 = row.get(7).ok()?;
    let agent: Option<String> = row.get(8).ok().flatten();

    Some(Analytics {
        timestamp_secs: ts as u64,
        command,
        subcommand,
        input_tokens: input_tokens as usize,
        output_tokens: output_tokens as usize,
        savings_pct: savings_pct as f32,
        duration_ms: duration_ms.map(|d| d as u64),
        cache_hit: cache_hit != 0,
        agent,
    })
}

/// Delete records older than `days` days. Returns the number of rows deleted.
pub fn cleanup_old(conn: &Connection, days: u32) -> Result<usize> {
    let cutoff = now_secs().saturating_sub(days as u64 * 86400) as i64;
    let n = conn.execute(
        "DELETE FROM records WHERE timestamp_secs < ?1",
        params![cutoff],
    )?;
    Ok(n)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_conn(dir: &TempDir) -> Connection {
        let path = dir.path().join("analytics.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS records (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp_secs  INTEGER NOT NULL,
                command         TEXT,
                subcommand      TEXT,
                input_tokens    INTEGER NOT NULL,
                output_tokens   INTEGER NOT NULL,
                savings_pct     REAL    NOT NULL,
                duration_ms     INTEGER,
                cache_hit       INTEGER NOT NULL DEFAULT 0,
                agent           TEXT,
                project_path    TEXT    NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_project_ts ON records(project_path, timestamp_secs);
            "#,
        )
        .unwrap();
        conn
    }

    fn make_record(cmd: &str, ts: u64) -> Analytics {
        Analytics {
            timestamp_secs: ts,
            command: Some(cmd.to_string()),
            subcommand: None,
            input_tokens: 100,
            output_tokens: 60,
            savings_pct: 40.0,
            duration_ms: Some(42),
            cache_hit: false,
            agent: None,
        }
    }

    #[test]
    fn insert_and_count() {
        let dir = TempDir::new().unwrap();
        let conn = temp_conn(&dir);
        for i in 0..100u64 {
            let a = make_record("git", 1_700_000_000 + i);
            conn.execute(
                "INSERT INTO records (timestamp_secs, command, subcommand, input_tokens, output_tokens, savings_pct, duration_ms, cache_hit, agent, project_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, '')",
                params![
                    a.timestamp_secs as i64, a.command, a.subcommand,
                    a.input_tokens as i64, a.output_tokens as i64,
                    a.savings_pct as f64, a.duration_ms.map(|d| d as i64),
                    a.cache_hit as i32, a.agent,
                ],
            ).unwrap();
        }
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 100);
    }

    #[test]
    fn cleanup_removes_old_records() {
        let dir = TempDir::new().unwrap();
        let conn = temp_conn(&dir);
        let now = now_secs();
        // Insert 50 old records (120 days ago) and 50 fresh records
        for i in 0..50u64 {
            conn.execute(
                "INSERT INTO records (timestamp_secs, command, input_tokens, output_tokens, savings_pct, cache_hit, project_path) VALUES (?1, 'git', 100, 60, 40.0, 0, '')",
                params![now.saturating_sub(120 * 86400 + i) as i64],
            ).unwrap();
        }
        for i in 0..50u64 {
            conn.execute(
                "INSERT INTO records (timestamp_secs, command, input_tokens, output_tokens, savings_pct, cache_hit, project_path) VALUES (?1, 'git', 100, 60, 40.0, 0, '')",
                params![now.saturating_sub(i) as i64],
            ).unwrap();
        }
        let deleted = cleanup_old(&conn, 90).unwrap();
        assert_eq!(deleted, 50, "should have deleted 50 old records");
        let remaining: i64 = conn.query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0)).unwrap();
        assert_eq!(remaining, 50);
    }

    #[test]
    fn project_path_scoping() {
        let dir = TempDir::new().unwrap();
        let conn = temp_conn(&dir);
        for _ in 0..10 {
            conn.execute(
                "INSERT INTO records (timestamp_secs, command, input_tokens, output_tokens, savings_pct, cache_hit, project_path) VALUES (1700000000, 'git', 100, 60, 40.0, 0, '/project/a')",
                [],
            ).unwrap();
        }
        for _ in 0..5 {
            conn.execute(
                "INSERT INTO records (timestamp_secs, command, input_tokens, output_tokens, savings_pct, cache_hit, project_path) VALUES (1700000000, 'cargo', 100, 60, 40.0, 0, '/project/b')",
                [],
            ).unwrap();
        }
        let count_a: i64 = conn.query_row(
            "SELECT COUNT(*) FROM records WHERE project_path = '/project/a'",
            [],
            |r| r.get(0),
        ).unwrap();
        let count_b: i64 = conn.query_row(
            "SELECT COUNT(*) FROM records WHERE project_path = '/project/b'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count_a, 10);
        assert_eq!(count_b, 5);
    }
}
