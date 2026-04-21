/// SQLite graph — schema v5, WAL mode, migration guard.
///
/// One database per (repo, commit). Opened read-only at query time,
/// read-write only during index builds. Schema version mismatch triggers
/// a full rebuild (caller's responsibility).
use anyhow::{bail, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub const SCHEMA_VERSION: &str = "8";

/// Open (or create) the database at `path` in read-write mode and apply the schema.
/// Returns an error if the existing schema version does not match SCHEMA_VERSION.
pub fn open_readwrite(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    configure(&conn)?;
    apply_schema(&conn)?;
    check_schema_version(&conn)?;
    Ok(conn)
}

/// Open an existing database at `path` in read-only mode.
/// Returns an error if the file does not exist or schema version mismatches.
pub fn open_readonly(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    check_schema_version(&conn)?;
    Ok(conn)
}

/// Returns true if the database at `path` exists and has the correct schema version.
pub fn is_valid(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => check_schema_version(&conn).is_ok(),
        Err(_) => false,
    }
}

fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch("
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous  = NORMAL;
        PRAGMA foreign_keys = ON;
    ")?;
    Ok(())
}

fn apply_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS files (
            path             TEXT PRIMARY KEY,
            summary          TEXT,
            embedding        BLOB,
            role             TEXT,
            role_confidence  REAL,
            commit_count     INTEGER NOT NULL DEFAULT 0,
            updated_at       INTEGER NOT NULL,
            symbols          TEXT,
            signatures       TEXT
        );

        CREATE TABLE IF NOT EXISTS cochanges (
            file_a        TEXT NOT NULL,
            file_b        TEXT NOT NULL,
            change_count  INTEGER NOT NULL DEFAULT 1,
            last_seen     INTEGER NOT NULL,
            PRIMARY KEY (file_a, file_b),
            CHECK (file_a < file_b)
        );

        CREATE INDEX IF NOT EXISTS idx_cochanges_file_a ON cochanges(file_a);
        CREATE INDEX IF NOT EXISTS idx_cochanges_file_b ON cochanges(file_b);

        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
    ")?;
    Ok(())
}

fn check_schema_version(conn: &Connection) -> Result<()> {
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row: &rusqlite::Row| row.get(0),
        )
        .ok();

    match version.as_deref() {
        Some(v) if v == SCHEMA_VERSION => Ok(()),
        Some(v) => bail!(
            "schema version mismatch: found '{}', expected '{}' — full rebuild required",
            v,
            SCHEMA_VERSION
        ),
        // No version row yet — write it (fresh database)
        None => {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                [SCHEMA_VERSION],
            )?;
            Ok(())
        }
    }
}

/// Update a file's embedding in the focus graph.
/// Used to refresh stale index-time embeddings with fresh ones from read processing.
pub fn update_embedding(conn: &Connection, path: &str, embedding_blob: &[u8]) -> Result<()> {
    conn.execute(
        "UPDATE files SET embedding = ?1 WHERE path = ?2",
        rusqlite::params![embedding_blob, path],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_and_reopen() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("graph.sqlite");

        let conn = open_readwrite(&db_path).unwrap();
        drop(conn);

        let conn2 = open_readwrite(&db_path).unwrap();
        drop(conn2);
    }

    #[test]
    fn schema_version_written_on_create() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("graph.sqlite");

        let conn = open_readwrite(&db_path).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn is_valid_returns_false_for_missing_file() {
        assert!(!is_valid(Path::new("/nonexistent/graph.sqlite")));
    }
}
