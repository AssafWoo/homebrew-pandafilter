//! Persistence layer for Zoom-In blocks.
//!
//! Blocks are stored at: ~/.local/share/panda/expand/{session_id}/ZI_N.txt
//! The expand command searches all session directories for a given ID.

use panda_core::zoom::ZoomBlock;
use std::path::PathBuf;

fn expand_dir() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("panda").join("expand"))
}

fn session_expand_dir(session_id: &str) -> Option<PathBuf> {
    Some(expand_dir()?.join(session_id))
}

/// Persist a batch of zoom blocks for the given session.
pub fn save_blocks(session_id: &str, blocks: Vec<ZoomBlock>) -> anyhow::Result<()> {
    if blocks.is_empty() {
        return Ok(());
    }
    let dir = session_expand_dir(session_id)
        .ok_or_else(|| anyhow::anyhow!("cannot determine data directory"))?;
    std::fs::create_dir_all(&dir)?;
    for block in blocks {
        let path = dir.join(format!("{}.txt", block.id));
        std::fs::write(path, block.lines.join("\n"))?;
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
