use anyhow::Result;
use std::path::Path;
use crate::integrity::{verify_hook, IntegrityStatus};

pub fn run() -> Result<()> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;

    println!("Claude Code:");
    let claude_tampered = report_status(
        &home.join(".claude").join("hooks").join("panda-rewrite.sh"),
        &home.join(".claude").join("hooks"),
        "panda init",
    );

    println!("\nCursor:");
    let cursor_tampered = report_status(
        &home.join(".cursor").join("hooks").join("panda-rewrite.sh"),
        &home.join(".cursor").join("hooks"),
        "panda init --agent cursor",
    );

    if claude_tampered || cursor_tampered {
        std::process::exit(1);
    }
    Ok(())
}

/// Print status for one agent's hook. Returns true if Tampered (caller should exit 1).
fn report_status(script: &Path, hashdir: &Path, reinstall_cmd: &str) -> bool {
    let hash_file = hashdir.join(".panda-hook.sha256");
    match verify_hook(script, hashdir) {
        IntegrityStatus::Verified => {
            println!("  OK  Verified   {}", script.display());
            false
        }
        IntegrityStatus::Tampered { expected, actual } => {
            println!("  ERR Tampered   {}", script.display());
            println!("      Expected: {}", expected);
            println!("      Actual:   {}", actual);
            println!();
            println!("  Run `{}` to reinstall the hook and reset the baseline.", reinstall_cmd);
            true
        }
        IntegrityStatus::NoBaseline => {
            println!("  ?   No baseline — hash file not found: {}", hash_file.display());
            println!("      Run `{}` to create the baseline.", reinstall_cmd);
            false
        }
        IntegrityStatus::NotInstalled => {
            println!("  -   Not installed (run `{}`)", reinstall_cmd);
            false
        }
        IntegrityStatus::OrphanedHash => {
            println!("  ?   Orphaned hash — script missing, hash exists: {}", hash_file.display());
            println!("      Run `{}` to reinstall.", reinstall_cmd);
            false
        }
    }
}
