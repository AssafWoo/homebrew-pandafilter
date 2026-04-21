//! Benchmark report formatting — tabular output + JSON export.

use std::collections::HashMap;
use std::path::Path;
use crate::bench::TaskResult;

#[derive(serde::Serialize)]
struct RepoSummary {
    repo: String,
    tasks: usize,
    v1_hit_at5: f64,
    v2_hit_at5: f64,
    sigmap_hit_at5: f64,
    v1_mrr: f64,
    v2_mrr: f64,
    sigmap_mrr: f64,
}

#[derive(serde::Serialize)]
struct FullReport {
    generated: String,
    total_tasks: usize,
    v1_hit_at5: f64,
    v2_hit_at5: f64,
    sigmap_hit_at5: f64,
    v1_mrr: f64,
    v2_mrr: f64,
    sigmap_mrr: f64,
    repos: Vec<RepoSummary>,
    regressions: Vec<RegressionDetail>,
}

#[derive(serde::Serialize)]
struct RegressionDetail {
    id: String,
    query: String,
    expected: Vec<String>,
    v1_top5: Vec<String>,
    v2_top5: Vec<String>,
    sigmap_top1: String,
}

/// Load SigMap reference scores from the bundled JSON file.
fn load_sigmap_reference(bench_dir: &Path) -> HashMap<String, (f64, f64)> {
    let path = bench_dir.join("reports/sigmap-reference.json");
    let mut map: HashMap<String, (f64, f64)> = HashMap::new();

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return map,
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return map,
    };

    if let Some(repos) = json["repos"].as_array() {
        for r in repos {
            if let (Some(repo), Some(hit5), Some(mrr)) = (
                r["repo"].as_str(),
                r["hitAt5"].as_f64(),
                r["mrr"].as_f64(),
            ) {
                map.insert(repo.to_string(), (hit5, mrr));
            }
        }
    }

    map
}

/// Print tabular report to stdout and write full JSON report.
pub fn print_and_save(results: &[TaskResult], bench_dir: &Path) {
    let sigmap = load_sigmap_reference(bench_dir);

    // Group by repo
    let mut by_repo: HashMap<&str, Vec<&TaskResult>> = HashMap::new();
    for r in results {
        by_repo.entry(r.repo.as_str()).or_default().push(r);
    }

    // Compute per-repo summaries
    let mut repo_summaries: Vec<RepoSummary> = Vec::new();
    let mut regressions: Vec<RegressionDetail> = Vec::new();

    let mut all_v1_hit5  = 0usize;
    let mut all_v2_hit5  = 0usize;
    let mut all_v1_rr    = 0.0_f64;
    let mut all_v2_rr    = 0.0_f64;
    let total = results.len();

    // Sort repos alphabetically for deterministic output
    let mut sorted_repos: Vec<(&str, Vec<&TaskResult>)> = by_repo.into_iter().collect();
    sorted_repos.sort_by_key(|(name, _)| *name);

    for (repo, tasks) in &sorted_repos {
        let n = tasks.len() as f64;
        let v1_hits: usize = tasks.iter().filter(|t| t.v1_hit_at_5()).count();
        let v2_hits: usize = tasks.iter().filter(|t| t.v2_hit_at_5()).count();
        let v1_rr_sum: f64 = tasks.iter().map(|t| t.v1_rr()).sum();
        let v2_rr_sum: f64 = tasks.iter().map(|t| t.v2_rr()).sum();

        all_v1_hit5 += v1_hits;
        all_v2_hit5 += v2_hits;
        all_v1_rr   += v1_rr_sum;
        all_v2_rr   += v2_rr_sum;

        let (sig_hit5, sig_mrr) = sigmap.get(*repo).copied().unwrap_or((0.0, 0.0));

        repo_summaries.push(RepoSummary {
            repo: repo.to_string(),
            tasks: tasks.len(),
            v1_hit_at5: v1_hits as f64 / n,
            v2_hit_at5: v2_hits as f64 / n,
            sigmap_hit_at5: sig_hit5,
            v1_mrr: v1_rr_sum / n,
            v2_mrr: v2_rr_sum / n,
            sigmap_mrr: sig_mrr,
        });

        // Collect regressions: tasks where v2 missed but sigmap hit@5 = correct
        for t in tasks {
            if !t.v2_hit_at_5() {
                regressions.push(RegressionDetail {
                    id: t.id.clone(),
                    query: t.query.clone(),
                    expected: t.expected_files.clone(),
                    v1_top5: t.v1_top5.clone(),
                    v2_top5: t.v2_top5.clone(),
                    sigmap_top1: String::new(), // not available from task files
                });
            }
        }
    }

    let v1_hit5_overall   = all_v1_hit5 as f64 / total as f64;
    let v2_hit5_overall   = all_v2_hit5 as f64 / total as f64;
    let v1_mrr_overall    = all_v1_rr   / total as f64;
    let v2_mrr_overall    = all_v2_rr   / total as f64;
    let (sig_hit5, sig_mrr) = {
        let values: Vec<(f64, f64)> = sigmap.values().copied().collect();
        let n = values.len() as f64;
        if n == 0.0 {
            (0.0, 0.0)
        } else {
            (
                values.iter().map(|(h, _)| h).sum::<f64>() / n,
                values.iter().map(|(_, m)| m).sum::<f64>() / n,
            )
        }
    };

    // ── Print summary table ───────────────────────────────────────────────
    println!();
    println!("┌──────────────────────────────────────────────────────────────────────────┐");
    println!("│   PandaFilter File Retrieval Benchmark  ({} tasks, {} repos)          │", total, sorted_repos.len());
    println!("├────────────────────────┬──────────┬──────────┬────────────────────────────┤");
    println!("│ Condition              │  Hit@5   │   MRR    │  vs SigMap                 │");
    println!("├────────────────────────┼──────────┼──────────┼────────────────────────────┤");

    let v1_vs = diff_str(v1_hit5_overall - sig_hit5);
    let v2_vs = diff_str(v2_hit5_overall - sig_hit5);

    println!("│ panda-v1 (bert)        │ {:>7.1}% │  {:.3}  │ {} │",
        v1_hit5_overall * 100.0, v1_mrr_overall, pad_right(&v1_vs, 26));
    println!("│ panda-v2 (hybrid)      │ {:>7.1}% │  {:.3}  │ {} │",
        v2_hit5_overall * 100.0, v2_mrr_overall, pad_right(&v2_vs, 26));
    println!("│ sigmap-ref             │ {:>7.1}% │  {:.3}  │ baseline                   │",
        sig_hit5 * 100.0, sig_mrr);

    println!("└────────────────────────┴──────────┴──────────┴────────────────────────────┘");
    println!();

    // ── Per-repo breakdown ────────────────────────────────────────────────
    println!("Per-repo breakdown:");
    println!("  {:20}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}",
        "repo", "v1-h@5", "v2-h@5", "sig-h@5", "v1-mrr", "v2-mrr", "sig-mrr");
    println!("  {}  {}  {}  {}  {}  {}  {}",
        "─".repeat(20), "─".repeat(8), "─".repeat(8), "─".repeat(8),
        "─".repeat(8), "─".repeat(8), "─".repeat(8));

    for r in &repo_summaries {
        println!("  {:20}  {:>7.0}%  {:>7.0}%  {:>7.0}%  {:>8.3}  {:>8.3}  {:>8.3}",
            r.repo,
            r.v1_hit_at5 * 100.0, r.v2_hit_at5 * 100.0, r.sigmap_hit_at5 * 100.0,
            r.v1_mrr, r.v2_mrr, r.sigmap_mrr);
    }

    // ── Worst regressions (v2 misses) ─────────────────────────────────────
    if !regressions.is_empty() {
        let show = regressions.len().min(5);
        println!();
        println!("Worst regressions — v2 misses (first {}/{}):", show, regressions.len());
        for reg in regressions.iter().take(show) {
            println!("  {} — expected {:?}", reg.id, reg.expected);
            println!("    v2 top5: {:?}", &reg.v2_top5);
        }
    }

    // ── Save JSON report ──────────────────────────────────────────────────
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let report = FullReport {
        generated: ts.to_string(),
        total_tasks: total,
        v1_hit_at5:    v1_hit5_overall,
        v2_hit_at5:    v2_hit5_overall,
        sigmap_hit_at5: sig_hit5,
        v1_mrr:    v1_mrr_overall,
        v2_mrr:    v2_mrr_overall,
        sigmap_mrr: sig_mrr,
        repos: repo_summaries,
        regressions,
    };

    let reports_dir = bench_dir.join("reports");
    let _ = std::fs::create_dir_all(&reports_dir);
    let report_path = reports_dir.join(format!("panda-v2-{}.json", ts));

    match serde_json::to_string_pretty(&report) {
        Ok(json) => {
            let _ = std::fs::write(&report_path, &json);
            println!();
            println!("Full report: {}", report_path.display());
        }
        Err(e) => eprintln!("warning: could not serialize report: {}", e),
    }
}

fn diff_str(delta: f64) -> String {
    let pp = delta * 100.0;
    if pp >= 0.05 {
        format!("+{:.1}pp", pp)
    } else if pp <= -0.05 {
        format!("{:.1}pp", pp)
    } else {
        "≈0pp".to_string()
    }
}

fn pad_right(s: &str, width: usize) -> String {
    let len = s.len();
    if len >= width {
        s[..width].to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - len))
    }
}
