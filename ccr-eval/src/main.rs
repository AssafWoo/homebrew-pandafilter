mod bench;
mod bench_report;
mod runner;
mod report;

use anyhow::Result;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // ── Retrieval benchmark mode: panda-eval --bench [--clone] ──────────────
    if args.iter().any(|a| a == "--bench") {
        let bench_dir = bench_dir();
        let do_clone = args.iter().any(|a| a == "--clone");

        if do_clone {
            println!("Cloning 18 benchmark repos and building indexes …");
            println!("(This may take 5-15 minutes and ~1-2 GB of disk space)");
            println!();
            bench::clone_and_index(&bench_dir)?;
        }

        println!("Running benchmark ({} repos) …", bench::BENCH_REPOS.len());
        println!();
        let results = bench::run_benchmark(&bench_dir)?;

        if results.is_empty() {
            eprintln!("No results — run with --clone first to clone and index the repos.");
            std::process::exit(1);
        }

        bench_report::print_and_save(&results, &bench_dir);
        return Ok(());
    }

    // ── Savings-only mode: compression metrics without Claude API ────────────
    // Usage: panda-eval --savings-only [--fixtures-dir <dir>]
    if args.iter().any(|a| a == "--savings-only") {
        let fixtures_dir = fixtures_dir_from_args(&args);
        let fixture_pairs = runner::discover_fixtures(&fixtures_dir)?;

        println!("PandaFilter — Compression Savings Report (no API key required)");
        println!("================================================================");
        println!("Fixtures dir: {}", fixtures_dir.display());
        println!();
        println!("{:<28} {:>8} {:>8} {:>9} {:>7} {:>7}  handler",
            "fixture", "tok-in", "tok-out", "savings%", "lines↓", "recall");
        println!("{}", "-".repeat(85));

        let mut total_in = 0usize;
        let mut total_out = 0usize;
        let mut total_facts = 0usize;
        let mut total_found = 0usize;

        for (txt_path, qa_path) in &fixture_pairs {
            match runner::run_fixture_savings(txt_path, qa_path) {
                Ok(r) => {
                    let recall = if r.facts_total == 0 { 100.0 } else {
                        r.facts_found as f32 / r.facts_total as f32 * 100.0
                    };
                    let savings_sign = if r.savings_pct >= 0.0 { "+" } else { "" };
                    println!("{:<28} {:>8} {:>8} {:>8}{}% {:>6}→{:<5}  {}",
                        &r.name[..r.name.len().min(28)],
                        r.input_tokens,
                        r.output_tokens,
                        savings_sign,
                        r.savings_pct as i32,
                        r.lines_in,
                        r.lines_out,
                        r.handler_name,
                    );
                    if recall < 100.0 {
                        println!("  ⚠ recall {}/{} ({:.0}%) — compressed output missing key facts",
                            r.facts_found, r.facts_total, recall);
                    }
                    total_in += r.input_tokens;
                    total_out += r.output_tokens;
                    total_facts += r.facts_total;
                    total_found += r.facts_found;
                }
                Err(e) => println!("{:<28}  ERROR: {}", txt_path.display(), e),
            }
        }

        println!("{}", "-".repeat(85));
        let overall_savings = if total_in == 0 { 0.0 } else {
            (total_in.saturating_sub(total_out)) as f32 / total_in as f32 * 100.0
        };
        let overall_recall = if total_facts == 0 { 100.0 } else {
            total_found as f32 / total_facts as f32 * 100.0
        };
        println!("{:<28} {:>8} {:>8} {:>8}%  overall recall {}/{}  ({:.0}%)",
            "TOTAL",
            total_in, total_out,
            overall_savings as i32,
            total_found, total_facts,
            overall_recall,
        );
        return Ok(());
    }

    // ── Default: pipeline / conversation eval ────────────────────────────────
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY must be set");

    let fixtures_dir = fixtures_dir_from_args(&args);

    println!("PandaFilter Evaluation Report");
    println!("=====================");
    println!("Fixtures dir: {}", fixtures_dir.display());
    println!();

    // ── Command output fixtures (.txt + .qa.toml) ─────────────────────────────
    let fixture_pairs = runner::discover_fixtures(&fixtures_dir)?;
    let mut pipeline_results = Vec::new();

    if !fixture_pairs.is_empty() {
        println!("── Command Output Fixtures ──────────────────────────────────────────────");
        println!();
        for (txt_path, qa_path) in &fixture_pairs {
            let fixture_name = txt_path.file_stem().unwrap().to_string_lossy().into_owned();
            println!("Running fixture: {}", fixture_name);
            match runner::run_fixture(txt_path, qa_path, &api_key) {
                Ok(result) => {
                    report::print_fixture_result(&result);
                    pipeline_results.push(result);
                }
                Err(e) => println!("  ERROR: {}", e),
            }
            println!();
        }
        report::print_summary(&pipeline_results);
        println!();
    }

    // ── Conversation fixtures (.conv.toml) — V1 vs V2 comparison ─────────────
    let conv_paths = runner::discover_conv_fixtures(&fixtures_dir)?;
    let mut compare_results = Vec::new();

    if !conv_paths.is_empty() {
        println!("── Conversation Compression: V1 (BERT) vs V2 (Ollama + BERT gate) ──────");
        println!();
        for path in &conv_paths {
            let name = path.file_name().unwrap().to_string_lossy().replace(".conv.toml", "");
            println!("Running fixture: {}", name);
            match runner::run_conv_fixture_compare(path, &api_key) {
                Ok(result) => {
                    report::print_conv_compare_result(&result);
                    compare_results.push(result);
                }
                Err(e) => println!("  ERROR: {}", e),
            }
            println!();
        }
        report::print_conv_compare_summary(&compare_results);
    }

    Ok(())
}

/// Resolve the fixtures directory from --fixtures-dir <path> arg, env var, or binary-relative default.
fn fixtures_dir_from_args(args: &[String]) -> std::path::PathBuf {
    // --fixtures-dir <path>
    if let Some(pos) = args.iter().position(|a| a == "--fixtures-dir") {
        if let Some(path) = args.get(pos + 1) {
            return std::path::PathBuf::from(path);
        }
    }
    std::path::PathBuf::from(
        std::env::var("PANDA_FIXTURES_DIR")
            .unwrap_or_else(|_| {
                let exe = std::env::current_exe().unwrap();
                exe.parent().unwrap()
                    .parent().unwrap()
                    .parent().unwrap()
                    .join("panda-eval/fixtures")
                    .to_string_lossy()
                    .into_owned()
            })
    )
}

/// Locate the benchmark directory relative to this binary.
/// In the workspace: `<workspace>/ccr-eval/benchmarks/`
fn bench_dir() -> std::path::PathBuf {
    // Try env override first
    if let Ok(dir) = std::env::var("PANDA_BENCH_DIR") {
        return std::path::PathBuf::from(dir);
    }

    // Walk up from current exe to find workspace root
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut dir = exe.as_path();
    // target/debug/panda-eval → target → workspace
    for _ in 0..4 {
        if let Some(parent) = dir.parent() {
            dir = parent;
            let bench = dir.join("ccr-eval/benchmarks");
            if bench.exists() {
                return bench;
            }
        }
    }

    // Fallback: relative to current directory
    std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("ccr-eval/benchmarks")
}
