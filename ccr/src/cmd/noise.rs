//! `ccr noise` — show or reset learned noise patterns for the current project.

use anyhow::Result;
use crate::noise_learner::{NoiseStore, noise_path};

pub fn run(reset: bool) -> Result<()> {
    let key = crate::util::project_key()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project key"))?;

    if reset {
        if let Some(path) = noise_path(&key) {
            if path.exists() {
                std::fs::remove_file(&path)?;
                println!("Noise patterns reset for project {}.", &key[..8.min(key.len())]);
            } else {
                println!("No noise patterns found for this project.");
            }
        }
        return Ok(());
    }

    let store = NoiseStore::load(&key);
    if store.patterns.is_empty() {
        println!("No noise patterns learned yet for project {} (cwd: {}).",
            &key[..8.min(key.len())],
            std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default());

        // Show other projects that do have data
        if let Some(projects_dir) = dirs::data_local_dir().map(|d| d.join("ccr").join("projects")) {
            if let Ok(entries) = std::fs::read_dir(&projects_dir) {
                let others: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().join("noise.json").exists())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|k| k != &key)
                    .collect();
                if !others.is_empty() {
                    println!("\nOther projects with noise data:");
                    for k in &others {
                        let s = NoiseStore::load(k);
                        let promoted = s.patterns.values().filter(|p| p.promoted).count();
                        println!("  {} — {} patterns, {} promoted", &k[..8.min(k.len())], s.patterns.len(), promoted);
                    }
                    println!("\nRun `ccr noise` from the relevant project directory to inspect.");
                }
            }
        }
        return Ok(());
    }

    let mut patterns: Vec<&crate::noise_learner::NoisePattern> =
        store.patterns.values().collect();
    patterns.sort_by(|a, b| b.count.cmp(&a.count));

    println!(
        "Learned noise patterns for project {} ({} total):",
        &key[..8.min(key.len())],
        patterns.len()
    );
    println!();
    println!(
        "{:<6}  {:<6}  {:<7}  {:<10}  {}",
        "COUNT", "SUPPR", "RATE%", "STATUS", "PATTERN"
    );
    println!("{}", "─".repeat(70));

    for p in patterns.iter().take(50) {
        let rate = if p.count > 0 {
            p.suppressed as f32 / p.count as f32 * 100.0
        } else {
            0.0
        };
        let status = if p.promoted { "promoted" } else { "learning" };
        let display = if p.pattern.len() > 38 {
            format!("{}…", &p.pattern[..38])
        } else {
            p.pattern.clone()
        };
        println!(
            "{:<6}  {:<6}  {:<7.1}  {:<10}  {}",
            p.count, p.suppressed, rate, status, display
        );
    }

    if patterns.len() > 50 {
        println!("[+{} more patterns not shown]", patterns.len() - 50);
    }

    let promoted = patterns.iter().filter(|p| p.promoted).count();
    println!();
    println!("{} promoted (active pre-filter), {} still learning", promoted, patterns.len() - promoted);

    Ok(())
}
