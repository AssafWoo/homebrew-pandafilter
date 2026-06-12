//! Persistence layer for Zoom-In blocks.
//!
//! Blocks are stored at: ~/.local/share/panda/expand/{session_id}/ZI_N.txt
//! The expand command searches all session directories for a given ID.
//!
//! Each block may carry a sidecar `ZI_N.meta` file holding the command that
//! produced it. The feedback loop (`feedback.rs`) uses it to attribute
//! `panda expand` events back to the command whose compression hid the
//! content, closing the over-compression signal loop.

use panda_core::zoom::ZoomBlock;
use std::path::PathBuf;

fn expand_dir() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("panda").join("expand"))
}

fn session_expand_dir(session_id: &str) -> Option<PathBuf> {
    Some(expand_dir()?.join(session_id))
}

/// Persist a batch of zoom blocks for the given session.
/// `command` — the command whose compression produced these blocks; recorded
/// as block metadata and counted in the feedback store when present.
pub fn save_blocks(
    session_id: &str,
    blocks: Vec<ZoomBlock>,
    command: Option<&str>,
) -> anyhow::Result<()> {
    if blocks.is_empty() {
        return Ok(());
    }
    let dir = session_expand_dir(session_id)
        .ok_or_else(|| anyhow::anyhow!("cannot determine data directory"))?;
    std::fs::create_dir_all(&dir)?;
    let n_blocks = blocks.len() as u64;
    for block in blocks {
        let path = dir.join(format!("{}.txt", block.id));
        std::fs::write(path, block.lines.join("\n"))?;
        if let Some(cmd) = command {
            let meta_path = dir.join(format!("{}.meta", block.id));
            let _ = std::fs::write(meta_path, cmd);
        }
    }
    if let Some(cmd) = command {
        crate::feedback::record_blocks(cmd, n_blocks);
    }
    Ok(())
}

/// Load a specific zoom block by ID, searching across all sessions.
pub fn load_block(id: &str) -> anyhow::Result<String> {
    let base = expand_dir()
        .ok_or_else(|| anyhow::anyhow!("cannot determine data directory"))?;

    if !base.exists() {
        anyhow::bail!("No expand blocks found. Run a command through panda first.");
    }

    for entry in std::fs::read_dir(&base)? {
        let session_dir = entry?.path();
        if !session_dir.is_dir() {
            continue;
        }
        let file = session_dir.join(format!("{}.txt", id));
        if file.exists() {
            return Ok(std::fs::read_to_string(file)?);
        }
    }

    anyhow::bail!(
        "No block found for '{}'. IDs are session-scoped — run the command again if the session expired.",
        id
    )
}

/// Return the command that produced block `id`, if its metadata exists.
/// Searches all sessions, mirroring `load_block`.
pub fn block_command(id: &str) -> Option<String> {
    let base = expand_dir()?;
    let sessions = std::fs::read_dir(&base).ok()?;
    for entry in sessions.flatten() {
        let session_dir = entry.path();
        if !session_dir.is_dir() {
            continue;
        }
        let meta = session_dir.join(format!("{}.meta", id));
        if meta.exists() {
            return std::fs::read_to_string(meta).ok().map(|s| s.trim().to_string());
        }
    }
    None
}

/// List all block IDs available across all sessions.
pub fn list_blocks() -> Vec<String> {
    let base = match expand_dir() {
        Some(d) => d,
        None => return Vec::new(),
    };
    let mut ids = Vec::new();
    if let Ok(sessions) = std::fs::read_dir(&base) {
        for session in sessions.flatten() {
            if let Ok(files) = std::fs::read_dir(session.path()) {
                for file in files.flatten() {
                    let name = file.file_name().to_string_lossy().to_string();
                    if name.ends_with(".txt") && name.starts_with("ZI_") {
                        ids.push(name.trim_end_matches(".txt").to_string());
                    }
                }
            }
        }
    }
    // Sort numerically by the N in ZI_N
    ids.sort_by(|a, b| {
        let n_a: usize = a.trim_start_matches("ZI_").parse().unwrap_or(0);
        let n_b: usize = b.trim_start_matches("ZI_").parse().unwrap_or(0);
        n_a.cmp(&n_b)
    });
    ids
}
