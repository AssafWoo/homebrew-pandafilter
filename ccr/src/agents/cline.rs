//! Cline agent installer.
//!
//! Cline uses prompt-level rules (`.clinerules`) rather than executable hooks.
//! `panda init --agent cline` writes rules to:
//!   - `.clinerules` in the current project directory (project-scoped)
//!   - `~/.config/cline/rules.md` (global, if --global flag detected)
//!
//! Appends to existing `.clinerules` rather than overwriting them.

use super::AgentInstaller;

pub struct ClineInstaller;

/// The rules PandaFilter adds to `.clinerules`.
const CCR_RULES_MARKER_START: &str = "<!-- panda-rules-start -->";
const CCR_RULES_MARKER_END: &str = "<!-- panda-rules-end -->";

fn panda_rules_block(panda_bin: &str) -> String {
    format!(
        r#"{start}
## PandaFilter Token Optimization Rules

When running shell commands, prefix the following commands with `{bin} run` to enable
automatic output compression and token savings:

- `git` operations: `{bin} run git status`, `{bin} run git diff`, etc.
- `cargo` builds: `{bin} run cargo build`, `{bin} run cargo test`
- `docker` / `kubectl` / `terraform` commands
- Test runners: `{bin} run npm test`, `{bin} run pytest`, `{bin} run go test ./...`
- Linters: `{bin} run eslint .`, `{bin} run mypy .`

When you see output containing `[N matching lines collapsed — panda expand ID]`, you can
retrieve the original lines with `{bin} expand ID`.
{end}
"#,
        bin = panda_bin,
        start = CCR_RULES_MARKER_START,
        end = CCR_RULES_MARKER_END,
    )
}

impl AgentInstaller for ClineInstaller {
    fn name(&self) -> &'static str {
        "Cline"
    }

    fn install(&self, panda_bin: &str) -> anyhow::Result<()> {
        let rules_block = panda_rules_block(panda_bin);

        // Project-local .clinerules
        let local_path = std::path::PathBuf::from(".clinerules");
        write_rules_to(&local_path, &rules_block)?;
        println!("PandaFilter rules written to {}", local_path.display());

        // Global Cline config (~/.config/cline/rules.md)
        if let Some(config_dir) = dirs::config_dir() {
            let cline_dir = config_dir.join("cline");
            let _ = std::fs::create_dir_all(&cline_dir);
            let global_path = cline_dir.join("rules.md");
            write_rules_to(&global_path, &rules_block)?;
            println!("PandaFilter rules written to {}", global_path.display());
        }

        println!();
        println!("Cline will now suggest using `{} run <cmd>` for supported commands.", panda_bin);
        println!("Restart Cline or reload the project for rules to take effect.");

        Ok(())
    }

    fn uninstall(&self) -> anyhow::Result<()> {
        remove_rules_from(&std::path::PathBuf::from(".clinerules"))?;

        if let Some(config_dir) = dirs::config_dir() {
            let global_path = config_dir.join("cline").join("rules.md");
            remove_rules_from(&global_path)?;
        }

        Ok(())
    }
}

/// Append PandaFilter rules to `path`, or replace an existing PandaFilter block.
fn write_rules_to(path: &std::path::Path, rules: &str) -> anyhow::Result<()> {
    let existing = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };

    let new_content = if existing.contains(CCR_RULES_MARKER_START) {
        // Replace existing PandaFilter block
        let start = existing.find(CCR_RULES_MARKER_START).unwrap();
        let end = existing
            .find(CCR_RULES_MARKER_END)
            .map(|p| p + CCR_RULES_MARKER_END.len())
            .unwrap_or(existing.len());
        format!("{}{}{}", &existing[..start], rules, &existing[end..])
    } else if existing.is_empty() {
        rules.to_string()
    } else {
        // Append after existing content
        format!("{}\n\n{}", existing.trim_end(), rules)
    };

    std::fs::write(path, new_content)?;
    Ok(())
}

/// Remove the PandaFilter rules block from `path` (if any).
fn remove_rules_from(path: &std::path::Path) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(path)?;
    if !content.contains(CCR_RULES_MARKER_START) {
        return Ok(());
    }

    let start = content.find(CCR_RULES_MARKER_START).unwrap();
    let end = content
        .find(CCR_RULES_MARKER_END)
        .map(|p| p + CCR_RULES_MARKER_END.len() + 1) // +1 for trailing newline
        .unwrap_or(content.len());

    let new_content = format!("{}{}", &content[..start], &content[end..]);
    if new_content.trim().is_empty() {
        std::fs::remove_file(path)?;
        println!("Removed {}", path.display());
    } else {
        std::fs::write(path, new_content)?;
        println!("Removed PandaFilter rules from {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_rules_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".clinerules");
        let rules = panda_rules_block("panda");
        write_rules_to(&path, &rules).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(CCR_RULES_MARKER_START));
        assert!(content.contains("panda run git status"));
    }

    #[test]
    fn write_rules_appends_to_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".clinerules");
        std::fs::write(&path, "# My existing rules\n").unwrap();
        let rules = panda_rules_block("panda");
        write_rules_to(&path, &rules).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("My existing rules"));
        assert!(content.contains(CCR_RULES_MARKER_START));
    }

    #[test]
    fn write_rules_replaces_existing_block() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".clinerules");
        let initial = format!(
            "# My rules\n\n{}\nold panda content\n{}\n",
            CCR_RULES_MARKER_START, CCR_RULES_MARKER_END
        );
        std::fs::write(&path, initial).unwrap();
        let rules = panda_rules_block("panda");
        write_rules_to(&path, &rules).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("My rules"));
        assert!(!content.contains("old panda content"));
        assert!(content.contains("panda run git status"));
    }

    #[test]
    fn remove_rules_cleans_up() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".clinerules");
        let rules = panda_rules_block("panda");
        write_rules_to(&path, &rules).unwrap();
        remove_rules_from(&path).unwrap();
        // File should be removed (was only PandaFilter content)
        assert!(!path.exists(), "empty .clinerules should be removed");
    }

    #[test]
    fn remove_rules_preserves_other_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".clinerules");
        let content = format!(
            "# Other rules\nDo something.\n\n{}\npandablock\n{}\n",
            CCR_RULES_MARKER_START, CCR_RULES_MARKER_END
        );
        std::fs::write(&path, content).unwrap();
        remove_rules_from(&path).unwrap();
        let remaining = std::fs::read_to_string(&path).unwrap();
        assert!(remaining.contains("Other rules"));
        assert!(!remaining.contains(CCR_RULES_MARKER_START));
    }
}
