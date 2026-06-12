//! Cross-session knowledge store (KS).
//!
//! Session state dies with the session, but the errors a project produces
//! recur across days and weeks. This store keeps one compact "atom" per
//! error signature per project: when it was seen, how often, and which files
//! were edited when it went away. When a known error recurs in a later
//! session, the hook injects a one-line hint — "seen before, resolved by
//! editing X" — instead of letting the agent rediscover the fix from scratch.
//!
//! Storage: SQLite at `~/.local/share/panda/knowledge.sqlite`, keyed by
//! (project, sig_key). Writes are best-effort; the hook never fails on a
//! knowledge-store error.

use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::PathBuf;

/// Maximum hint lines injected per command output — hints must stay cheaper
/// than the content they replace.
pub const MAX_HINTS: usize = 2;
/// Resolutions are only suggested while reasonably fresh; a fix from months
/// ago is more likely stale than helpful.
const MAX_RESOLUTION_AGE_SECS: u64 = 30 * 86_400;

fn db_path() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("panda").join("knowledge.sqlite"))
}

pub fn open() -> Result<Connection> {
    let path = db_path().ok_or_else(|| anyhow::anyhow!("cannot determine data dir"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS error_atoms (
            project      TEXT NOT NULL,
            sig_key      TEXT NOT NULL,
            display      TEXT NOT NULL DEFAULT '',
            first_seen   INTEGER NOT NULL,
            last_seen    INTEGER NOT NULL,
            occurrences  INTEGER NOT NULL DEFAULT 1,
            resolved_by  TEXT,
            resolved_ts  INTEGER,
            PRIMARY KEY (project, sig_key)
        );",
    )?;
    Ok(conn)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Record occurrences of the given error signatures for `project`.
/// `sigs` items are `(key, display)` pairs from `ErrorSignature`.
pub fn record_errors(conn: &Connection, project: &str, sigs: &[(String, String)]) -> Result<()> {
    let now = now_secs();
    for (key, display) in sigs {
        conn.execute(
            "INSERT INTO error_atoms (project, sig_key, display, first_seen, last_seen, occurrences)
             VALUES (?1, ?2, ?3, ?4, ?4, 1)
             ON CONFLICT(project, sig_key) DO UPDATE SET
                 last_seen = ?4,
                 occurrences = occurrences + 1,
                 display = ?3",
            params![project, key, display, now as i64],
        )?;
    }
    Ok(())
}

/// Mark error signatures as resolved by the given edited files.
/// Called when errors that were present in the previous run of a command
/// disappear from the current run.
pub fn record_resolutions(
    conn: &Connection,
    project: &str,
    sig_keys: &[String],
    edited_files: &[String],
) -> Result<()> {
    if edited_files.is_empty() || sig_keys.is_empty() {
        return Ok(());
    }
    // Bare filenames are enough for a hint and keep rows small.
    let files: Vec<String> = edited_files
        .iter()
        .filter_map(|p| {
            std::path::Path::new(p)
                .file_name()
                .and_then(|f| f.to_str())
                .map(|s| s.to_string())
        })
        .take(5)
        .collect();
    if files.is_empty() {
        return Ok(());
    }
    let resolved_by = files.join(", ");
    let now = now_secs();
    for key in sig_keys {
        conn.execute(
            "UPDATE error_atoms SET resolved_by = ?1, resolved_ts = ?2
             WHERE project = ?3 AND sig_key = ?4",
            params![resolved_by, now as i64, project, key],
        )?;
    }
    Ok(())
}

/// For currently-occurring error signatures, return hint lines for those seen
/// and resolved before. Capped at [`MAX_HINTS`].
pub fn recurrence_hints(
    conn: &Connection,
    project: &str,
    sig_keys: &[String],
) -> Result<Vec<String>> {
    let now = now_secs();
    let mut hints = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT display, occurrences, resolved_by, resolved_ts
         FROM error_atoms
         WHERE project = ?1 AND sig_key = ?2 AND resolved_by IS NOT NULL",
    )?;
    for key in sig_keys {
        if hints.len() >= MAX_HINTS {
            break;
        }
        let row = stmt
            .query_row(params![project, key], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .ok();
        if let Some((display, occurrences, resolved_by, resolved_ts)) = row {
            let age = now.saturating_sub(resolved_ts as u64);
            if age > MAX_RESOLUTION_AGE_SECS {
                continue;
            }
            hints.push(format!(
                "[panda] recurring error ({}× before): {} — previously resolved by editing: {}",
                occurrences, display, resolved_by
            ));
        }
    }
    Ok(hints)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE error_atoms (
                project      TEXT NOT NULL,
                sig_key      TEXT NOT NULL,
                display      TEXT NOT NULL DEFAULT '',
                first_seen   INTEGER NOT NULL,
                last_seen    INTEGER NOT NULL,
                occurrences  INTEGER NOT NULL DEFAULT 1,
                resolved_by  TEXT,
                resolved_ts  INTEGER,
                PRIMARY KEY (project, sig_key)
            );",
        )
        .unwrap();
        conn
    }

    fn sig(key: &str) -> (String, String) {
        (key.to_string(), format!("display for {}", key))
    }

    #[test]
    fn record_errors_increments_occurrences() {
        let conn = test_conn();
        record_errors(&conn, "/repo", &[sig("E0308|main.rs|mismatch")]).unwrap();
        record_errors(&conn, "/repo", &[sig("E0308|main.rs|mismatch")]).unwrap();
        let n: i64 = conn
            .query_row("SELECT occurrences FROM error_atoms", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn no_hint_without_resolution() {
        let conn = test_conn();
        record_errors(&conn, "/repo", &[sig("E1")]).unwrap();
        let hints = recurrence_hints(&conn, "/repo", &["E1".to_string()]).unwrap();
        assert!(hints.is_empty());
    }

    #[test]
    fn resolution_then_recurrence_produces_hint() {
        let conn = test_conn();
        record_errors(&conn, "/repo", &[sig("E1")]).unwrap();
        record_resolutions(
            &conn,
            "/repo",
            &["E1".to_string()],
            &["/repo/src/auth/jwt.rs".to_string()],
        )
        .unwrap();
        record_errors(&conn, "/repo", &[sig("E1")]).unwrap();
        let hints = recurrence_hints(&conn, "/repo", &["E1".to_string()]).unwrap();
        assert_eq!(hints.len(), 1);
        assert!(hints[0].contains("jwt.rs"), "hint was: {}", hints[0]);
        assert!(hints[0].contains("2×"), "hint was: {}", hints[0]);
    }

    #[test]
    fn hints_scoped_by_project() {
        let conn = test_conn();
        record_errors(&conn, "/repo-a", &[sig("E1")]).unwrap();
        record_resolutions(&conn, "/repo-a", &["E1".to_string()], &["fix.rs".to_string()])
            .unwrap();
        let hints = recurrence_hints(&conn, "/repo-b", &["E1".to_string()]).unwrap();
        assert!(hints.is_empty());
    }

    #[test]
    fn hints_capped_at_max() {
        let conn = test_conn();
        let keys: Vec<String> = (0..5).map(|i| format!("E{}", i)).collect();
        let sigs: Vec<(String, String)> = keys.iter().map(|k| sig(k)).collect();
        record_errors(&conn, "/repo", &sigs).unwrap();
        record_resolutions(&conn, "/repo", &keys, &["fix.rs".to_string()]).unwrap();
        let hints = recurrence_hints(&conn, "/repo", &keys).unwrap();
        assert_eq!(hints.len(), MAX_HINTS);
    }

    #[test]
    fn resolution_with_no_files_is_noop() {
        let conn = test_conn();
        record_errors(&conn, "/repo", &[sig("E1")]).unwrap();
        record_resolutions(&conn, "/repo", &["E1".to_string()], &[]).unwrap();
        let hints = recurrence_hints(&conn, "/repo", &["E1".to_string()]).unwrap();
        assert!(hints.is_empty());
    }
}
