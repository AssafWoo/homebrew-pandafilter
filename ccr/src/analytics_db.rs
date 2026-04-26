//! SQLite-backed analytics store.
//! Replaces the JSONL file at `~/.local/share/panda/analytics.jsonl`.
//!
//! On first open, if `analytics.jsonl` exists and the DB is empty, all JSONL
//! records are migrated automatically (the source is renamed to `.jsonl.bak`).

use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::PathBuf;

use panda_core::analytics::Analytics;

// ── Paths ──────────────────────────────────────────────────────────────────────

/// Returns the path to the SQLite database file.
pub fn db_path() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("panda").join("analytics.db"))
}

fn jsonl_path() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("panda").join("analytics.jsonl"))
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
            model           TEXT,
            project_path    TEXT    NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_project_ts ON records(project_path, timestamp_secs);

        -- Add model column to existing databases (no-op on fresh DBs)
        -- SQLite ignores duplicate column errors via the pragma below.
        "#,
    )?;
    // ALTER TABLE ADD COLUMN is a no-op if the column already exists (since SQLite 3.35+),
    // but older versions error. Catch and ignore.
    let _ = conn.execute_batch("ALTER TABLE records ADD COLUMN model TEXT;");
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS guidance_records (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp_secs      INTEGER NOT NULL,
            session_id          TEXT NOT NULL,
            files_recommended   INTEGER NOT NULL,
            files_in_repo       INTEGER NOT NULL,
            excluded_tokens_est INTEGER,
            guidance_tokens     INTEGER,
            project_path        TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_guidance_session ON guidance_records(session_id, timestamp_secs);

        CREATE TABLE IF NOT EXISTS session_reads (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id      TEXT NOT NULL,
            file_path       TEXT NOT NULL,
            token_count     INTEGER NOT NULL,
            timestamp_secs  INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_reads_session ON session_reads(session_id);

        CREATE TABLE IF NOT EXISTS focus_compression_events (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id          TEXT NOT NULL,
            file_path           TEXT NOT NULL,
            timestamp_secs      INTEGER NOT NULL,
            sections_total      INTEGER NOT NULL,
            sections_preserved  INTEGER NOT NULL,
            sections_compressed INTEGER NOT NULL,
            lines_preserved     INTEGER NOT NULL,
            lines_compressed    INTEGER NOT NULL,
            old_method_tokens   INTEGER NOT NULL DEFAULT 0,
            new_method_tokens   INTEGER NOT NULL DEFAULT 0,
            section_details     TEXT,
            project_path        TEXT NOT NULL DEFAULT ''
        );

        CREATE TABLE IF NOT EXISTS focus_edit_hits (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id      TEXT NOT NULL,
            file_path       TEXT NOT NULL,
            timestamp_secs  INTEGER NOT NULL,
            edit_line       INTEGER NOT NULL,
            was_preserved   INTEGER NOT NULL
        );
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

// ── Legacy ccr → panda DB migration ──────────────────────────────────────

/// One-time migration: if the old `ccr/analytics.db` exists, copy all records
/// into the new `panda/analytics.db` using INSERT OR IGNORE (safe for re-runs).
fn maybe_migrate_from_ccr(conn: &Connection) -> Result<()> {
    let Some(data_dir) = dirs::data_local_dir() else { return Ok(()) };
    let old_db = data_dir.join("ccr").join("analytics.db");
    if !old_db.exists() {
        return Ok(());
    }

    // Attach the old DB and copy records, skipping any that already exist.
    conn.execute_batch(&format!(
        "ATTACH DATABASE '{}' AS old_ccr;",
        old_db.to_string_lossy().replace('\'', "''")
    ))?;

    let migrated: usize = conn.execute(
        "INSERT OR IGNORE INTO records
            (timestamp_secs, command, subcommand, input_tokens, output_tokens,
             savings_pct, duration_ms, cache_hit, agent, project_path)
         SELECT timestamp_secs, command, subcommand, input_tokens, output_tokens,
                savings_pct, duration_ms, cache_hit, agent, project_path
         FROM old_ccr.records",
        [],
    )?;

    conn.execute_batch("DETACH DATABASE old_ccr;")?;

    if migrated > 0 {
        // Rename old DB so we don't re-scan every time
        let bak = old_db.with_extension("db.bak");
        let _ = std::fs::rename(&old_db, &bak);
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
    let _ = maybe_migrate_from_ccr(&conn);

    conn.execute(
        "INSERT INTO records (timestamp_secs, command, subcommand, input_tokens, output_tokens, savings_pct, duration_ms, cache_hit, agent, model, project_path) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
            analytics.model,
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
    let _ = maybe_migrate_from_ccr(&conn);

    let mut out = Vec::new();

    if let Some(proj) = project_path {
        let mut stmt = conn.prepare(
            "SELECT timestamp_secs, command, subcommand, input_tokens, output_tokens, savings_pct, \
             duration_ms, cache_hit, agent, model \
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
             duration_ms, cache_hit, agent, model \
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
    let model: Option<String> = row.get(9).ok().flatten();

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
        model,
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

// ── Context Focusing guidance tracking ──────────────────────────────────────────

/// Record a context focusing guidance event.
pub fn record_guidance(
    session_id: &str,
    files_recommended: usize,
    files_in_repo: usize,
    excluded_tokens_est: Option<usize>,
    guidance_tokens: Option<usize>,
    project_path: &str,
) -> Result<()> {
    let conn = open()?;
    conn.execute(
        "INSERT INTO guidance_records (timestamp_secs, session_id, files_recommended, files_in_repo, excluded_tokens_est, guidance_tokens, project_path) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            now_secs() as i64,
            session_id,
            files_recommended as i64,
            files_in_repo as i64,
            excluded_tokens_est.map(|t| t as i64),
            guidance_tokens.map(|t| t as i64),
            project_path,
        ],
    )?;
    Ok(())
}

/// Record that a file was read in a session.
pub fn record_session_read(session_id: &str, file_path: &str, token_count: usize) -> Result<()> {
    let conn = open()?;
    conn.execute(
        "INSERT INTO session_reads (session_id, file_path, token_count, timestamp_secs) \
         VALUES (?1, ?2, ?3, ?4)",
        params![
            session_id,
            file_path,
            token_count as i64,
            now_secs() as i64,
        ],
    )?;
    Ok(())
}

/// Get guidance records for a session.
pub fn get_session_guidance(session_id: &str) -> Result<Vec<(usize, usize, Option<usize>)>> {
    let conn = open()?;
    let mut stmt = conn.prepare(
        "SELECT files_recommended, files_in_repo, excluded_tokens_est FROM guidance_records WHERE session_id = ?1 ORDER BY timestamp_secs",
    )?;
    let rows = stmt.query_map(params![session_id], |row| {
        let recommended: i64 = row.get(0)?;
        let total: i64 = row.get(1)?;
        let excluded: Option<i64> = row.get(2)?;
        Ok((recommended as usize, total as usize, excluded.map(|e| e as usize)))
    })?;

    let mut results = Vec::new();
    for row_result in rows {
        results.push(row_result?);
    }
    Ok(results)
}

/// Average tokens per file read across all session_reads.
/// Used to improve focus guidance estimates (replaces hardcoded 50).
pub fn avg_tokens_per_read() -> Result<usize> {
    let conn = open()?;
    let avg: f64 = conn.query_row(
        "SELECT COALESCE(AVG(token_count), 0) FROM session_reads",
        [],
        |row| row.get(0),
    )?;
    // Floor to at least 50 (the old default) to avoid underestimation on sparse data
    Ok((avg as usize).max(50))
}

/// Get file read frequencies for a project over the last N days, normalized to [0, 1].
/// Returns a map of relative file path → normalized frequency.
pub fn get_file_read_frequencies(project_path: &str, days: u32) -> Result<std::collections::HashMap<String, f64>> {
    let conn = open()?;
    let cutoff = (now_secs().saturating_sub(days as u64 * 86400)) as i64;

    // Get read counts per file across all sessions for this project's files
    let mut stmt = conn.prepare(
        "SELECT file_path, COUNT(*) as cnt FROM session_reads
         WHERE timestamp_secs >= ?1
         GROUP BY file_path
         ORDER BY cnt DESC"
    )?;

    let rows = stmt.query_map(params![cutoff], |row| {
        let path: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        Ok((path, count))
    })?;

    let mut counts: Vec<(String, i64)> = Vec::new();
    for row_result in rows {
        if let Ok((path, count)) = row_result {
            // Convert absolute paths to relative by stripping the project prefix
            let rel_path = path.strip_prefix(project_path)
                .map(|p| p.trim_start_matches('/').to_string())
                .unwrap_or(path);
            counts.push((rel_path, count));
        }
    }

    let max_count = counts.iter().map(|(_, c)| *c).max().unwrap_or(1).max(1) as f64;

    let mut freqs = std::collections::HashMap::new();
    for (path, count) in counts {
        freqs.insert(path, count as f64 / max_count);
    }

    Ok(freqs)
}

/// Get reads in a session.
pub fn get_session_reads(session_id: &str) -> Result<Vec<(String, usize)>> {
    let conn = open()?;
    let mut stmt = conn.prepare(
        "SELECT file_path, token_count FROM session_reads WHERE session_id = ?1",
    )?;
    let rows = stmt.query_map(params![session_id], |row| {
        let path: String = row.get(0)?;
        let tokens: i64 = row.get(1)?;
        Ok((path, tokens as usize))
    })?;

    let mut results = Vec::new();
    for row_result in rows {
        results.push(row_result?);
    }
    Ok(results)
}

// ── Focus compression analytics ──────────────────────────────────────────────

/// Record a focus compression event for a file.
pub fn record_focus_compression(
    session_id: &str,
    file_path: &str,
    result: &crate::handlers::focus_compress::FocusCompressResult,
    project_path: &str,
) -> Result<()> {
    let conn = open()?;
    // Serialize section_details as array of (start, end, was_preserved) tuples
    let details_vec: Vec<(usize, usize, bool)> = result
        .section_details
        .iter()
        .map(|d| (d.start_line, d.end_line, d.preserved))
        .collect();
    let details_json = serde_json::to_string(&details_vec).ok();
    conn.execute(
        "INSERT INTO focus_compression_events \
         (session_id, file_path, timestamp_secs, sections_total, sections_preserved, \
          sections_compressed, lines_preserved, lines_compressed, old_method_tokens, \
          new_method_tokens, section_details, project_path) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            session_id,
            file_path,
            now_secs() as i64,
            result.sections_total as i64,
            result.sections_preserved as i64,
            result.sections_compressed as i64,
            result.lines_preserved as i64,
            result.lines_compressed as i64,
            result.old_method_tokens as i64,
            result.new_method_tokens as i64,
            details_json,
            project_path,
        ],
    )?;
    Ok(())
}

/// Record whether an edit landed in a preserved or compressed section.
pub fn record_focus_edit_hit(
    session_id: &str,
    file_path: &str,
    edit_line: usize,
    was_preserved: bool,
) -> Result<()> {
    let conn = open()?;
    conn.execute(
        "INSERT INTO focus_edit_hits \
         (session_id, file_path, timestamp_secs, edit_line, was_preserved) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            session_id,
            file_path,
            now_secs() as i64,
            edit_line as i64,
            was_preserved as i32,
        ],
    )?;
    Ok(())
}

pub struct FocusCompressionStats {
    pub files_compressed: usize,
    pub total_lines_saved_vs_old: usize,
    pub total_new_tokens: usize,
    pub total_old_tokens: usize,
    pub edit_hit_rate: f64,
}

/// Query aggregate focus compression stats since `cutoff` (unix timestamp).
pub fn focus_compression_stats(cutoff: u64) -> Result<FocusCompressionStats> {
    let conn = open()?;

    let (files_compressed, total_old_tokens, total_new_tokens, _total_lines_compressed): (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(old_method_tokens), 0), \
                    COALESCE(SUM(new_method_tokens), 0), COALESCE(SUM(lines_compressed), 0) \
             FROM focus_compression_events WHERE timestamp_secs >= ?1",
            params![cutoff as i64],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap_or((0, 0, 0, 0));

    let total_lines_saved_vs_old = (total_old_tokens.saturating_sub(total_new_tokens)) as usize;

    let (total_hits, total_edits): (i64, i64) = conn
        .query_row(
            "SELECT COALESCE(SUM(was_preserved), 0), COUNT(*) \
             FROM focus_edit_hits WHERE timestamp_secs >= ?1",
            params![cutoff as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0, 0));

    let edit_hit_rate = if total_edits > 0 {
        total_hits as f64 / total_edits as f64
    } else {
        0.0
    };

    Ok(FocusCompressionStats {
        files_compressed: files_compressed as usize,
        total_lines_saved_vs_old,
        total_new_tokens: total_new_tokens as usize,
        total_old_tokens: total_old_tokens as usize,
        edit_hit_rate,
    })
}

// ── Quality signals ───────────────────────────────────────────────────────────

/// Aggregate signals used by the quality score in `panda gain --insight`.
pub struct QualitySignals {
    /// Average savings % over recent records (0-100).
    pub avg_savings_pct: f64,
    /// Fraction of invocations that were cache hits (0-1).
    pub cache_hit_rate: f64,
    /// Fraction of file re-reads that used delta or structural mode (0-1).
    pub delta_read_rate: f64,
    /// Total number of records in the window.
    pub total_records: usize,
}

/// Compute quality signals from records in the last `days` days.
pub fn get_quality_signals(days: u32) -> Result<QualitySignals> {
    let conn = open()?;
    let cutoff = now_secs().saturating_sub(days as u64 * 86400) as i64;

    // Use token-weighted savings (same formula as the banner) so quality score
    // agrees with what the user sees. Simple AVG(savings_pct) is misleading because
    // many small/zero-savings runs drag it down even when big commands save 90%+.
    let (total_records, cache_hits, total_input, total_output): (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(cache_hit), 0), \
                    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0) \
             FROM records WHERE timestamp_secs >= ?1",
            params![cutoff],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap_or((0, 0, 0, 0));

    let sum_savings = if total_input > 0 {
        (total_input - total_output) as f64 / total_input as f64 * 100.0
    } else {
        0.0
    };

    // Delta/structural re-reads are stored with command = "(read-delta)" or "(read-structural)"
    let delta_reads: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM records \
             WHERE timestamp_secs >= ?1 \
               AND (command = '(read-delta)' OR command = '(read-structural)')",
            params![cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let total_reads: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM records \
             WHERE timestamp_secs >= ?1 \
               AND (command = '(read-delta)' OR command = '(read-structural)' OR command = '(read)')",
            params![cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let cache_hit_rate = if total_records > 0 {
        cache_hits as f64 / total_records as f64
    } else {
        0.0
    };

    let delta_read_rate = if total_reads > 0 {
        delta_reads as f64 / total_reads as f64
    } else {
        0.0
    };

    Ok(QualitySignals {
        avg_savings_pct: sum_savings,
        cache_hit_rate,
        delta_read_rate,
        total_records: total_records as usize,
    })
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
            model: None,
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
