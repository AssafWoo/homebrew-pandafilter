//! File-retrieval benchmark — panda vs SigMap baseline.
//!
//! Compares two ranking conditions on 90 tasks across 18 repos:
//!   V1: BERT-only (current baseline, first 1000 chars embedded)
//!   V2: Hybrid BERT + lexical on structural signatures
//!
//! Usage:
//!   panda-eval --bench --clone   # clone 18 repos + index (one-time, ~1-2 GB)
//!   panda-eval --bench           # run benchmark, print report

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Repo catalogue — matches SigMap's run-benchmark.mjs
// ---------------------------------------------------------------------------

pub const BENCH_REPOS: &[(&str, &str)] = &[
    ("express",          "https://github.com/expressjs/express.git"),
    ("flask",            "https://github.com/pallets/flask.git"),
    ("gin",              "https://github.com/gin-gonic/gin.git"),
    ("spring-petclinic", "https://github.com/spring-projects/spring-petclinic.git"),
    ("rails",            "https://github.com/rails/rails.git"),
    ("axios",            "https://github.com/axios/axios.git"),
    ("rust-analyzer",    "https://github.com/rust-lang/rust-analyzer.git"),
    ("abseil-cpp",       "https://github.com/abseil/abseil-cpp.git"),
    ("serilog",          "https://github.com/serilog/serilog.git"),
    ("riverpod",         "https://github.com/rrousselGit/riverpod.git"),
    ("okhttp",           "https://github.com/square/okhttp.git"),
    ("laravel",          "https://github.com/laravel/framework.git"),
    ("akka",             "https://github.com/akka/akka.git"),
    ("vapor",            "https://github.com/vapor/vapor.git"),
    ("vue-core",         "https://github.com/vuejs/core.git"),
    ("svelte",           "https://github.com/sveltejs/svelte.git"),
    ("fastify",          "https://github.com/fastify/fastify.git"),
    ("fastapi",          "https://github.com/fastapi/fastapi.git"),
];

// ---------------------------------------------------------------------------
// Task format (matches SigMap JSONL)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct BenchTask {
    pub id: String,
    pub query: String,
    pub expected_files: Vec<String>,
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TaskResult {
    pub id: String,
    pub repo: String,
    pub query: String,
    pub expected_files: Vec<String>,
    /// rank of first hit in top-5 (1-indexed), None if not found
    pub v1_first_hit: Option<usize>,
    pub v2_first_hit: Option<usize>,
    pub v1_top5: Vec<String>,
    pub v2_top5: Vec<String>,
}

impl TaskResult {
    pub fn v1_hit_at_5(&self) -> bool {
        self.v1_first_hit.map_or(false, |r| r <= 5)
    }
    pub fn v2_hit_at_5(&self) -> bool {
        self.v2_first_hit.map_or(false, |r| r <= 5)
    }
    pub fn v1_rr(&self) -> f64 {
        self.v1_first_hit.map_or(0.0, |r| if r <= 5 { 1.0 / r as f64 } else { 0.0 })
    }
    pub fn v2_rr(&self) -> f64 {
        self.v2_first_hit.map_or(0.0, |r| if r <= 5 { 1.0 / r as f64 } else { 0.0 })
    }
}

// ---------------------------------------------------------------------------
// Clone step
// ---------------------------------------------------------------------------

/// Clone all 18 repos (depth=1) and build their panda indexes.
/// Safe to re-run: skips repos that are already present.
pub fn clone_and_index(bench_dir: &Path) -> Result<()> {
    let repos_dir  = bench_dir.join("repos");
    let index_dir  = bench_dir.join("indexes");
    std::fs::create_dir_all(&repos_dir)?;
    std::fs::create_dir_all(&index_dir)?;

    for (name, url) in BENCH_REPOS {
        let repo_path = repos_dir.join(name);

        if !repo_path.exists() {
            println!("  Cloning {} …", name);
            let status = Command::new("git")
                .args(["clone", "--depth=1", "--quiet", url, &repo_path.to_string_lossy()])
                .status()
                .with_context(|| format!("git clone failed for {}", name))?;
            if !status.success() {
                eprintln!("  warning: clone failed for {} — skipping", name);
                continue;
            }
        } else {
            println!("  {} already cloned — skipping", name);
        }

        let repo_index_parent = index_dir.join(name);
        std::fs::create_dir_all(&repo_index_parent)?;

        // Only skip if we have a schema-valid index; stale-schema indexes trigger a rebuild.
        let has_valid_index = {
            let dirs = panda_core::focus::indexer::list_index_dirs(&repo_index_parent);
            dirs.iter().any(|(dir, _)| {
                panda_core::focus::graph_is_valid(&dir.join("graph.sqlite"))
            })
        };

        if has_valid_index {
            println!("  {} already indexed — skipping", name);
        } else {
            println!("  Indexing {} …", name);
            panda_core::focus::run_index(&repo_path, &repo_index_parent)
                .with_context(|| format!("panda index failed for {}", name))?;
            println!("  ✓ {} indexed", name);
        }
    }

    println!();
    println!("Clone + index complete.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Run step
// ---------------------------------------------------------------------------

pub fn run_benchmark(bench_dir: &Path) -> Result<Vec<TaskResult>> {
    let tasks_dir  = bench_dir.join("tasks");
    let index_dir  = bench_dir.join("indexes");

    let mut all_results: Vec<TaskResult> = Vec::new();

    for (repo_name, _) in BENCH_REPOS {
        let task_file = tasks_dir.join(format!("{}.jsonl", repo_name));
        if !task_file.exists() {
            eprintln!("warning: task file missing: {}", task_file.display());
            continue;
        }

        let repo_index_parent = index_dir.join(repo_name);
        let db_path = match find_db(&repo_index_parent) {
            Some(p) => p,
            None => {
                eprintln!("warning: no index for {} — run with --clone first", repo_name);
                continue;
            }
        };

        let conn = rusqlite::Connection::open(&db_path)
            .with_context(|| format!("Cannot open index for {}", repo_name))?;

        let tasks = load_tasks(&task_file)?;
        println!("  {} ({} tasks) …", repo_name, tasks.len());

        for task in &tasks {
            let result = run_task(&conn, task, repo_name)?;
            all_results.push(result);
        }
    }

    Ok(all_results)
}

fn run_task(conn: &rusqlite::Connection, task: &BenchTask, repo: &str) -> Result<TaskResult> {
    // Embed the query
    let embeddings = panda_core::summarizer::embed_batch(&[task.query.as_str()])
        .context("BERT embedding failed")?;
    let embedding = embeddings.into_iter().next().context("empty embedding")?;

    // V1: BERT only
    let v1 = panda_core::focus::query(conn, &embedding, 5)
        .context("V1 query failed")?;
    let v1_paths: Vec<String> = v1.iter().map(|f| f.path.clone()).collect();

    // V2: hybrid BERT + lexical
    let v2 = panda_core::focus::query_hybrid(conn, &task.query, &embedding, 5)
        .context("V2 query failed")?;
    let v2_paths: Vec<String> = v2.iter().map(|f| f.path.clone()).collect();

    let v1_first_hit = first_hit_rank(&v1_paths, &task.expected_files);
    let v2_first_hit = first_hit_rank(&v2_paths, &task.expected_files);

    Ok(TaskResult {
        id:             task.id.clone(),
        repo:           repo.to_string(),
        query:          task.query.clone(),
        expected_files: task.expected_files.clone(),
        v1_first_hit,
        v2_first_hit,
        v1_top5: v1_paths,
        v2_top5: v2_paths,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_tasks(path: &Path) -> Result<Vec<BenchTask>> {
    let content = std::fs::read_to_string(path)?;
    let mut tasks = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let task: BenchTask = serde_json::from_str(line)
            .with_context(|| format!("Failed to parse task: {}", line))?;
        tasks.push(task);
    }
    Ok(tasks)
}

/// Find the best db within a panda index parent dir.
/// Uses a schema-version-agnostic check for benchmark use — the query-time
/// noise filter and weighting adjustments work on any schema version.
fn find_db(index_parent: &Path) -> Option<PathBuf> {
    // First try schema-valid index
    let dirs = panda_core::focus::indexer::list_index_dirs(index_parent);
    for (dir, _) in &dirs {
        let db = dir.join("graph.sqlite");
        if panda_core::focus::graph_is_valid(&db) {
            return Some(db);
        }
    }
    // Fallback: any SQLite file that exists (ignores schema version mismatch)
    for (dir, _) in dirs {
        let db = dir.join("graph.sqlite");
        if db.exists() {
            return Some(db);
        }
    }
    None
}

/// Find the 1-indexed rank of the first result path that basename-matches any expected file.
///
/// SigMap uses basename matching: "src/flask/app.py" matches if "app.py" is in top-5.
/// We do the same to be apples-to-apples.
fn first_hit_rank(result_paths: &[String], expected_files: &[String]) -> Option<usize> {
    let expected_basenames: Vec<&str> = expected_files.iter()
        .map(|e| Path::new(e).file_name().and_then(|n| n.to_str()).unwrap_or(""))
        .collect();

    for (i, path) in result_paths.iter().enumerate() {
        let result_basename = Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or("");
        if expected_basenames.iter().any(|&e| e == result_basename) {
            return Some(i + 1); // 1-indexed
        }
    }
    None
}
