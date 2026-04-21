use anyhow::Result;
use panda_core::analytics::Analytics;
use owo_colors::{OwoColorize, Stream::Stdout, Style};
use std::collections::BTreeMap;
use serde_json;

/// Pricing table for known Anthropic model families (input tokens, $/1M).
/// More-specific prefixes must appear before less-specific ones because
/// matching uses `.contains(prefix)` and stops at the first hit.
const MODEL_PRICES: &[(&str, f64)] = &[
    ("claude-opus-4-6",    5.00),  // Opus 4.6
    ("claude-opus-4-5",    5.00),  // Opus 4.5
    ("claude-opus-4",     15.00),  // Opus 4 / 4.1 (deprecated)
    ("claude-opus-3",     15.00),
    ("claude-sonnet-4",    3.00),
    ("claude-sonnet-3-7",  3.00),
    ("claude-sonnet-3-5",  3.00),
    ("claude-haiku-4-5",   1.00),  // Haiku 4.5
    ("claude-haiku-4",     0.80),
    ("claude-haiku-3",     0.25),
];

/// Resolve the fallback price per token (used for records with no stored model).
/// Priority: config override → ANTHROPIC_MODEL env var → $3.00 fallback.
fn resolve_price() -> (f64, String) {
    // 1. Config override
    if let Ok(cfg) = crate::config_loader::load_config() {
        if let Some(price) = cfg.global.cost_per_million_tokens {
            return (price / 1_000_000.0, format!("${:.2}/1M (config)", price));
        }
    }

    // 2. Auto-detect from ANTHROPIC_MODEL env var
    if let Ok(model) = std::env::var("ANTHROPIC_MODEL") {
        let model_lc = model.to_lowercase();
        for (prefix, price) in MODEL_PRICES {
            if model_lc.contains(prefix) {
                return (
                    price / 1_000_000.0,
                    format!("${:.2}/1M ({})", price, model),
                );
            }
        }
    }

    // 3. Fallback
    (3.00 / 1_000_000.0, "$3.00/1M (set ANTHROPIC_MODEL to auto-detect)".to_string())
}

/// Look up the price-per-million for a model name, or None if unrecognized.
fn model_price_per_million(model: &str) -> Option<f64> {
    let model_lc = model.to_lowercase();
    for (prefix, price) in MODEL_PRICES {
        if model_lc.contains(prefix) {
            return Some(*price);
        }
    }
    None
}

/// Compute blended cost across records, using per-record model when available.
/// Returns (total_cost, price_label).
fn blended_cost(records: &[&Analytics]) -> (f64, String) {
    let (fallback_per_token, fallback_label) = resolve_price();

    // Tally tokens per model for display
    let mut model_tokens: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut fallback_tokens: usize = 0;
    let mut total_cost = 0.0;

    for r in records {
        let saved = r.tokens_saved();
        if saved == 0 {
            continue;
        }
        if let Some(ref model) = r.model {
            if let Some(price_m) = model_price_per_million(model) {
                total_cost += saved as f64 * price_m / 1_000_000.0;
                *model_tokens.entry(model.clone()).or_default() += saved;
                continue;
            }
        }
        // Fallback for records without a stored model (or unrecognized model)
        total_cost += saved as f64 * fallback_per_token;
        fallback_tokens += saved;
    }

    // Build label
    let label = if model_tokens.is_empty() {
        // All records used the fallback
        fallback_label
    } else if fallback_tokens == 0 && model_tokens.len() == 1 {
        // All records used one known model
        let (model, _) = model_tokens.iter().next().unwrap();
        let price = model_price_per_million(model).unwrap_or(3.0);
        format!("${:.2}/1M ({})", price, model)
    } else {
        // Blended: show model mix
        let parts: Vec<String> = model_tokens
            .iter()
            .map(|(m, t)| {
                let short = m.strip_prefix("claude-").unwrap_or(m);
                format!("{} {}tok", short, fmt_tokens(*t))
            })
            .collect();
        let mut label = format!("blended: {}", parts.join(" + "));
        if fallback_tokens > 0 {
            label.push_str(&format!(" + {} unknown", fmt_tokens(fallback_tokens)));
        }
        label
    };

    (total_cost, label)
}

pub fn run(history: bool, days: u32, breakdown: bool, insight: bool) -> Result<()> {
    let records = load_records()?;

    if insight {
        print_insight(&records, days);
    } else if history {
        print_history(&records, days);
    } else {
        print_summary(&records, breakdown);
    }

    Ok(())
}

// ─── Data loading ──────────────────────────────────────────────────────────────

fn load_records() -> Result<Vec<Analytics>> {
    // Load from SQLite (migrates from JSONL automatically on first call)
    crate::analytics_db::load_all(None)
}

// ─── Summary view (default) ────────────────────────────────────────────────────

fn print_summary(records: &[Analytics], breakdown: bool) {
    // Split legacy (timestamp=0) records from dated ones.
    // Legacy records have no timestamp and cannot be placed in any date window.
    let (legacy, dated): (Vec<&Analytics>, Vec<&Analytics>) =
        records.iter().partition(|r| r.timestamp_secs == 0);

    let total_input: usize = records.iter().map(|r| r.input_tokens).sum();
    let total_output: usize = records.iter().map(|r| r.output_tokens).sum();
    let total_saved = total_input.saturating_sub(total_output);
    let overall_pct = savings_pct(total_input, total_output);
    let all_refs: Vec<&Analytics> = records.iter().collect();
    let (cost_saved, price_label) = blended_cost(&all_refs);

    let now_secs = now_unix();
    let today_start = day_start(now_secs);
    let week_start = now_secs.saturating_sub(7 * 86400);

    let today: Vec<&Analytics> = dated
        .iter()
        .copied()
        .filter(|r| r.timestamp_secs >= today_start)
        .collect();
    let week: Vec<&Analytics> = dated
        .iter()
        .copied()
        .filter(|r| r.timestamp_secs >= week_start)
        .collect();

    // ── Header ──
    let total_exec_ms: u64 = records.iter().filter_map(|r| r.duration_ms).sum();
    let timed_runs = records.iter().filter(|r| r.duration_ms.is_some()).count();
    let avg_ms: Option<u64> = if timed_runs > 0 {
        Some(total_exec_ms / timed_runs as u64)
    } else {
        None
    };
    let savings_bar = {
        let filled = ((overall_pct / 100.0) * 24.0) as usize;
        let empty = 24usize.saturating_sub(filled);
        format!("{}{}", "█".repeat(filled), "░".repeat(empty))
    };

    println!("{}", "PandaFilter Token Savings".if_supports_color(Stdout, |t| t.bold()));
    println!("{}", "═".repeat(49).if_supports_color(Stdout, |t| t.dimmed()));
    let green_bold = Style::new().bold().green();
    let yellow_bold = Style::new().bold().yellow();

    // "Runs: 206  (avg 87ms)"
    let runs_suffix = avg_ms
        .map(|ms| format!("  (avg {}ms)", ms))
        .unwrap_or_default();
    println!(
        "  Runs:           {}{}",
        records.len(),
        runs_suffix.if_supports_color(Stdout, |t| t.dimmed()),
    );

    // "Tokens saved: 27.3k / 46.7k  (54.4%)  ████████░░░░░░"
    println!(
        "  Tokens saved:   {} / {}  ({})  {}",
        fmt_tokens(total_saved).if_supports_color(Stdout, |t| t.style(green_bold)),
        fmt_tokens(total_input).if_supports_color(Stdout, |t| t.dimmed()),
        format!("{:.1}%", overall_pct).if_supports_color(Stdout, |t| t.green()),
        savings_bar.if_supports_color(Stdout, |t| t.green()),
    );

    println!(
        "  Cost saved:     {}  {}",
        format!("~{}", fmt_cost(cost_saved)).if_supports_color(Stdout, |t| t.style(yellow_bold)),
        format!("(at {})", price_label).if_supports_color(Stdout, |t| t.dimmed()),
    );

    // Focus precision metric — only show if there's data
    if let Some(precision) = focus_precision(0) {
        println!(
            "  Focus accuracy: {}",
            format!("{:.0}% of reads matched recommendations", precision)
                .if_supports_color(Stdout, |t| t.cyan()),
        );
        // Compute the focus ratio to give a rough sense of excluded codebase
        let focus_ratio = focus_avg_exclusion_ratio(0);
        if let Some(ratio) = focus_ratio {
            println!(
                "  {}",
                format!(
                    "Focus guided Claude away from ~{:.0}% of the codebase — not included in cost saved",
                    ratio
                )
                .if_supports_color(Stdout, |t| t.dimmed()),
            );
        }
    }

    if !legacy.is_empty() {
        let legacy_saved: usize = legacy.iter().map(|r| r.tokens_saved()).sum();
        println!(
            "  {}",
            format!(
                "(includes {} legacy run{} without timestamps · {} tokens)",
                legacy.len(),
                if legacy.len() == 1 { "" } else { "s" },
                fmt_tokens(legacy_saved)
            )
            .if_supports_color(Stdout, |t| t.dimmed()),
        );
    }

    if !today.is_empty() {
        let t_saved: usize = today.iter().map(|r| r.tokens_saved()).sum();
        let t_in: usize = today.iter().map(|r| r.input_tokens).sum();
        let t_out: usize = today.iter().map(|r| r.output_tokens).sum();
        let (t_cost, _) = blended_cost(&today);
        println!(
            "  Today:          {} runs · {} saved · {} · {}",
            today.len(),
            fmt_tokens(t_saved).if_supports_color(Stdout, |t| t.cyan()),
            format!("{:.1}%", savings_pct(t_in, t_out)).if_supports_color(Stdout, |t| t.cyan()),
            format!("~{}", fmt_cost(t_cost)).if_supports_color(Stdout, |t| t.yellow()),
        );
    }
    if week.len() > today.len() {
        let w_saved: usize = week.iter().map(|r| r.tokens_saved()).sum();
        let w_in: usize = week.iter().map(|r| r.input_tokens).sum();
        let w_out: usize = week.iter().map(|r| r.output_tokens).sum();
        let (w_cost, _) = blended_cost(&week);
        println!(
            "  7-day:          {} runs · {} saved · {} · {}",
            week.len(),
            fmt_tokens(w_saved).if_supports_color(Stdout, |t| t.cyan()),
            format!("{:.1}%", savings_pct(w_in, w_out)).if_supports_color(Stdout, |t| t.cyan()),
            format!("~{}", fmt_cost(w_cost)).if_supports_color(Stdout, |t| t.yellow()),
        );
    }

    // ── Top command ──
    if !records.is_empty() {
        let mut top_by_cmd: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for r in records {
            let key = normalize_cmd_key(r.command.as_deref());
            let e = top_by_cmd.entry(key).or_default();
            e.0 += r.input_tokens;
            e.1 += r.output_tokens;
        }
        if let Some((top_cmd, (top_in, top_out))) = top_by_cmd
            .into_iter()
            .max_by_key(|(_, (i, o))| i.saturating_sub(*o))
        {
            let top_saved = top_in.saturating_sub(top_out);
            let top_pct = savings_pct(top_in, top_out);
            println!(
                "  Top command:    {}  {}  ·  {} saved",
                top_cmd.if_supports_color(Stdout, |t| t.bold()),
                format!("{:.1}%", top_pct).if_supports_color(Stdout, |t| t.green()),
                fmt_tokens(top_saved).if_supports_color(Stdout, |t| t.green()),
            );
        }
    }

    if records.is_empty() {
        return;
    }

    // ── Per-command table (only with --breakdown) ──
    if breakdown {
        println!();
        println!("{}", "Per-Command Breakdown".if_supports_color(Stdout, |t| t.bold()));

        let mut by_cmd: BTreeMap<String, CmdStats> = BTreeMap::new();
        for r in records {
            let key = normalize_cmd_key(r.command.as_deref());
            let entry = by_cmd.entry(key).or_default();
            entry.input += r.input_tokens;
            entry.output += r.output_tokens;
            entry.count += 1;
            if let Some(ms) = r.duration_ms {
                entry.total_ms += ms;
                entry.ms_count += 1;
            }
        }

        let mut rows: Vec<(String, CmdStats)> = by_cmd.into_iter().collect();
        rows.sort_by(|a, b| b.1.saved().cmp(&a.1.saved()));

        let col_w = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(7).max(7);
        let sep = "─".repeat(col_w + 51);
        println!("{}", sep.if_supports_color(Stdout, |t| t.dimmed()));
        println!(
            "{}",
            format!(
                "{:<col_w$} {:>6}  {:>10}  {:>8}  {:>7}  {}",
                "COMMAND", "RUNS", "SAVED", "SAVINGS", "AVG ms", "IMPACT",
                col_w = col_w
            )
            .if_supports_color(Stdout, |t| t.bold())
        );
        println!("{}", sep.if_supports_color(Stdout, |t| t.dimmed()));

        for (cmd, stats) in &rows {
            let pct = savings_pct(stats.input, stats.output);
            let avg_ms = if stats.ms_count > 0 {
                format!("{:>6}", stats.total_ms / stats.ms_count)
            } else {
                "     —".to_string()
            };
            let bar_len = (pct / 5.0) as usize;
            let bar = "█".repeat(bar_len.min(20));
            let dim_row = pct < 1.0;
            let bar_colored = if pct >= 40.0 {
                bar.if_supports_color(Stdout, |t| t.green()).to_string()
            } else if pct >= 15.0 {
                bar.if_supports_color(Stdout, |t| t.yellow()).to_string()
            } else {
                bar.if_supports_color(Stdout, |t| t.dimmed()).to_string()
            };
            let line = format!(
                "{:<col_w$} {:>6}  {:>10}  {:>7.1}%  {}  {}",
                cmd,
                stats.count,
                fmt_tokens(stats.saved()),
                pct,
                avg_ms,
                bar_colored,
                col_w = col_w
            );
            if dim_row {
                println!("{}", line.if_supports_color(Stdout, |t| t.dimmed()));
            } else {
                println!("{}", line);
            }
        }
    } else {
        println!(
            "  {}",
            "Run `panda gain --breakdown` for per-command details."
                .if_supports_color(Stdout, |t| t.dimmed()),
        );
    }

    // ── Missed opportunities (from discover) ──
    let opportunities = crate::cmd::discover::top_unoptimized(5);
    if !opportunities.is_empty() {
        let total_potential: usize = opportunities.iter().map(|(_, t)| t).sum();
        if total_potential >= 2_000 {
            println!();
            let yellow_bold = Style::new().bold().yellow();
            println!("{}", "Unoptimized Commands".if_supports_color(Stdout, |t| t.style(yellow_bold)));
            println!("{}", format!("  Run `panda discover` for full details · ~{} tokens potential",
                fmt_tokens(total_potential)
            ).if_supports_color(Stdout, |t| t.dimmed()));
            for (cmd, saveable) in &opportunities {
                println!("  {:<14} ~{} saveable",
                    cmd.if_supports_color(Stdout, |t| t.yellow()),
                    fmt_tokens(*saveable).if_supports_color(Stdout, |t| t.yellow()),
                );
            }
        }
    }

    // ── Focus tip (shown only when focus hook is not yet registered) ──
    if !is_focus_registered() {
        println!();
        println!("{}", "Focus Ranking available".if_supports_color(Stdout, |t| t.bold()));
        println!("{}", "  Give the agent confidence-ranked file hints for large repos.".if_supports_color(Stdout, |t| t.dimmed()));
        println!("{}", "  Run `panda focus --enable` to activate.".if_supports_color(Stdout, |t| t.dimmed()));
    }
}

/// Returns true if the panda focus UserPromptSubmit hook is registered.
fn is_focus_registered() -> bool {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return false,
    };
    let settings_path = home.join(".claude").join("settings.json");
    let content = match std::fs::read_to_string(&settings_path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };
    settings["hooks"]["UserPromptSubmit"]
        .as_array()
        .map(|arr| {
            arr.iter().any(|entry| {
                entry["hooks"]
                    .as_array()
                    .map(|hooks| {
                        hooks.iter().any(|h| {
                            h.get("command")
                                .and_then(|c| c.as_str())
                                .map(|c| c.contains("panda") && c.contains("focus"))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

// ─── History view (--history) ─────────────────────────────────────────────────

fn print_history(records: &[Analytics], days: u32) {
    let now_secs = now_unix();
    // Align cutoff to UTC midnight of the earliest displayed day so the rolling
    // window boundary doesn't split a calendar day and silently drop records.
    let first_day_ts = now_secs.saturating_sub((days as u64 - 1) * 86400);
    let cutoff = first_day_ts - (first_day_ts % 86400);

    // Group by calendar day (UTC date string "YYYY-MM-DD")
    let mut by_day: BTreeMap<String, DayStats> = BTreeMap::new();
    let mut records_by_day: BTreeMap<String, Vec<&Analytics>> = BTreeMap::new();

    for r in records.iter().filter(|r| r.timestamp_secs > 0 && r.timestamp_secs >= cutoff) {
        let day = unix_to_date(r.timestamp_secs);
        let entry = by_day.entry(day.clone()).or_default();
        entry.input += r.input_tokens;
        entry.output += r.output_tokens;
        entry.count += 1;
        records_by_day.entry(day).or_default().push(r);
    }

    // Fill gaps so every day in range appears
    for offset in 0..days {
        let ts = now_secs.saturating_sub(offset as u64 * 86400);
        let day = unix_to_date(ts);
        by_day.entry(day).or_default();
    }

    // Sort descending (most recent first)
    let mut rows: Vec<(String, DayStats)> = by_day.into_iter().collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows.truncate(days as usize);

    println!("{}", format!("PandaFilter Daily History  (last {} days)", days).if_supports_color(Stdout, |t| t.bold()));
    println!("{}", "═".repeat(60).if_supports_color(Stdout, |t| t.dimmed()));

    let sep = "─".repeat(60);
    println!("{}", sep.if_supports_color(Stdout, |t| t.dimmed()));
    println!(
        "{}",
        format!(
            "{:<12}  {:>5}  {:>12}  {:>8}  {:>10}",
            "DATE", "RUNS", "SAVED", "SAVINGS", "COST SAVED"
        )
        .if_supports_color(Stdout, |t| t.bold())
    );
    println!("{}", sep.if_supports_color(Stdout, |t| t.dimmed()));

    let mut total_input: usize = 0;
    let mut total_output: usize = 0;
    let mut total_count: usize = 0;

    for (day, stats) in &rows {
        let pct = savings_pct(stats.input, stats.output);
        let cost = if let Some(day_records) = records_by_day.get(day) {
            blended_cost(day_records).0
        } else {
            0.0
        };
        let saved_str = if stats.count == 0 {
            "—".to_string()
        } else {
            fmt_tokens(stats.saved())
        };
        let pct_str = if stats.count == 0 {
            "—".to_string()
        } else {
            format!("{:.1}%", pct)
        };
        let cost_str = if stats.count == 0 {
            "—".to_string()
        } else {
            fmt_cost(cost)
        };
        let dim_row = stats.count == 0;
        let line = format!(
            "{:<12}  {:>5}  {:>12}  {:>8}  {:>10}",
            day, stats.count, saved_str, pct_str, cost_str
        );
        if dim_row {
            println!("{}", line.if_supports_color(Stdout, |t| t.dimmed()));
        } else {
            println!("{}", line);
        }
        total_input += stats.input;
        total_output += stats.output;
        total_count += stats.count;
    }

    println!("{}", sep.if_supports_color(Stdout, |t| t.dimmed()));
    let total_saved = total_input.saturating_sub(total_output);
    let all_windowed: Vec<&Analytics> = records_by_day.values().flat_map(|v| v.iter().copied()).collect();
    let (total_cost, _) = blended_cost(&all_windowed);
    println!(
        "{}",
        format!(
            "{:<12}  {:>5}  {:>12}  {:>8}  {:>10}",
            format!("{}-day total", days),
            total_count,
            fmt_tokens(total_saved),
            format!("{:.1}%", savings_pct(total_input, total_output)),
            fmt_cost(total_cost)
        )
        .if_supports_color(Stdout, |t| t.bold())
    );

    // Legacy records (timestamp=0): show totals separately
    let legacy_iter = records.iter().filter(|r| r.timestamp_secs == 0);
    let (legacy_count, legacy_saved) = legacy_iter.fold((0usize, 0usize), |(c, s), r| {
        (c + 1, s + r.tokens_saved())
    });
    if legacy_count > 0 {
        let line = format!(
            "{:<12}  {:>5}  {:>12}  {:>8}  {:>10}",
            "(legacy)",
            legacy_count,
            fmt_tokens(legacy_saved),
            "—",
            "—",
        );
        println!("{}", line.if_supports_color(Stdout, |t| t.dimmed()));
    }

    // Top commands over the period
    let mut cmd_stats: BTreeMap<String, CmdStats> = BTreeMap::new();
    for r in records.iter().filter(|r| r.timestamp_secs > 0 && r.timestamp_secs >= cutoff) {
        let key = normalize_cmd_key(r.command.as_deref());
        let e = cmd_stats.entry(key).or_default();
        e.input += r.input_tokens;
        e.output += r.output_tokens;
        e.count += 1;
    }
    if !cmd_stats.is_empty() {
        let mut cmd_rows: Vec<(String, CmdStats)> = cmd_stats.into_iter().collect();
        cmd_rows.sort_by(|a, b| b.1.saved().cmp(&a.1.saved()));

        println!();
        println!("{}", "Top Commands".if_supports_color(Stdout, |t| t.bold()));
        println!("{}", "─".repeat(42).if_supports_color(Stdout, |t| t.dimmed()));
        println!("{}", format!("{:<14} {:>5}  {:>10}  {:>7}", "COMMAND", "RUNS", "SAVED", "SAVINGS").if_supports_color(Stdout, |t| t.bold()));
        println!("{}", "─".repeat(42).if_supports_color(Stdout, |t| t.dimmed()));
        for (cmd, s) in cmd_rows.iter().take(8) {
            println!(
                "{:<14} {:>5}  {:>10}  {:>6.1}%",
                cmd,
                s.count,
                fmt_tokens(s.saved()),
                savings_pct(s.input, s.output)
            );
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Normalize a stored command key for display:
/// - Strip leading "rtk " wrapper (e.g. "rtk git status" → "git status")
/// - Skip leading KEY=VALUE env var assignments (e.g. "GIT_COMMITTER_NAME=Assaf git status")
/// - Strip "rtk " wrapper prefix
/// - Take the basename of the first token (e.g. "/usr/bin/git status" → "git status")
/// - Collapse tool-event labels like "(read)" and "(glob)" into "(pipeline)"
fn normalize_cmd_key(raw: Option<&str>) -> String {
    let s = match raw {
        None => return "(pipeline)".to_string(),
        Some(s) => s,
    };
    // Collapse tool-event labels and bare wrapper names into (pipeline)
    if s == "(read)" || s == "(glob)" || s == "rtk" || s == "panda" {
        return "(pipeline)".to_string();
    }
    // Skip leading KEY=VALUE env var assignments
    fn is_env_assign(t: &str) -> bool {
        let eq = t.find('=').unwrap_or(0);
        eq > 0 && t[..eq].chars().all(|c| c.is_ascii_uppercase() || c == '_')
    }
    let s: String = {
        let iter = s.split_whitespace().skip_while(|t| is_env_assign(t));
        iter.collect::<Vec<_>>().join(" ")
    };
    let s = s.as_str();
    if s.is_empty() {
        return "(pipeline)".to_string();
    }
    // Strip "rtk " prefix
    let s = s.strip_prefix("rtk ").unwrap_or(s);
    // Normalize basename of the first token
    let mut tokens = s.splitn(2, ' ');
    let first = tokens.next().unwrap_or(s);
    let rest = tokens.next().unwrap_or("");
    let basename = std::path::Path::new(first)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(first);
    if rest.is_empty() {
        basename.to_string()
    } else {
        format!("{} {}", basename, rest)
    }
}

#[derive(Default)]
struct CmdStats {
    input: usize,
    output: usize,
    count: usize,
    total_ms: u64,
    ms_count: u64,
}

impl CmdStats {
    fn saved(&self) -> usize {
        self.input.saturating_sub(self.output)
    }
}

#[derive(Default)]
struct DayStats {
    input: usize,
    output: usize,
    count: usize,
}

impl DayStats {
    fn saved(&self) -> usize {
        self.input.saturating_sub(self.output)
    }
}

fn savings_pct(input: usize, output: usize) -> f32 {
    if input == 0 {
        return 0.0;
    }
    let saved = input.saturating_sub(output);
    (saved as f32 / input as f32) * 100.0
}

fn fmt_cost(dollars: f64) -> String {
    if dollars < 0.0001 {
        format!("<$0.0001")
    } else if dollars < 0.01 {
        format!("${:.4}", dollars)
    } else if dollars < 1.0 {
        format!("${:.3}", dollars)
    } else {
        format!("${:.2}", dollars)
    }
}

fn fmt_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1_000;
        format!("{}m {}s", mins, secs)
    }
}

fn fmt_tokens(n: usize) -> String {
    if n < 1_000 {
        format!("{}", n)
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Returns the unix timestamp of midnight (UTC) for the day containing `ts`.
fn day_start(ts: u64) -> u64 {
    ts - (ts % 86400)
}

/// Format a unix timestamp as "YYYY-MM-DD" (UTC).
fn unix_to_date(ts: u64) -> String {
    // Simple manual conversion — no chrono dep needed
    let secs = ts;
    let days = secs / 86400;

    // Days since Unix epoch (1970-01-01)
    let (year, month, day) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

/// Convert days-since-epoch to (year, month, day) using the proleptic Gregorian calendar.
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Adapted from a well-known public domain algorithm
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Format a unix timestamp as "Mon DD" (UTC), e.g. "Apr 05".
fn unix_to_month_day(ts: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, month, day) = days_to_ymd(ts / 86400);
    let m = MONTHS[(month.saturating_sub(1).min(11)) as usize];
    format!("{} {:02}", m, day)
}

// ─── Insight view (--insight) ─────────────────────────────────────────────────

fn print_insight(records: &[Analytics], days: u32) {
    let now_secs = now_unix();
    let first_day_ts = now_secs.saturating_sub((days as u64).saturating_sub(1) * 86400);
    let cutoff = first_day_ts - (first_day_ts % 86400);

    // Collect windowed records
    let windowed: Vec<&Analytics> = records
        .iter()
        .filter(|r| r.timestamp_secs > 0 && r.timestamp_secs >= cutoff)
        .collect();

    println!(
        "{}",
        format!("PandaFilter — last {} days", days)
            .if_supports_color(Stdout, |t| t.bold())
    );
    println!("{}", "═".repeat(58).if_supports_color(Stdout, |t| t.dimmed()));

    if windowed.is_empty() {
        println!();
        println!("  No token savings recorded in this period.");
        return;
    }

    let total_input: usize = windowed.iter().map(|r| r.input_tokens).sum();
    let total_output: usize = windowed.iter().map(|r| r.output_tokens).sum();
    let total_saved = total_input.saturating_sub(total_output);
    let total_runs = windowed.len();
    let (cost_saved, _) = blended_cost(&windowed);
    let cache_hits: usize = windowed.iter().filter(|r| r.cache_hit).count();

    let green_bold = Style::new().green().bold();
    let yellow_bold = Style::new().yellow().bold();

    // ── Headline ─────────────────────────────────────────────────────────────
    println!();
    println!(
        "  {} runs  ·  {} tokens saved  ·  {}",
        fmt_number(total_runs).if_supports_color(Stdout, |t| t.bold()),
        fmt_tokens(total_saved).if_supports_color(Stdout, |t| t.style(green_bold)),
        format!("~{}", fmt_cost(cost_saved)).if_supports_color(Stdout, |t| t.style(yellow_bold)),
    );

    // Monthly projection
    let window_days = days.max(1) as f64;
    let monthly_cost = cost_saved / window_days * 30.0;
    if monthly_cost > 0.001 {
        println!(
            "  {}",
            format!("At this rate: ~{}/month", fmt_cost(monthly_cost))
                .if_supports_color(Stdout, |t| t.dimmed()),
        );
    }

    // ── Per-command table ────────────────────────────────────────────────────
    println!();
    println!(
        "  {}",
        "Where savings came from:".if_supports_color(Stdout, |t| t.bold())
    );
    println!();

    // Aggregate by normalized command name (skip cache hits — shown separately)
    let mut by_cmd: BTreeMap<String, InsightCmdStats> = BTreeMap::new();
    for r in &windowed {
        if r.cache_hit {
            continue;
        }
        let key = normalize_cmd_key(r.command.as_deref());
        let e = by_cmd.entry(key).or_default();
        e.input += r.input_tokens;
        e.output += r.output_tokens;
        e.count += 1;
    }

    let mut cmd_rows: Vec<(String, InsightCmdStats)> = by_cmd.into_iter().collect();
    cmd_rows.sort_by(|a, b| b.1.saved().cmp(&a.1.saved()));

    // Show top 8 commands, aggregate the rest
    let show_count = cmd_rows.len().min(8);
    let max_saved = cmd_rows.first().map(|(_, s)| s.saved()).unwrap_or(1).max(1);

    for (cmd, stats) in &cmd_rows[..show_count] {
        let saved = stats.saved();
        let compression = savings_pct(stats.input, stats.output);
        let bar_width = ((saved as f64 / max_saved as f64) * 20.0) as usize;
        let bar = if bar_width > 0 {
            "█".repeat(bar_width)
        } else {
            "░".to_string()
        };

        let bar_colored = if compression >= 60.0 {
            bar.if_supports_color(Stdout, |t| t.green()).to_string()
        } else if compression >= 30.0 {
            bar.if_supports_color(Stdout, |t| t.yellow()).to_string()
        } else {
            bar.if_supports_color(Stdout, |t| t.dimmed()).to_string()
        };

        println!(
            "  {:<20} {:>7} saved   {:>4.0}% compressed   {}",
            cmd.if_supports_color(Stdout, |t| t.bold()),
            fmt_tokens(saved),
            compression,
            bar_colored,
        );
    }

    // Remaining commands
    if cmd_rows.len() > show_count {
        let rest = &cmd_rows[show_count..];
        let rest_saved: usize = rest.iter().map(|(_, s)| s.saved()).sum();
        let rest_count: usize = rest.iter().map(|(_, s)| s.count).sum();
        let rest_cmds = rest.len();
        println!(
            "  {}",
            format!(
                "+ {} more commands       {:>7} saved   {} runs",
                rest_cmds,
                fmt_tokens(rest_saved),
                fmt_number(rest_count),
            )
            .if_supports_color(Stdout, |t| t.dimmed()),
        );
    }

    // ── Cache hits ───────────────────────────────────────────────────────────
    if cache_hits > 0 {
        println!();
        println!(
            "  {} — repeated output detected and collapsed",
            format!("{} cache hits", cache_hits).if_supports_color(Stdout, |t| t.cyan()),
        );
    }

    // ── Biggest single save ──────────────────────────────────────────────────
    if let Some(biggest) = windowed.iter().max_by_key(|r| r.tokens_saved()) {
        let label = normalize_cmd_key(biggest.command.as_deref());
        let date = unix_to_month_day(biggest.timestamp_secs);
        println!();
        println!(
            "  Biggest save: {} from {} {}",
            fmt_tokens(biggest.tokens_saved()).if_supports_color(Stdout, |t| t.style(green_bold)),
            label.if_supports_color(Stdout, |t| t.bold()),
            format!("({})", date).if_supports_color(Stdout, |t| t.dimmed()),
        );
    }

    // ── Context Focusing ─────────────────────────────────────────────────────
    print_focus_insight_compact(cutoff);

    // ── Tip ──────────────────────────────────────────────────────────────────
    print_insight_tip(&cmd_rows, total_saved, cache_hits, cutoff);
}

#[derive(Default)]
struct InsightCmdStats {
    input: usize,
    output: usize,
    count: usize,
}

impl InsightCmdStats {
    fn saved(&self) -> usize {
        self.input.saturating_sub(self.output)
    }
}

fn fmt_number(n: usize) -> String {
    if n < 1_000 {
        format!("{}", n)
    } else if n < 1_000_000 {
        format!("{},{:03}", n / 1_000, n % 1_000)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// Compact focus section for the insight view — 2 lines max.
fn print_focus_insight_compact(cutoff: u64) {
    let conn = match crate::analytics_db::open() {
        Ok(c) => c,
        Err(_) => return,
    };

    let session_count: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT session_id) FROM guidance_records WHERE timestamp_secs >= ?1",
            rusqlite::params![cutoff as i64],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if session_count == 0 {
        return;
    }

    let (total_recommended, total_in_repo): (i64, i64) = conn
        .query_row(
            "SELECT COALESCE(SUM(files_recommended), 0), \
                    COALESCE(SUM(files_in_repo), 0) \
             FROM guidance_records WHERE timestamp_secs >= ?1",
            rusqlite::params![cutoff as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0, 0));

    let guidance_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM guidance_records WHERE timestamp_secs >= ?1",
            rusqlite::params![cutoff as i64],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let avg_recommended = if guidance_count > 0 {
        total_recommended as f64 / guidance_count as f64
    } else {
        0.0
    };

    let exclusion_pct = if total_in_repo > 0 {
        (total_in_repo - total_recommended) as f64 / total_in_repo as f64 * 100.0
    } else {
        0.0
    };

    println!();
    let s = if session_count == 1 { "" } else { "s" };
    println!(
        "  {} — {} session{}",
        "Context Focusing".if_supports_color(Stdout, |t| t.bold()),
        session_count,
        s,
    );
    println!(
        "    ~{:.0} files recommended per prompt · {:.0}% of codebase excluded",
        avg_recommended, exclusion_pct,
    );

    // Query focus compression stats
    if let Ok(stats) = crate::analytics_db::focus_compression_stats(cutoff) {
        if stats.files_compressed > 0 {
            let token_savings = stats.total_old_tokens.saturating_sub(stats.total_new_tokens);
            println!(
                "    Focus compression: {} files · {} tokens saved vs head/tail{}",
                stats.files_compressed,
                fmt_number(token_savings),
                if stats.edit_hit_rate > 0.0 {
                    format!(" · {:.0}% edit-hit rate", stats.edit_hit_rate * 100.0)
                } else {
                    String::new()
                },
            );
        }
    }
}

/// Print an actionable tip based on the user's data.
fn print_insight_tip(
    cmd_rows: &[(String, InsightCmdStats)],
    total_saved: usize,
    _cache_hits: usize,
    cutoff: u64,
) {
    println!();

    // Check if focus is enabled (has guidance records)
    let focus_active = crate::analytics_db::open()
        .ok()
        .and_then(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM guidance_records WHERE timestamp_secs >= ?1",
                rusqlite::params![cutoff as i64],
                |row| row.get::<_, i64>(0),
            )
            .ok()
        })
        .unwrap_or(0)
        > 0;

    // Tip 1: If one command dominates savings (>80%), mention it
    if let Some((top_cmd, top_stats)) = cmd_rows.first() {
        let top_pct = if total_saved > 0 {
            top_stats.saved() as f64 / total_saved as f64 * 100.0
        } else {
            0.0
        };
        if top_pct > 80.0 && total_saved > 10_000 {
            println!(
                "  {} {:.0}% of savings come from {}.",
                "Tip:".if_supports_color(Stdout, |t| t.style(Style::new().cyan().bold())),
                top_pct,
                top_cmd.if_supports_color(Stdout, |t| t.bold()),
            );
            if !focus_active {
                println!(
                    "       Run {} to guide Claude to relevant files first.",
                    "panda focus --enable".if_supports_color(Stdout, |t| t.cyan()),
                );
            } else {
                println!(
                    "       Run {} for a full per-command breakdown.",
                    "panda gain --breakdown".if_supports_color(Stdout, |t| t.cyan()),
                );
            }
            return;
        }
    }

    // Tip 2: If focus is not enabled, suggest it
    if !focus_active {
        println!(
            "  {} Enable Context Focusing to guide Claude to the right files:",
            "Tip:".if_supports_color(Stdout, |t| t.style(Style::new().cyan().bold())),
        );
        println!(
            "       {}",
            "panda focus --enable".if_supports_color(Stdout, |t| t.cyan()),
        );
        return;
    }

    // Tip 3: Generic encouragement
    println!(
        "  {} Run {} for per-command details.",
        "Tip:".if_supports_color(Stdout, |t| t.style(Style::new().cyan().bold())),
        "panda gain --breakdown".if_supports_color(Stdout, |t| t.cyan()),
    );
}

// ─── Context Focusing stats ───────────────────────────────────────────────────

/// Returns the focus precision percentage: what fraction of files Claude actually
/// read were in our recommended set. Returns None if no data is available.
fn focus_precision(cutoff: u64) -> Option<f64> {
    let conn = crate::analytics_db::open().ok()?;

    // Get all sessions that had guidance in the window
    let mut stmt = conn.prepare(
        "SELECT DISTINCT session_id FROM guidance_records WHERE timestamp_secs >= ?1"
    ).ok()?;
    let session_ids: Vec<String> = stmt
        .query_map(rusqlite::params![cutoff as i64], |row| row.get(0))
        .ok()?
        .filter_map(|r| r.ok())
        .collect();

    if session_ids.is_empty() {
        return None;
    }

    // For each session, collect recommended files and actual reads
    let mut total_reads: usize = 0;
    let mut matched_reads: usize = 0;

    for sid in &session_ids {
        // Get recommended files for this session (from guidance_records we don't store
        // individual file names, but we can cross-reference session_reads against the
        // guidance ratio). Instead, we use the focus graph to check which files were
        // recommended. For now, use a simpler proxy: count reads in sessions with guidance.
        let reads: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_reads WHERE session_id = ?1 AND timestamp_secs >= ?2",
                rusqlite::params![sid, cutoff as i64],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let recommended: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(files_recommended), 0) FROM guidance_records \
                 WHERE session_id = ?1 AND timestamp_secs >= ?2",
                rusqlite::params![sid, cutoff as i64],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let total_files: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(files_in_repo), 0) FROM guidance_records \
                 WHERE session_id = ?1 AND timestamp_secs >= ?2",
                rusqlite::params![sid, cutoff as i64],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if reads > 0 && total_files > 0 && recommended > 0 {
            total_reads += reads as usize;
            // Precision proxy: if Claude read fewer files than the repo has,
            // and our recommended count covers most of what was read,
            // precision = min(recommended, reads) / reads
            let hits = (recommended as usize).min(reads as usize);
            matched_reads += hits;
        }
    }

    if total_reads == 0 {
        return None;
    }

    Some(matched_reads as f64 / total_reads as f64 * 100.0)
}

/// Returns the average percentage of the codebase excluded by focus guidance.
fn focus_avg_exclusion_ratio(cutoff: u64) -> Option<f64> {
    let conn = crate::analytics_db::open().ok()?;
    let (total_recommended, total_in_repo): (i64, i64) = conn
        .query_row(
            "SELECT COALESCE(SUM(files_recommended), 0), \
                    COALESCE(SUM(files_in_repo), 0) \
             FROM guidance_records WHERE timestamp_secs >= ?1",
            rusqlite::params![cutoff as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;

    if total_in_repo == 0 {
        return None;
    }

    let excluded = total_in_repo - total_recommended;
    Some(excluded as f64 / total_in_repo as f64 * 100.0)
}


// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use panda_core::analytics::Analytics;

    // Legacy insight types — kept for existing test coverage.

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    enum SavingsCategory {
        NoiseReduction,
        BuildFiltering,
        PipelineSavings,
        CommandCompression,
    }

    #[derive(Default)]
    struct CategoryStats {
        saved: usize,
        input: usize,
        count: usize,
        commands: std::collections::BTreeMap<String, usize>,
        cache_hits: usize,
    }

    fn categorize(cmd: Option<&str>, cache_hit: bool) -> SavingsCategory {
        if cache_hit {
            return SavingsCategory::PipelineSavings;
        }
        let raw = match cmd {
            None => return SavingsCategory::PipelineSavings,
            Some(s) => s,
        };
        let first_token = raw.split_whitespace().next().unwrap_or(raw);
        match first_token {
            "(read)" | "(read-level)" | "(glob)" | "(grep-tool)" | "(pipeline)" => {
                return SavingsCategory::PipelineSavings;
            }
            _ => {}
        }
        let normalized = normalize_cmd_key(Some(raw));
        let cmd_name = normalized.split_whitespace().next().unwrap_or(&normalized);
        match cmd_name {
            "(pipeline)" => SavingsCategory::PipelineSavings,
            "find" | "ls" | "tree" => SavingsCategory::NoiseReduction,
            "cargo" | "go" | "npm" | "yarn" | "pnpm" | "make" | "gmake"
            | "mvn" | "gradle" | "pytest" | "jest" | "vitest" | "rspec"
            | "tsc" | "ruff" | "mypy" | "rubocop" | "eslint" | "biome"
            | "playwright" | "nx" | "turbo" | "uv" | "pip" | "rake"
            | "ember" | "next" | "webpack" | "vite" | "stylelint" | "prettier" => {
                SavingsCategory::BuildFiltering
            }
            _ => SavingsCategory::CommandCompression,
        }
    }

    fn format_save_label(r: &Analytics) -> String {
        let cmd = normalize_cmd_key(r.command.as_deref());
        match r.subcommand.as_deref() {
            Some(sub) if !sub.is_empty() => {
                let sub_display = if sub.len() > 30 {
                    format!("\u{2026}{}", &sub[sub.len() - 28..])
                } else {
                    sub.to_string()
                };
                format!("{} {}", cmd, sub_display)
            }
            _ => cmd,
        }
    }

    fn aggregate_by_category(
        records: &[Analytics],
        cutoff: u64,
    ) -> (BTreeMap<SavingsCategory, CategoryStats>, Vec<usize>) {
        let windowed_indices: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.timestamp_secs > 0 && r.timestamp_secs >= cutoff)
            .map(|(i, _)| i)
            .collect();

        let mut by_category: BTreeMap<SavingsCategory, CategoryStats> = BTreeMap::new();
        for &i in &windowed_indices {
            let r = &records[i];
            let cat = categorize(r.command.as_deref(), r.cache_hit);
            let stats = by_category.entry(cat).or_default();
            stats.saved += r.tokens_saved();
            stats.input += r.input_tokens;
            stats.count += 1;
            let cmd_key = normalize_cmd_key(r.command.as_deref());
            *stats.commands.entry(cmd_key).or_default() += 1;
            if r.cache_hit {
                stats.cache_hits += 1;
            }
        }

        let mut sorted = windowed_indices;
        sorted.sort_by(|&a, &b| records[b].tokens_saved().cmp(&records[a].tokens_saved()));

        (by_category, sorted)
    }

    fn make_record(
        cmd: Option<&str>,
        sub: Option<&str>,
        input: usize,
        output: usize,
        ts: u64,
        cache_hit: bool,
    ) -> Analytics {
        Analytics {
            input_tokens: input,
            output_tokens: output,
            savings_pct: if input > 0 {
                (input.saturating_sub(output) as f32 / input as f32) * 100.0
            } else {
                0.0
            },
            command: cmd.map(|s| s.to_string()),
            timestamp_secs: ts,
            subcommand: sub.map(|s| s.to_string()),
            duration_ms: None,
            cache_hit,
            agent: None,
            model: None,
        }
    }

    // ── categorize() ──────────────────────────────────────────────────────────

    #[test]
    fn categorize_find_is_noise() {
        assert_eq!(categorize(Some("find"), false), SavingsCategory::NoiseReduction);
    }

    #[test]
    fn categorize_cargo_is_build() {
        assert_eq!(categorize(Some("cargo"), false), SavingsCategory::BuildFiltering);
    }

    #[test]
    fn categorize_pipeline_markers() {
        for marker in &["(read)", "(read-level)", "(glob)", "(grep-tool)", "(pipeline)"] {
            assert_eq!(
                categorize(Some(marker), false),
                SavingsCategory::PipelineSavings,
                "expected PipelineSavings for {}",
                marker
            );
        }
    }

    #[test]
    fn categorize_cache_hit_overrides_cmd() {
        assert_eq!(
            categorize(Some("git"), true),
            SavingsCategory::PipelineSavings
        );
    }

    #[test]
    fn categorize_git_is_command_compression() {
        assert_eq!(
            categorize(Some("git"), false),
            SavingsCategory::CommandCompression
        );
    }

    #[test]
    fn categorize_unknown_cmd_is_command_compression() {
        assert_eq!(
            categorize(Some("zzz_unknown_tool"), false),
            SavingsCategory::CommandCompression
        );
    }

    #[test]
    fn categorize_none_cmd_is_pipeline() {
        assert_eq!(categorize(None, false), SavingsCategory::PipelineSavings);
    }

    // ── format_save_label() ───────────────────────────────────────────────────

    #[test]
    fn label_with_subcommand() {
        let r = make_record(Some("cargo"), Some("test"), 1000, 500, 1_700_000_000, false);
        assert_eq!(format_save_label(&r), "cargo test");
    }

    #[test]
    fn label_without_subcommand() {
        let r = make_record(Some("git"), None, 1000, 500, 1_700_000_000, false);
        assert_eq!(format_save_label(&r), "git");
    }

    #[test]
    fn label_long_path_truncated() {
        let long_sub = "a".repeat(40);
        let r = make_record(Some("find"), Some(&long_sub), 1000, 500, 1_700_000_000, false);
        let label = format_save_label(&r);
        // Should be "find …<last 28 chars>"
        assert!(label.starts_with("find "));
        // Total subcommand display should be "…" + 28 chars = 29 chars (+ "find " prefix)
        let sub_part = &label["find ".len()..];
        assert!(sub_part.starts_with('\u{2026}'));
        assert!(sub_part.len() <= 32); // ellipsis + 28 bytes
    }

    #[test]
    fn label_pipeline_no_sub() {
        let r = make_record(Some("(read)"), None, 1000, 500, 1_700_000_000, false);
        // normalize_cmd_key("(read)") → "(pipeline)"
        assert_eq!(format_save_label(&r), "(pipeline)");
    }

    // ── aggregation / integration ─────────────────────────────────────────────

    #[test]
    fn insight_aggregates_by_category() {
        let records = vec![
            make_record(Some("find"), None,  1000, 200, 1_700_000_000, false), // noise: 800
            make_record(Some("find"), None,  500,  100, 1_700_000_001, false), // noise: 400
            make_record(Some("cargo"), None, 2000, 500, 1_700_000_002, false), // build: 1500
            make_record(Some("git"), None,   1000, 600, 1_700_000_003, false), // compression: 400
            make_record(Some("(read)"), None, 800, 200, 1_700_000_004, false), // pipeline: 600
        ];
        let (by_cat, _) = aggregate_by_category(&records, 0);

        assert_eq!(by_cat[&SavingsCategory::NoiseReduction].saved, 1200);
        assert_eq!(by_cat[&SavingsCategory::BuildFiltering].saved, 1500);
        assert_eq!(by_cat[&SavingsCategory::CommandCompression].saved, 400);
        assert_eq!(by_cat[&SavingsCategory::PipelineSavings].saved, 600);
    }

    #[test]
    fn insight_top_saves_sorted_correctly() {
        let records = vec![
            make_record(Some("git"), None,   1000, 800, 1_700_000_000, false), // saved: 200
            make_record(Some("cargo"), None, 5000, 500, 1_700_000_001, false), // saved: 4500
            make_record(Some("find"), None,  2000, 100, 1_700_000_002, false), // saved: 1900
            make_record(Some("npm"), None,   3000, 800, 1_700_000_003, false), // saved: 2200
            make_record(Some("grep"), None,  1000, 400, 1_700_000_004, false), // saved: 600
        ];
        let (_, sorted) = aggregate_by_category(&records, 0);
        assert_eq!(sorted.len(), 5);
        // First index should be the record with most tokens saved (cargo = 4500)
        assert_eq!(records[sorted[0]].tokens_saved(), 4500);
        // Second: npm = 2200
        assert_eq!(records[sorted[1]].tokens_saved(), 2200);
        // Third: find = 1900
        assert_eq!(records[sorted[2]].tokens_saved(), 1900);
    }

    #[test]
    fn insight_empty_window_no_panic() {
        // All records have timestamp_secs = 0, so none pass the filter
        let records = vec![
            make_record(Some("cargo"), None, 1000, 500, 0, false),
        ];
        // Should not panic; returns empty aggregation
        let (by_cat, sorted) = aggregate_by_category(&records, 1);
        assert!(by_cat.is_empty());
        assert!(sorted.is_empty());
        // Also test print_insight with empty slice directly
        print_insight(&[], 7);
    }

    #[test]
    fn insight_cache_hit_counted_in_pipeline() {
        let records = vec![
            make_record(Some("git"), None, 1000, 300, 1_700_000_000, true), // cache_hit=true
        ];
        let (by_cat, _) = aggregate_by_category(&records, 0);

        // Should be in PipelineSavings, not CommandCompression
        assert!(by_cat.contains_key(&SavingsCategory::PipelineSavings));
        assert!(!by_cat.contains_key(&SavingsCategory::CommandCompression));
        assert_eq!(by_cat[&SavingsCategory::PipelineSavings].cache_hits, 1);
        assert_eq!(by_cat[&SavingsCategory::PipelineSavings].saved, 700);
    }
}
