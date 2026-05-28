use anyhow::Result;
use std::io::Write as IoWrite;

const FILTER_FILE: &str = ".panda/filters.toml";

/// `panda trust` — display and review .panda/filters.toml, then record its hash.
pub fn run_trust() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let filter_path = cwd.join(FILTER_FILE);

    if !filter_path.exists() {
        println!("No {} found in the current directory.", FILTER_FILE);
        println!();
        println!("Create one to define project-local filter rules, then run `panda trust` again.");
        return Ok(());
    }

    let content = std::fs::read_to_string(&filter_path)?;

    // ── Display the file ──────────────────────────────────────────────────────
    println!("=== {} ===", FILTER_FILE);
    println!("{}", content);
    println!("{}", "=".repeat(40));
    println!();

    // ── Risk analysis ─────────────────────────────────────────────────────────
    let risks = crate::filter_trust::analyze_risk(&content);
    if !risks.is_empty() {
        println!("SECURITY NOTICE — high-risk patterns detected:");
        for r in &risks {
            println!("  ⚠  {}", r);
        }
        println!();
        println!(
            "These patterns can control what the AI is allowed to see. \
             Only grant trust if you authored this file or have read and understand every rule."
        );
        println!();
    }

    // ── Prompt ────────────────────────────────────────────────────────────────
    println!("Grant trust to {} in this project? [y/N] ", FILTER_FILE);
    print!("> ");
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    if input.trim().eq_ignore_ascii_case("y") {
        crate::filter_trust::record_trust(&filter_path)?;
        let hash = {
            let bytes = std::fs::read(&filter_path)?;
            crate::filter_trust::compute_hash(&bytes)
        };
        println!();
        println!("Trusted. SHA-256 baseline recorded:");
        println!("  {}  {}", &hash[..16], FILTER_FILE);
        println!();
        println!(
            "If the file changes after a git pull, trust is automatically revoked \
             and you will be prompted to re-review."
        );
    } else {
        println!();
        println!("Aborted — filters remain blocked.");
    }

    Ok(())
}

/// `panda untrust` — revoke trust for .panda/filters.toml in the current project.
pub fn run_untrust() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let filter_path = cwd.join(FILTER_FILE);

    crate::filter_trust::revoke_trust(&filter_path)?;
    println!("Trust revoked for {} in this project.", FILTER_FILE);
    println!("Filters will not be applied until you run `panda trust` again.");
    Ok(())
}
