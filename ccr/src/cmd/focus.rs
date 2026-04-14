use anyhow::Result;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct FocusArgs {
    pub enable: bool,
    pub disable: bool,
    pub status: bool,
    pub dry_run: bool,
}

/// Context Focusing — manage file-relationship graph and guidance.
pub fn run(args: FocusArgs) -> Result<()> {
    match (args.enable, args.disable, args.status, args.dry_run) {
        (true, false, false, false) => enable_focus(),
        (false, true, false, false) => disable_focus(),
        (false, false, true, false) => show_status(),
        (false, false, false, true) => dry_run_guidance(),
        (false, false, false, false) => {
            // Hook mode — reads JSON from stdin, outputs guidance
            hook_mode()
        }
        _ => Err(anyhow::anyhow!("Invalid focus arguments")),
    }
}

fn enable_focus() -> Result<()> {
    let repo_root = std::env::current_dir()?;
    let repo_hash = compute_repo_hash(&repo_root)?;
    let index_parent = get_index_parent(&repo_hash)?;

    println!("Registering Context Focusing hook...");
    println!("Building index for: {}", repo_root.display());

    // Build the index
    panda_core::focus::run_index(&repo_root, &index_parent)?;

    // Register UserPromptSubmit hook in Claude settings
    register_focus_hook()?;

    println!("✓ Context Focusing enabled. Index built and hook registered.");
    Ok(())
}

fn disable_focus() -> Result<()> {
    // Remove UserPromptSubmit hook from Claude settings
    unregister_focus_hook()?;
    println!("Context Focusing disabled. Index preserved — re-enable with `panda focus --enable`.");
    Ok(())
}

/// Register the UserPromptSubmit hook in ~/.claude/settings.json
fn register_focus_hook() -> Result<()> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    let settings_path = home.join(".claude").join("settings.json");

    // Resolve panda binary path
    let panda_bin = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("panda"));
    let panda_bin_str = panda_bin.to_string_lossy();

    let focus_cmd = format!(
        "PANDA_SESSION_ID=$PPID {} focus",
        panda_bin_str
    );

    // Load or create settings.json
    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content)?
    } else {
        serde_json::json!({})
    };

    // Ensure hooks.UserPromptSubmit exists as an array
    if settings.get("hooks").is_none() {
        settings["hooks"] = serde_json::json!({});
    }
    if settings["hooks"].get("UserPromptSubmit").is_none() {
        settings["hooks"]["UserPromptSubmit"] = serde_json::json!([]);
    }

    let arr = settings["hooks"]["UserPromptSubmit"]
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("UserPromptSubmit is not an array"))?;

    // Check if already registered
    let already = arr.iter().any(|entry| {
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
    });

    if !already {
        arr.push(serde_json::json!({
            "matcher": "",
            "hooks": [{ "type": "command", "command": focus_cmd }]
        }));
    }

    std::fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
    Ok(())
}

/// Remove the UserPromptSubmit focus hook from ~/.claude/settings.json
fn unregister_focus_hook() -> Result<()> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    let settings_path = home.join(".claude").join("settings.json");

    if !settings_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&settings_path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    if let Some(arr) = settings["hooks"]["UserPromptSubmit"].as_array_mut() {
        arr.retain(|entry| {
            !entry["hooks"]
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
        });
    }

    std::fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
    Ok(())
}

fn show_status() -> Result<()> {
    let repo_root = std::env::current_dir()?;
    let repo_hash = compute_repo_hash(&repo_root)?;
    let index_parent = get_index_parent(&repo_hash)?;

    println!("Context Focusing status:");
    println!("  Repository: {}", repo_root.display());

    // Check if index exists
    if let Ok(head) = get_current_head(&repo_root) {
        let head_dir = index_parent.join(&head);
        let db_path = head_dir.join("graph.sqlite");

        if panda_core::focus::graph_is_valid(&db_path) {
            if let Ok(meta) = panda_core::focus::indexer::Meta::read(&head_dir) {
                let file_count = count_indexed_files(&db_path).unwrap_or(0);
                let cochange_count = count_cochange_pairs(&db_path).unwrap_or(0);
                println!("  Status: ✓ Index valid");
                println!("  Indexed files: {}", file_count);
                println!("  Cochange pairs: {}", cochange_count);
                println!("  Last indexed: {}", format_timestamp(meta.indexed_at));
                println!("  HEAD: {} ({})", &head[..8.min(head.len())], meta.head_hash);
            }
        } else {
            println!("  Status: ✗ No valid index (run `panda index` to build)");
        }
    } else {
        println!("  Status: ✗ Not in a git repository");
    }

    Ok(())
}

fn dry_run_guidance() -> Result<()> {
    println!("Context Focusing would inject:");
    println!("  Recommended files: (would compute)");
    println!("  Excluded files: (would compute)");
    Ok(())
}

fn hook_mode() -> Result<()> {
    use std::io::Read;
    use serde_json::json;

    // Read JSON from stdin (UserPromptSubmit hook data)
    let mut stdin_raw = String::new();
    std::io::stdin().read_to_string(&mut stdin_raw)?;

    if stdin_raw.is_empty() {
        // Pass-through: no input
        println!("{}", json!({"guidance": null}));
        return Ok(());
    }

    // Parse input JSON
    let input: serde_json::Value = match serde_json::from_str(&stdin_raw) {
        Ok(v) => v,
        Err(_) => {
            // Invalid JSON: pass-through
            println!("{}", json!({"guidance": null}));
            return Ok(());
        }
    };

    // Extract prompt text
    let prompt_text = input
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if prompt_text.is_empty() {
        println!("{}", json!({"guidance": null}));
        return Ok(());
    }

    // Get current repo root and index path
    let repo_root = std::env::current_dir()?;
    let repo_hash = compute_repo_hash(&repo_root)?;
    let index_parent = get_index_parent(&repo_hash)?;

    // Get current HEAD
    let head = match get_current_head(&repo_root) {
        Ok(h) => h,
        Err(_) => {
            // Not in a git repo: pass-through
            println!("{}", json!({"guidance": null}));
            return Ok(());
        }
    };

    let head_dir = index_parent.join(&head);
    let db_path = head_dir.join("graph.sqlite");

    // Check if index exists and is valid
    if !panda_core::focus::graph_is_valid(&db_path) {
        // Try to spawn background index build if not already in progress
        attempt_spawn_index_build(&repo_root, &index_parent)?;
        println!("{}", json!({"guidance": null}));
        return Ok(());
    }

    // Embed the prompt
    let embeddings = match panda_core::summarizer::embed_batch(&[prompt_text]) {
        Ok(embs) => embs,
        Err(_) => {
            println!("{}", json!({"guidance": null}));
            return Ok(());
        }
    };

    let prompt_embedding = match embeddings.first() {
        Some(emb) => emb.clone(),
        None => {
            println!("{}", json!({"guidance": null}));
            return Ok(());
        }
    };

    // Session continuity: skip guidance if prompt is similar to recent prompts
    let sid = crate::session::session_id();
    let mut session = crate::session::SessionState::load(&sid);

    // Check similarity to previous prompt topics
    if let Some(prev_prompt_centroid) = session.command_centroid("(focus_prompt)") {
        let similarity = crate::handlers::util::cosine_similarity(&prompt_embedding, prev_prompt_centroid);
        // If similarity > 0.85, this is a follow-up prompt on the same topic: skip guidance
        if similarity > 0.85 {
            println!("{}", json!({"guidance": null}));
            return Ok(());
        }
    }

    // Query the focus graph
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(_) => {
            println!("{}", json!({"guidance": null}));
            return Ok(());
        }
    };

    // Count total files in the repo
    let total_files: usize = match conn.query_row(
        "SELECT COUNT(*) FROM files",
        [],
        |row| row.get(0),
    ) {
        Ok(count) => count,
        Err(_) => 0,
    };

    // Check if repo is too small (auto-skip)
    if total_files < 25 {
        println!("{}", json!({"guidance": null}));
        return Ok(());
    }

    // Query for relevant files
    let ranked = match panda_core::focus::query(&conn, &prompt_embedding, 6) {
        Ok(files) => files,
        Err(_) => {
            println!("{}", json!({"guidance": null}));
            return Ok(());
        }
    };

    // Capture the count before moving ranked into assemble
    let recommended_count = ranked.len();

    // Assemble guidance output
    let guidance = panda_core::focus::assemble(ranked, total_files);

    // Update session with new prompt embedding (rolling mean centroid)
    session.update_command_centroid("(focus_prompt)", prompt_embedding);
    session.save(&sid);

    // Record guidance event for analytics
    let guidance_tokens = panda_core::tokens::count_tokens(&guidance.guidance_text);
    let excluded_tokens_est = total_files.saturating_sub(recommended_count) * 50; // rough estimate
    let project_path = crate::util::project_key().unwrap_or_default();
    let _ = crate::analytics_db::record_guidance(
        &sid,
        recommended_count,
        total_files,
        Some(excluded_tokens_est),
        Some(guidance_tokens),
        &project_path,
    );

    // Output guidance as JSON
    let output = json!({
        "guidance": {
            "recommended_files": guidance.recommended_files,
            "negative_guidance": guidance.negative_guidance,
            "guidance_text": guidance.guidance_text,
        }
    });

    println!("{}", output);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compute_repo_hash(repo_root: &std::path::Path) -> Result<String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let path_str = repo_root.to_string_lossy();
    let mut hasher = DefaultHasher::new();
    path_str.hash(&mut hasher);
    let hash = hasher.finish();
    Ok(format!("{:x}", hash))
}

fn get_index_parent(repo_hash: &str) -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    let parent = home.join(".local/share/panda/indexes").join(repo_hash);
    Ok(parent)
}

fn get_current_head(repo_root: &std::path::Path) -> Result<String> {
    panda_core::focus::indexer::current_head(repo_root)
}

fn count_indexed_files(db_path: &std::path::Path) -> Result<usize> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path)?;
    let count: usize = conn.query_row(
        "SELECT COUNT(*) FROM files",
        [],
        |row| row.get(0),
    )?;
    Ok(count)
}

fn count_cochange_pairs(db_path: &std::path::Path) -> Result<usize> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path)?;
    let count: usize = conn.query_row(
        "SELECT COUNT(*) FROM cochanges",
        [],
        |row| row.get(0),
    )?;
    Ok(count)
}

fn format_timestamp(secs: u64) -> String {
    use std::time::{UNIX_EPOCH, Duration, SystemTime};
    let duration = Duration::from_secs(secs);
    let datetime = UNIX_EPOCH + duration;
    // Simple format: just show if it's recent
    if let Ok(elapsed) = SystemTime::now().duration_since(datetime) {
        if elapsed.as_secs() < 60 {
            "just now".to_string()
        } else if elapsed.as_secs() < 3600 {
            format!("{}m ago", elapsed.as_secs() / 60)
        } else if elapsed.as_secs() < 86400 {
            format!("{}h ago", elapsed.as_secs() / 3600)
        } else {
            format!("{}d ago", elapsed.as_secs() / 86400)
        }
    } else {
        "unknown".to_string()
    }
}

/// Attempt to spawn a background index build if not already in progress.
///
/// Uses atomic file locking (O_EXCL) to ensure only one build runs at a time.
/// Returns Ok(()) regardless of success — failures are silent (pass-through behavior).
fn attempt_spawn_index_build(repo_root: &std::path::Path, index_parent: &std::path::PathBuf) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    // Create parent directory if needed
    let _ = std::fs::create_dir_all(index_parent);

    // Try to acquire an exclusive lock file
    let lock_path = index_parent.join("build.lock");
    let lock_acquired = OpenOptions::new()
        .write(true)
        .create_new(true)  // O_EXCL: fail if file exists
        .open(&lock_path)
        .is_ok();

    if !lock_acquired {
        // Another build is already in progress or recently completed
        return Ok(());
    }

    // Lock acquired: spawn background `panda index` process
    // Use spawn (detached) so it continues even if parent exits
    let repo_path = repo_root.to_string_lossy().to_string();
    let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("panda"));

    #[cfg(unix)]
    {
        use std::process::Command;
        // Unix: use nohup or redirect to /dev/null to detach
        let _ = Command::new("nohup")
            .arg(&current_exe)
            .arg("index")
            .arg("--repo")
            .arg(&repo_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    #[cfg(windows)]
    {
        use std::process::Command;
        use std::os::windows::process::CommandExt;
        // Windows: use detached process (CREATE_NO_WINDOW)
        let _ = Command::new(&current_exe)
            .arg("index")
            .arg("--repo")
            .arg(&repo_path)
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn();
    }

    // Write a timestamp to the lock file so we know when the build started
    if let Ok(mut f) = OpenOptions::new().write(true).open(&lock_path) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "{}", now);
    }

    Ok(())
}
