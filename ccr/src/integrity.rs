use sha2::{Digest, Sha256};
use std::path::Path;

pub enum IntegrityStatus {
    Verified,
    Tampered { expected: String, actual: String },
    NoBaseline,   // hook present, hash file absent
    NotInstalled, // neither hook nor hash exists
    OrphanedHash, // hash present, hook script absent
}

fn compute_sha256(path: &Path) -> anyhow::Result<String> {
    let content = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    Ok(hex::encode(hasher.finalize()))
}

/// Write `<64-hex>  panda-rewrite.sh\n` to `<hash_dir>/.panda-hook.sha256`.
/// Sets permissions to 0o444 (read-only speed bump against accidental overwrites).
pub fn write_baseline(script_path: &Path, hash_dir: &Path) -> anyhow::Result<()> {
    let hash = compute_sha256(script_path)?;
    let script_name = script_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("panda-rewrite.sh");
    // sha256sum-compatible format: two spaces between hash and filename
    let content = format!("{}  {}\n", hash, script_name);
    let hash_file = hash_dir.join(".panda-hook.sha256");
    // The file is written 0o444 (read-only) after each init. Make it writable
    // before overwriting so that re-running `panda init` doesn't fail with EACCES.
    #[cfg(unix)]
    if hash_file.exists() {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hash_file)?.permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&hash_file, perms)?;
    }
    std::fs::write(&hash_file, &content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hash_file)?.permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&hash_file, perms)?;
    }
    Ok(())
}

pub fn verify_hook(script_path: &Path, hash_dir: &Path) -> IntegrityStatus {
    let hash_file = hash_dir.join(".panda-hook.sha256");
    let script_exists = script_path.exists();
    let hash_exists = hash_file.exists();

    match (script_exists, hash_exists) {
        (false, false) => IntegrityStatus::NotInstalled,
        (true, false) => IntegrityStatus::NoBaseline,
        (false, true) => IntegrityStatus::OrphanedHash,
        (true, true) => {
            let baseline = match std::fs::read_to_string(&hash_file) {
                Ok(s) => s,
                Err(_) => return IntegrityStatus::NoBaseline,
            };
            // First whitespace-separated token is the hex hash
            let expected = baseline.split_whitespace().next().unwrap_or("").to_string();
            if expected.len() != 64 {
                return IntegrityStatus::NoBaseline;
            }
            let actual = match compute_sha256(script_path) {
                Ok(h) => h,
                Err(_) => return IntegrityStatus::NoBaseline,
            };
            if expected == actual {
                IntegrityStatus::Verified
            } else {
                IntegrityStatus::Tampered { expected, actual }
            }
        }
    }
}

/// On Tampered: print a warning to stderr and exit(1). Silent for all other states.
pub fn runtime_check(script_path: &Path, hash_dir: &Path) {
    if let IntegrityStatus::Tampered { expected, actual } = verify_hook(script_path, hash_dir) {
        eprintln!("panda: SECURITY WARNING — hook script has been modified!");
        eprintln!("  expected: {}", expected);
        eprintln!("  actual:   {}", actual);
        eprintln!("  Run `panda init` to reinstall, or `panda verify` for details.");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_script(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("panda-rewrite.sh");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_verified() {
        let tmp = tempfile::tempdir().unwrap();
        let script = write_script(tmp.path(), "#!/bin/bash\necho hello\n");
        write_baseline(&script, tmp.path()).unwrap();
        assert!(matches!(verify_hook(&script, tmp.path()), IntegrityStatus::Verified));
    }

    #[test]
    fn test_tampered() {
        let tmp = tempfile::tempdir().unwrap();
        let script = write_script(tmp.path(), "#!/bin/bash\necho hello\n");
        write_baseline(&script, tmp.path()).unwrap();
        // Modify the script
        fs::write(&script, "#!/bin/bash\necho tampered\n").unwrap();
        assert!(matches!(
            verify_hook(&script, tmp.path()),
            IntegrityStatus::Tampered { .. }
        ));
    }

    #[test]
    fn test_no_baseline() {
        let tmp = tempfile::tempdir().unwrap();
        let script = write_script(tmp.path(), "#!/bin/bash\necho hello\n");
        // No hash file written
        assert!(matches!(
            verify_hook(&script, tmp.path()),
            IntegrityStatus::NoBaseline
        ));
    }

    #[test]
    fn test_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("panda-rewrite.sh"); // does not exist
        assert!(matches!(
            verify_hook(&script, tmp.path()),
            IntegrityStatus::NotInstalled
        ));
    }

    #[test]
    fn test_orphaned_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let script = write_script(tmp.path(), "#!/bin/bash\necho hello\n");
        write_baseline(&script, tmp.path()).unwrap();
        // Remove the script, leaving only the hash
        fs::remove_file(&script).unwrap();
        assert!(matches!(
            verify_hook(&script, tmp.path()),
            IntegrityStatus::OrphanedHash
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_write_baseline_idempotent_after_readonly() {
        // Simulates re-running `ccr init` after the hash file was set to 0o444.
        // The second write_baseline call must not fail with Permission denied.
        let tmp = tempfile::tempdir().unwrap();
        let script = write_script(tmp.path(), "#!/bin/bash\necho hello\n");
        write_baseline(&script, tmp.path()).unwrap();
        // Second call — file is now read-only, must still succeed
        write_baseline(&script, tmp.path()).unwrap();
        assert!(matches!(verify_hook(&script, tmp.path()), IntegrityStatus::Verified));
    }

    #[cfg(unix)]
    #[test]
    fn test_hash_file_readonly() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let script = write_script(tmp.path(), "#!/bin/bash\necho hello\n");
        write_baseline(&script, tmp.path()).unwrap();
        let hash_file = tmp.path().join(".panda-hook.sha256");
        let mode = fs::metadata(&hash_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o444, "hash file should be read-only (0o444)");
    }
}
