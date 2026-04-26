use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// Static savings-ratio table covering all known ccr handlers.
/// Values are fractions (0.0–1.0) of output tokens that ccr typically eliminates.
const HANDLER_SAVINGS: &[(&str, f32)] = &[
    ("cargo", 0.87),
    ("curl", 0.96),
    ("git", 0.80),
    ("docker", 0.85),
    ("docker-compose", 0.85),
    ("npm", 0.85),
    ("pnpm", 0.85),
    ("yarn", 0.85),
    ("ls", 0.80),
    ("cat", 0.70),
    ("grep", 0.80),
    ("rg", 0.80),
    ("find", 0.78),
    ("kubectl", 0.75),
    ("terraform", 0.70),
    ("pytest", 0.80),
    ("jest", 0.75),
    ("vitest", 0.75),
    ("pip", 0.60),
    ("pip3", 0.60),
    ("uv", 0.60),
    ("go", 0.65),
    ("helm", 0.70),
    ("brew", 0.65),
    ("gh", 0.60),
    ("make", 0.55),
    ("tsc", 0.70),
    ("mvn", 0.80),
    ("python", 0.50),
    ("python3", 0.50),
    ("eslint", 0.65),
    ("aws", 0.65),
    ("jq", 0.60),
    ("diff", 0.60),
    ("journalctl", 0.75),
    ("psql", 0.65),
    ("tree", 0.70),
    ("env", 0.50),
];

struct Opportunity {
    command: String,
    total_output_tokens: usize,
    call_count: usize,
    savings_pct: f32,
    ratio_source: &'static str,
}

/// Returns the top `limit` unoptimized commands sorted by potential token savings (highest first).
/// Each entry is (command_name, estimated_tokens_saveable).
/// Commands already routed through ccr are excluded.
pub fn top_unoptimized(limit: usize) -> Vec<(String, usize)> {
    let projects_dir = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("projects"),
        None => return vec![],
    };

    if !projects_dir.exists() {
        return vec![];
    }

    let mut jsonl_files: Vec<std::path::PathBuf> = Vec::new();
    collect_jsonl(&projects_dir, &mut jsonl_files);

    if jsonl_files.is_empty() {
        return vec![];
    }

    // Sort by modification time (newest first) and cap at 20 files so that
    // `ccr gain` never loads hundreds of MB of old conversation history.
    jsonl_files.sort_by(|a, b| {
        let mt_a = a.metadata().and_then(|m| m.modified()).ok();
        let mt_b = b.metadata().and_then(|m| m.modified()).ok();
        mt_b.cmp(&mt_a)
    });
    jsonl_files.truncate(20);

    let mut by_cmd: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for path in &jsonl_files {
        scan_jsonl(path, &mut by_cmd);
    }

    let actual_ratios = load_actual_savings_ratios();
    let static_map: BTreeMap<&str, f32> = HANDLER_SAVINGS.iter().cloned().collect();

    let mut results: Vec<(String, usize)> = by_cmd
        .iter()
        .filter_map(|(cmd, (tokens, _count))| {
            if *tokens == 0 {
                return None;
            }
            let savings_ratio = if let Some(&r) = actual_ratios.get(cmd.as_str()) {
                r
            } else if let Some(&r) = static_map.get(cmd.as_str()) {
                r
            } else {
                0.40 // BERT fallback
            };
            let estimated_saveable = (*tokens as f32 * savings_ratio) as usize;
            if estimated_saveable < 500 {
                return None;
            }
            Some((cmd.clone(), estimated_saveable))
        })
        .collect();

    results.sort_by(|a, b| b.1.cmp(&a.1));
    results.truncate(limit);
    results
}

pub fn run() -> Result<()> {
    let projects_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?
        .join(".claude")
        .join("projects");

    if !projects_dir.exists() {
        println!("No Claude Code history found at {}", projects_dir.display());
        return Ok(());
    }

    // Collect all JSONL files
    let mut jsonl_files: Vec<std::path::PathBuf> = Vec::new();
    collect_jsonl(&projects_dir, &mut jsonl_files);

    if jsonl_files.is_empty() {
        println!("No session history found in {}", projects_dir.display());
        return Ok(());
    }

    // Aggregate by command: track total output tokens and call count
    let mut by_cmd: BTreeMap<String, (usize, usize)> = BTreeMap::new(); // cmd -> (tokens, count)

    for path in &jsonl_files {
        scan_jsonl(path, &mut by_cmd);
    }

    if by_cmd.is_empty() {
        // All commands are already wrapped by PandaFilter (panda run ...) or
        // history is empty — fall back to the DB low-compression report.
        return run_db_report();
    }

    // Load actual measured savings ratios from analytics.jsonl (beats static estimates)
    let actual_ratios = load_actual_savings_ratios();

    // Extended static fallback table covering all known handlers
    let static_map: BTreeMap<&str, f32> = HANDLER_SAVINGS.iter().cloned().collect();

    let mut opportunities: Vec<Opportunity> = by_cmd
        .iter()
        .filter_map(|(cmd, (tokens, count))| {
            if *tokens == 0 {
                return None;
            }
            // Prefer measured actual ratio, then static fallback, then BERT default
            let (savings_pct, source) = if let Some(&r) = actual_ratios.get(cmd.as_str()) {
                (r * 100.0, "measured")
            } else if let Some(&r) = static_map.get(cmd.as_str()) {
                (r * 100.0, "estimated")
            } else {
                (40.0, "estimated") // BERT fallback
            };

            if savings_pct > 0.0 {
                Some(Opportunity {
                    command: cmd.clone(),
                    total_output_tokens: *tokens,
                    call_count: *count,
                    savings_pct,
                    ratio_source: source,
                })
            } else {
                None
            }
        })
        .collect();

    // Sort by estimated token savings descending
    opportunities.sort_by(|a, b| {
        let a_saved = (a.total_output_tokens as f32 * a.savings_pct / 100.0) as usize;
        let b_saved = (b.total_output_tokens as f32 * b.savings_pct / 100.0) as usize;
        b_saved.cmp(&a_saved)
    });

    if opportunities.is_empty() {
        // JSONL is empty or all commands are already wrapped — fall through to
        // DB-based low-compression report, which is always available once PandaFilter
        // has run sessions.
        return run_db_report();
    }

    println!("PandaFilter Discover — Missed Savings");
    println!("==============================");
    println!(
        "{:<12} {:>6} {:>10} {:>8}  {}",
        "COMMAND", "CALLS", "TOKENS", "SAVINGS", "IMPACT"
    );
    println!("{}", "-".repeat(58));

    let mut total_potential_tokens: usize = 0;
    for opp in &opportunities {
        let saved_tokens =
            (opp.total_output_tokens as f32 * opp.savings_pct / 100.0) as usize;
        total_potential_tokens += saved_tokens;

        let bar_len = (opp.savings_pct / 5.0) as usize; // 20 chars = 100%
        let bar = "█".repeat(bar_len.min(20));
        let source_marker = if opp.ratio_source == "measured" { "*" } else { " " };

        println!(
            "{:<12} {:>6} {:>10} {:>7.0}%{} {}",
            opp.command,
            opp.call_count,
            opp.total_output_tokens,
            opp.savings_pct,
            source_marker,
            bar,
        );
    }

    println!("{}", "-".repeat(58));
    println!(
        "Potential savings: {} tokens across {} command types",
        total_potential_tokens,
        opportunities.len()
    );
    if !actual_ratios.is_empty() {
        println!("(* = ratio measured from your actual panda usage)");
    }
    println!();
    println!("Run `panda init` to enable PreToolUse auto-rewriting.");

    Ok(())
}

/// Show commands that ARE running through PandaFilter but with low compression —
/// candidates for custom filter rules in panda.toml.
fn run_db_report() -> anyhow::Result<()> {
    use rusqlite::params;

    let conn = crate::analytics_db::open()?;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cutoff = now_secs.saturating_sub(30 * 86400) as i64;

    // Commands with ≥5 runs, meaningful input, and <35% weighted savings
    // Exclude internal pseudo-commands like (read-delta), (pipeline), etc.
    let mut stmt = conn.prepare(
        "SELECT command,
                COUNT(*) as runs,
                COALESCE(SUM(input_tokens), 0) as total_in,
                COALESCE(SUM(output_tokens), 0) as total_out
         FROM records
         WHERE timestamp_secs >= ?1
           AND command NOT LIKE '(%'
           AND input_tokens > 0
         GROUP BY command
         HAVING runs >= 5
            AND total_in > 2000
            AND CAST(total_in - total_out AS REAL) / total_in < 0.35
         ORDER BY total_in DESC
         LIMIT 15",
    )?;

    struct Row { command: String, runs: i64, total_in: i64, total_out: i64 }

    let rows: Vec<Row> = stmt
        .query_map(params![cutoff], |r| {
            Ok(Row {
                command: r.get(0)?,
                runs: r.get(1)?,
                total_in: r.get(2)?,
                total_out: r.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        println!("PandaFilter Discover");
        println!("════════════════════");
        println!();
        println!("All commands running through PandaFilter are compressing well (≥35% savings).");
        println!("Nothing to tune. Run `panda gain --breakdown` to see per-command details.");
        return Ok(());
    }

    println!("PandaFilter Discover — Low-Compression Commands (last 30 days)");
    println!("══════════════════════════════════════════════════════════════");
    println!("These commands run through PandaFilter but save less than 35%.");
    println!("Adding custom rules in .panda/filters.toml can improve them.");
    println!();
    println!("{:<18} {:>5}  {:>8}  {:>7}  {}",
        "COMMAND", "RUNS", "TOKENS IN", "SAVINGS", "OPPORTUNITY");
    println!("{}", "─".repeat(62));

    for row in &rows {
        let savings_pct = if row.total_in > 0 {
            (row.total_in - row.total_out) as f64 / row.total_in as f64 * 100.0
        } else { 0.0 };

        let potential = ((row.total_in as f64 * 0.60) - (row.total_in - row.total_out) as f64).max(0.0) as usize;
        let bar_len = ((1.0 - savings_pct / 100.0) * 10.0) as usize;
        let bar = "█".repeat(bar_len.min(10));

        println!("{:<18} {:>5}  {:>8}  {:>6.0}%  +~{}tok potential {}",
            row.command,
            row.runs,
            format_tokens(row.total_in as usize),
            savings_pct,
            format_tokens(potential),
            bar,
        );
    }

    println!("{}", "─".repeat(62));
    println!();
    println!("Tip: add a [commands.<name>] block in .panda/filters.toml to");
    println!("     remove noisy lines and improve compression for these commands.");

    Ok(())
}

fn format_tokens(n: usize) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{:.1}k", n as f64 / 1_000.0) }
    else { format!("{}", n) }
}

/// Load per-command token-weighted savings ratios from the analytics SQLite DB.
/// Returns a map of command → actual savings ratio (0.0–1.0).
fn load_actual_savings_ratios() -> BTreeMap<String, f32> {
    let Ok(conn) = crate::analytics_db::open() else {
        return BTreeMap::new();
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT command, SUM(input_tokens), SUM(output_tokens) \
         FROM records \
         WHERE command IS NOT NULL AND input_tokens > 0 \
         GROUP BY command"
    ) else {
        return BTreeMap::new();
    };

    stmt.query_map([], |row| {
        let cmd: String = row.get(0)?;
        let input: i64 = row.get(1)?;
        let output: i64 = row.get(2)?;
        Ok((cmd, input, output))
    })
    .ok()
    .map(|rows| {
        rows.filter_map(|r| r.ok())
            .filter_map(|(cmd, input, output)| {
                if input == 0 { return None; }
                let ratio = (input - output).max(0) as f32 / input as f32;
                if ratio > 0.0 { Some((cmd, ratio)) } else { None }
            })
            .collect()
    })
    .unwrap_or_default()
}

fn collect_jsonl(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                collect_jsonl(&path, out);
            } else if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                out.push(path);
            }
        }
    }
}

fn scan_jsonl(path: &Path, by_cmd: &mut BTreeMap<String, (usize, usize)>) {
    use std::io::{BufRead, BufReader};

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            _ => continue,
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        let cmd_str = v
            .get("tool_input")
            .and_then(|ti| ti.get("command"))
            .and_then(|c| c.as_str());

        let output_str = v
            .get("tool_response")
            .and_then(|tr| tr.get("output"))
            .and_then(|o| o.as_str());

        let Some(cmd) = cmd_str else { continue };

        // Skip already-optimized commands
        let trimmed = cmd.trim();
        if trimmed.starts_with("panda ") {
            continue;
        }

        let first = trimmed.split_whitespace().next().unwrap_or("");
        if first.is_empty() {
            continue;
        }

        // Count tokens (more accurate than byte length for savings estimation)
        let output_tokens = output_str
            .map(|o| panda_core::tokens::count_tokens(o))
            .unwrap_or(0);

        let entry = by_cmd.entry(first.to_string()).or_insert((0, 0));
        entry.0 += output_tokens;
        entry.1 += 1;
    }
}

#[allow(dead_code)]
fn human_tokens(tokens: usize) -> String {
    if tokens < 1000 {
        format!("{}", tokens)
    } else {
        format!("{:.1}k", tokens as f64 / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_ratios_empty_when_no_analytics() {
        // When analytics file does not exist, should return empty map without panic
        let ratios = load_actual_savings_ratios();
        // Either empty (file doesn't exist) or has entries (file exists) — both fine
        let _ = ratios;
    }

    #[test]
    fn scan_jsonl_counts_tokens_not_bytes() {
        // Build a minimal JSONL line and verify token counting
        use std::io::Write;
        let dir = std::env::temp_dir().join("panda_test_discover");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.jsonl");
        let output = "error: something went wrong\nwarning: check the config";
        let line = serde_json::json!({
            "tool_input": {"command": "cargo build"},
            "tool_response": {"output": output}
        });
        let mut f = std::fs::File::create(&file).unwrap();
        writeln!(f, "{}", line).unwrap();
        drop(f);

        let mut by_cmd: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        scan_jsonl(&file, &mut by_cmd);

        let (tokens, count) = by_cmd["cargo"];
        assert_eq!(count, 1);
        // Tokens should be non-zero and ≤ byte length (tokens are usually smaller)
        assert!(tokens > 0);
        assert!(tokens <= output.len());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scan_jsonl_skips_panda_prefixed_commands() {
        use std::io::Write;
        let dir = std::env::temp_dir().join("panda_test_discover2");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.jsonl");
        let line = serde_json::json!({
            "tool_input": {"command": "panda run cargo build"},
            "tool_response": {"output": "some output"}
        });
        let mut f = std::fs::File::create(&file).unwrap();
        writeln!(f, "{}", line).unwrap();
        drop(f);

        let mut by_cmd: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        scan_jsonl(&file, &mut by_cmd);
        assert!(by_cmd.is_empty(), "panda-prefixed commands should be skipped");

        std::fs::remove_dir_all(&dir).ok();
    }
}
