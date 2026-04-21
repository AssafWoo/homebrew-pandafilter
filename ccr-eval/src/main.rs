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

    // ── Default: pipeline / conversation eval ────────────────────────────────
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY must be set");

    let fixtures_dir = std::path::PathBuf::from(
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
    );

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
