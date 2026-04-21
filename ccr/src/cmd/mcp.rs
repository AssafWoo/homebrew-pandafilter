//! `panda mcp` — expose panda's file index as MCP tools.
//!
//! Implements a minimal MCP 2024-11 server over stdio JSON-RPC 2.0.
//! No extra dependencies: uses serde_json (already in workspace) + the
//! existing panda-core focus primitives.
//!
//! Four tools:
//!   - `query_files(query, top_k?)` — BERT + lexical hybrid ranking
//!   - `file_signatures(path)`      — function/struct/type names from DB
//!   - `file_impact(path, depth?)`  — co-change neighbourhood (BFS)
//!   - `index_status()`             — meta + file count

use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::collections::{HashMap, VecDeque};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    // Locate the index once, lazily on first tool call.
    // If we can't find it, tool calls return an error result (not an RPC error).
    let index: Option<DbIndex> = DbIndex::find_for_cwd().ok();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = error_response(Value::Null, -32700, &format!("Parse error: {e}"), None);
                let _ = writeln!(stdout, "{}", err);
                let _ = stdout.flush();
                continue;
            }
        };

        let method = request
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("");

        // JSON-RPC notifications have no "id" field at all — no response.
        if !request.as_object().map_or(false, |o| o.contains_key("id")) {
            continue;
        }

        let id = request.get("id").cloned().unwrap_or(Value::Null);

        let response = handle_request(method, &request, &index);
        let _ = writeln!(stdout, "{}", response);
        let _ = stdout.flush();
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Request dispatch
// ---------------------------------------------------------------------------

fn handle_request(method: &str, req: &Value, index: &Option<DbIndex>) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => handle_initialize(id, &params),
        "tools/list" => handle_tools_list(id),
        "tools/call" => handle_tools_call(id, &params, index),
        _ => error_response(id, -32601, &format!("Method not found: {method}"), None),
    }
}

// ---------------------------------------------------------------------------
// MCP: initialize
// ---------------------------------------------------------------------------

fn handle_initialize(id: Value, _params: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "panda",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    })
}

// ---------------------------------------------------------------------------
// MCP: tools/list
// ---------------------------------------------------------------------------

fn handle_tools_list(id: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "tools": [
                {
                    "name": "query_files",
                    "description": "Rank files in this repo by relevance to a query. Uses BERT semantic similarity + lexical matching on code signatures + co-change history. Returns up to top_k file paths with relevance scores.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Natural language or code description of what you're looking for"
                            },
                            "top_k": {
                                "type": "integer",
                                "description": "Maximum number of results to return (default: 10)",
                                "default": 10
                            }
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "file_signatures",
                    "description": "Return the structural signatures (function/struct/type names with collapsed bodies) stored in the index for a file. Useful for understanding a file's API surface without reading its full content.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Repo-relative file path (e.g. src/handlers/cargo.rs)"
                            }
                        },
                        "required": ["path"]
                    }
                },
                {
                    "name": "file_impact",
                    "description": "Return files that frequently change together with the given file (co-change neighbours). Useful for understanding what else might need to change when editing a file.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Repo-relative file path"
                            },
                            "depth": {
                                "type": "integer",
                                "description": "BFS depth for co-change graph traversal (default: 1, max: 2)",
                                "default": 1
                            }
                        },
                        "required": ["path"]
                    }
                },
                {
                    "name": "index_status",
                    "description": "Return the status of the panda file index for this repo: number of indexed files, last index time, schema version, and whether the index is current.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                }
            ]
        }
    })
}

// ---------------------------------------------------------------------------
// MCP: tools/call
// ---------------------------------------------------------------------------

fn handle_tools_call(id: Value, params: &Value, index: &Option<DbIndex>) -> Value {
    let tool_name = params
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("");

    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    let content = match tool_name {
        "query_files"      => tool_query_files(&args, index),
        "file_signatures"  => tool_file_signatures(&args, index),
        "file_impact"      => tool_file_impact(&args, index),
        "index_status"     => tool_index_status(index),
        other => Err(anyhow::anyhow!("Unknown tool: {other}")),
    };

    match content {
        Ok(text) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{ "type": "text", "text": text }],
                "isError": false
            }
        }),
        Err(e) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{ "type": "text", "text": format!("Error: {e}") }],
                "isError": true
            }
        }),
    }
}

// ---------------------------------------------------------------------------
// Tool: query_files
// ---------------------------------------------------------------------------

fn tool_query_files(args: &Value, index: &Option<DbIndex>) -> Result<String> {
    let index = index.as_ref().ok_or_else(|| anyhow::anyhow!(
        "No index found for this repo. Run `panda index` first."
    ))?;

    let query = args.get("query")
        .and_then(|q| q.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: query"))?;

    let top_k = args.get("top_k")
        .and_then(|k| k.as_u64())
        .unwrap_or(10) as usize;
    let top_k = top_k.max(1).min(50);

    // Embed the query text with BERT
    let embeddings = panda_core::summarizer::embed_batch(&[query])
        .map_err(|e| anyhow::anyhow!("Embedding failed: {e}"))?;
    let embedding = embeddings.into_iter().next()
        .ok_or_else(|| anyhow::anyhow!("Embedding returned empty result"))?;

    let conn = rusqlite::Connection::open(&index.db_path)
        .map_err(|e| anyhow::anyhow!("Cannot open index: {e}"))?;

    let results = panda_core::focus::query_hybrid(&conn, query, &embedding, top_k)
        .map_err(|e| anyhow::anyhow!("Query failed: {e}"))?;

    if results.is_empty() {
        return Ok("No relevant files found. The index may be empty — run `panda index`.".to_string());
    }

    let mut out = format!("Top {} files for query: \"{}\"\n\n", results.len(), query);
    for (i, f) in results.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} (score: {:.3}, role: {}, cochanges: {})\n",
            i + 1, f.path, f.relevance_score, f.role, f.cochange_count
        ));
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Tool: file_signatures
// ---------------------------------------------------------------------------

fn tool_file_signatures(args: &Value, index: &Option<DbIndex>) -> Result<String> {
    let index = index.as_ref().ok_or_else(|| anyhow::anyhow!(
        "No index found for this repo. Run `panda index` first."
    ))?;

    let path = args.get("path")
        .and_then(|p| p.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: path"))?;

    let conn = rusqlite::Connection::open(&index.db_path)
        .map_err(|e| anyhow::anyhow!("Cannot open index: {e}"))?;

    let result: Result<(String, String), _> = conn.query_row(
        "SELECT COALESCE(signatures, ''), role FROM files WHERE path = ?1",
        [path],
        |row| Ok((row.get(0)?, row.get(1)?)),
    );

    match result {
        Ok((sigs, role)) => {
            if sigs.trim().is_empty() {
                Ok(format!(
                    "No signatures stored for: {}\n(role: {})\n\nThis file may be a data file, config, or was indexed before signature extraction was added. Re-run `panda index` to rebuild.",
                    path, role
                ))
            } else {
                Ok(format!("Signatures for {} (role: {}):\n\n{}", path, role, sigs))
            }
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            Err(anyhow::anyhow!("File not in index: {path}. Run `panda index` to rebuild."))
        }
        Err(e) => Err(anyhow::anyhow!("DB error: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Tool: file_impact
// ---------------------------------------------------------------------------

fn tool_file_impact(args: &Value, index: &Option<DbIndex>) -> Result<String> {
    let index = index.as_ref().ok_or_else(|| anyhow::anyhow!(
        "No index found for this repo. Run `panda index` first."
    ))?;

    let path = args.get("path")
        .and_then(|p| p.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: path"))?;

    let depth = args.get("depth")
        .and_then(|d| d.as_u64())
        .unwrap_or(1) as usize;
    let depth = depth.max(1).min(2);

    let conn = rusqlite::Connection::open(&index.db_path)
        .map_err(|e| anyhow::anyhow!("Cannot open index: {e}"))?;

    // BFS over co-change graph
    let mut visited: HashMap<String, u64> = HashMap::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    queue.push_back((path.to_string(), 0));
    visited.insert(path.to_string(), u64::MAX);

    let mut neighbours: Vec<(String, u64, usize)> = Vec::new(); // (path, count, depth)

    while let Some((current, current_depth)) = queue.pop_front() {
        if current_depth >= depth {
            continue;
        }

        let mut stmt = conn.prepare(
            "SELECT file_b as peer, change_count FROM cochanges WHERE file_a = ?1
             UNION ALL
             SELECT file_a as peer, change_count FROM cochanges WHERE file_b = ?1
             ORDER BY change_count DESC
             LIMIT 20"
        ).map_err(|e| anyhow::anyhow!("DB error: {e}"))?;

        let rows = stmt.query_map([&current], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        }).map_err(|e| anyhow::anyhow!("DB error: {e}"))?;

        for row in rows {
            let (peer, count) = row.map_err(|e| anyhow::anyhow!("DB row error: {e}"))?;
            if !visited.contains_key(&peer) {
                visited.insert(peer.clone(), count);
                neighbours.push((peer.clone(), count, current_depth + 1));
                queue.push_back((peer, current_depth + 1));
            }
        }
    }

    if neighbours.is_empty() {
        return Ok(format!(
            "No co-change neighbours found for: {}\n\nThis file may be new or rarely changed together with other files.",
            path
        ));
    }

    // Sort by change count descending
    neighbours.sort_by(|a, b| b.1.cmp(&a.1));

    let mut out = format!("Co-change neighbours of {} (depth: {}):\n\n", path, depth);
    for (peer, count, d) in &neighbours {
        out.push_str(&format!("  [depth {}] {} ({} joint commits)\n", d, peer, count));
    }
    out.push_str(&format!("\n{} files change together with {}", neighbours.len(), path));

    Ok(out)
}

// ---------------------------------------------------------------------------
// Tool: index_status
// ---------------------------------------------------------------------------

fn tool_index_status(index: &Option<DbIndex>) -> Result<String> {
    match index {
        None => Ok(format!(
            "No index found for this repo.\n\nRun `panda index` to build the file relationship index.\n\
             The index enables context focusing and semantic file search."
        )),
        Some(idx) => {
            let conn = rusqlite::Connection::open(&idx.db_path)
                .map_err(|e| anyhow::anyhow!("Cannot open index: {e}"))?;

            let file_count: usize = conn.query_row(
                "SELECT COUNT(*) FROM files", [], |row| row.get(0)
            ).unwrap_or(0);

            let cochange_count: usize = conn.query_row(
                "SELECT COUNT(*) FROM cochanges", [], |row| row.get(0)
            ).unwrap_or(0);

            let sig_count: usize = conn.query_row(
                "SELECT COUNT(*) FROM files WHERE signatures != '' AND signatures IS NOT NULL",
                [], |row| row.get(0)
            ).unwrap_or(0);

            let meta = panda_core::focus::indexer::Meta::read(&idx.index_dir).ok();

            let mut out = String::from("Panda index status:\n\n");
            out.push_str(&format!("  Repo:         {}\n", idx.repo_root.display()));
            out.push_str(&format!("  Indexed files:    {}\n", file_count));
            out.push_str(&format!("  With signatures:  {} ({:.0}%)\n",
                sig_count,
                if file_count > 0 { sig_count as f64 / file_count as f64 * 100.0 } else { 0.0 }
            ));
            out.push_str(&format!("  Cochange pairs:   {}\n", cochange_count));

            if let Some(meta) = meta {
                out.push_str(&format!("  Schema version:   {}\n", meta.schema_version));
                out.push_str(&format!("  Model:            {}\n", meta.embedding_model));
                out.push_str(&format!("  HEAD:             {}\n", &meta.head_hash[..8.min(meta.head_hash.len())]));
                out.push_str(&format!("  Last indexed:     {}\n", format_timestamp(meta.indexed_at)));
            }

            Ok(out)
        }
    }
}

// ---------------------------------------------------------------------------
// DB index locator
// ---------------------------------------------------------------------------

struct DbIndex {
    db_path:    std::path::PathBuf,
    index_dir:  std::path::PathBuf,
    repo_root:  std::path::PathBuf,
}

impl DbIndex {
    fn find_for_cwd() -> Result<Self> {
        let repo_root = std::env::current_dir()?;
        let repo_hash = compute_repo_hash(&repo_root);
        let index_parent = get_index_parent(&repo_hash)?;
        let head = panda_core::focus::indexer::current_head(&repo_root)?;
        let index_dir = index_parent.join(&head);
        let db_path = index_dir.join("graph.sqlite");

        if !panda_core::focus::graph_is_valid(&db_path) {
            anyhow::bail!("Index not found or schema mismatch. Run `panda index`.");
        }

        Ok(DbIndex { db_path, index_dir, repo_root })
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn compute_repo_hash(repo_root: &std::path::Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let path_str = repo_root.to_string_lossy();
    let mut hasher = DefaultHasher::new();
    path_str.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

fn get_index_parent(repo_hash: &str) -> Result<std::path::PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    Ok(home.join(".local/share/panda/indexes").join(repo_hash))
}

fn format_timestamp(secs: u64) -> String {
    use std::time::{UNIX_EPOCH, Duration, SystemTime};
    let datetime = UNIX_EPOCH + Duration::from_secs(secs);
    if let Ok(elapsed) = SystemTime::now().duration_since(datetime) {
        match elapsed.as_secs() {
            s if s < 60    => "just now".to_string(),
            s if s < 3600  => format!("{}m ago", s / 60),
            s if s < 86400 => format!("{}h ago", s / 3600),
            s              => format!("{}d ago", s / 86400),
        }
    } else {
        "unknown".to_string()
    }
}

fn error_response(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut err = json!({ "code": code, "message": message });
    if let Some(d) = data {
        err["data"] = d;
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": err
    })
}
