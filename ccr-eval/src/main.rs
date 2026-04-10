mod runner;
mod report;

use anyhow::Result;

fn main() -> Result<()> {
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
