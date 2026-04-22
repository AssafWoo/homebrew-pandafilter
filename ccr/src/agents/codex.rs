//! OpenAI Codex CLI agent installer.
//!
//! Codex CLI reads hooks from `~/.codex/hooks.json`.
//!
//! Hook registration format (PreToolUse — command rewriting):
//!   ```json
//!   { "hooks": { "PreToolUse": [
//!     { "matcher": "shell",
//!       "hooks": [{ "type": "command", "command": "<script_path>" }] }
//!   ]}}
//!   ```
//!
//! Hook registration format (PostToolUse — output compression):
//!   ```json
//!   { "hooks": { "PostToolUse": [
//!     { "matcher": "shell",
//!       "hooks": [{ "type": "command", "command": "<panda hook cmd>" }] }
//!   ]}}
//!   ```
//!
//! Hook input (PreToolUse):  stdin JSON with `tool_name`, `tool_input.command`
//! Hook output (rewrite): `{"decision": "allow", "hookSpecificOutput": {"tool_input": {"command": "rewritten"}}}`
//! Hook output (no-op):   `{"decision": "allow"}`
//!
//! Exit 0 on ALL error paths — Codex CLI terminates on non-zero hook exit.

use super::AgentInstaller;
use std::path::PathBuf;

pub struct CodexInstaller;

fn codex_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".codex"))
}

fn hooks_json_path() -> Option<PathBuf> {
    Some(codex_dir()?.join("hooks.json"))
}

impl AgentInstaller for CodexInstaller {
    fn name(&self) -> &'static str {
        "OpenAI Codex"
    }

    fn install(&self, panda_bin: &str) -> anyhow::Result<()> {
        let Some(codex_dir) = codex_dir() else {
            anyhow::bail!("Cannot determine Codex config directory");
        };

        // Only install if Codex is already present on this machine.
        // Avoids creating a stray ~/.codex/ on machines that don't use Codex.
        if !codex_dir.exists() {
            println!("Codex not found (no ~/.codex directory) — skipping Codex install.");
            println!("If you install Codex later, run: panda init --agent codex");
            return Ok(());
        }

        std::fs::create_dir_all(&codex_dir)?;

        // Write PreToolUse rewrite script
        let script_path = codex_dir.join("panda-rewrite.sh");
        let script = generate_codex_rewrite_script(panda_bin);
        std::fs::write(&script_path, &script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script_path, perms)?;
        }

        // Write integrity baseline
        if let Err(e) = crate::integrity::write_baseline(&script_path, &codex_dir) {
            eprintln!("warning: could not write integrity baseline: {e}");
        }

        // Update ~/.codex/hooks.json
        let Some(hooks_path) = hooks_json_path() else {
            anyhow::bail!("Cannot determine Codex hooks.json path");
        };

        let mut root: serde_json::Value = if hooks_path.exists() {
            let content = std::fs::read_to_string(&hooks_path)?;
            serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        let script_str = script_path.to_string_lossy().to_string();
        // PANDA_AGENT=codex tells hook.rs to check ~/.codex for integrity
        let hook_cmd = format!("PANDA_SESSION_ID=$PPID PANDA_AGENT=codex {} hook", panda_bin);

        // Remove any existing PandaFilter entries before re-inserting
        remove_panda_entries(&mut root, "PreToolUse");
        remove_panda_entries(&mut root, "PostToolUse");

        // Insert PreToolUse rewrite entry
        insert_hook_entry(&mut root, "PreToolUse", "shell", &script_str);
        // Insert PostToolUse compression entry
        insert_hook_entry(&mut root, "PostToolUse", "shell", &hook_cmd);

        std::fs::write(&hooks_path, serde_json::to_string_pretty(&root)?)?;

        println!("PandaFilter hooks installed (OpenAI Codex CLI + VS Code extension):");
        println!("  Note: Codex CLI and VS Code extension share ~/.codex/hooks.json");
        println!("  Rewrite script: {}", script_path.display());
        println!("  Hooks config:   {}", hooks_path.display());

        Ok(())
    }

    fn uninstall(&self) -> anyhow::Result<()> {
        let Some(codex_dir) = codex_dir() else {
            return Ok(());
        };

        // Remove rewrite script
        let script_path = codex_dir.join("panda-rewrite.sh");
        if script_path.exists() {
            std::fs::remove_file(&script_path)?;
            println!("Removed {}", script_path.display());
        }

        // Remove integrity baseline
        let hash_path = codex_dir.join(".panda-hook.sha256");
        if hash_path.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&hash_path) {
                    let mut perms = meta.permissions();
                    perms.set_mode(0o644);
                    let _ = std::fs::set_permissions(&hash_path, perms);
                }
            }
            std::fs::remove_file(&hash_path)?;
            println!("Removed {}", hash_path.display());
        }

        // Strip PandaFilter entries from hooks.json
        let Some(hooks_path) = hooks_json_path() else {
            return Ok(());
        };
        if hooks_path.exists() {
            let content = std::fs::read_to_string(&hooks_path)?;
            let mut root: serde_json::Value =
                serde_json::from_str(&content).unwrap_or(serde_json::json!({}));

            remove_panda_entries(&mut root, "PreToolUse");
            remove_panda_entries(&mut root, "PostToolUse");

            std::fs::write(&hooks_path, serde_json::to_string_pretty(&root)?)?;
            println!("Removed PandaFilter entries from {}", hooks_path.display());
        }

        Ok(())
    }
}

/// Generate the Codex PreToolUse shell script that rewrites commands.
/// Always exits 0 — Codex CLI terminates on non-zero hook exit.
fn generate_codex_rewrite_script(panda_bin: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
# PandaFilter Codex PreToolUse hook
# Rewrites shell invocations for token savings.
# ALWAYS exits 0 — Codex terminates on non-zero hook exit.
INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')
if [ -z "$CMD" ]; then
  echo '{{"decision": "allow"}}'
  exit 0
fi
REWRITTEN=$(PANDA_SESSION_ID=$PPID "{panda_bin}" rewrite "$CMD" 2>/dev/null) || {{
  echo '{{"decision": "allow"}}'
  exit 0
}}
if [ "$CMD" = "$REWRITTEN" ]; then
  echo '{{"decision": "allow"}}'
  exit 0
fi
jq -n --arg cmd "$REWRITTEN" '{{
  "decision": "allow",
  "hookSpecificOutput": {{
    "tool_input": {{"command": $cmd}}
  }}
}}'
"#,
        panda_bin = panda_bin
    )
}

/// Remove all PandaFilter hook entries from `hooks.<event>` in `root`.
fn remove_panda_entries(root: &mut serde_json::Value, event: &str) {
    if let Some(arr) = root
        .get_mut("hooks")
        .and_then(|h| h.get_mut(event))
        .and_then(|e| e.as_array_mut())
    {
        arr.retain(|entry| {
            // Check nested hooks array
            if let Some(hooks) = entry["hooks"].as_array() {
                let has_panda = hooks.iter().any(|h| {
                    let c = h["command"].as_str().unwrap_or("");
                    c.contains("panda") || c.contains("ccr")
                });
                if has_panda {
                    return false;
                }
            }
            // Check top-level command
            let cmd = entry["command"].as_str().unwrap_or("");
            !cmd.contains("panda") && !cmd.contains("ccr")
        });
    }
}

/// Insert a hook entry under `hooks.<event>` for the given `matcher` and `command`.
/// Skips insertion if the exact command is already present.
fn insert_hook_entry(
    root: &mut serde_json::Value,
    event: &str,
    matcher: &str,
    command: &str,
) {
    let entry = serde_json::json!({
        "matcher": matcher,
        "hooks": [{ "type": "command", "command": command }]
    });

    let root_obj = root.as_object_mut().unwrap();
    let hooks = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .unwrap();
    let event_arr = hooks
        .entry(event)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .unwrap();

    let already = event_arr.iter().any(|e| {
        e["hooks"]
            .as_array()
            .and_then(|h| h.first())
            .and_then(|h| h["command"].as_str())
            .unwrap_or("")
            == command
    });
    if !already {
        event_arr.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_remove_entries() {
        let mut root = serde_json::json!({});
        insert_hook_entry(&mut root, "PreToolUse", "shell", "/usr/local/bin/panda-rewrite.sh");
        insert_hook_entry(&mut root, "PostToolUse", "shell", "PANDA_AGENT=codex /usr/local/bin/panda hook");

        assert!(root["hooks"]["PreToolUse"].as_array().unwrap().len() == 1);
        assert!(root["hooks"]["PostToolUse"].as_array().unwrap().len() == 1);

        // Inserting same command again should be a no-op
        insert_hook_entry(&mut root, "PreToolUse", "shell", "/usr/local/bin/panda-rewrite.sh");
        assert!(root["hooks"]["PreToolUse"].as_array().unwrap().len() == 1);

        remove_panda_entries(&mut root, "PreToolUse");
        remove_panda_entries(&mut root, "PostToolUse");

        assert!(root["hooks"]["PreToolUse"].as_array().unwrap().is_empty());
        assert!(root["hooks"]["PostToolUse"].as_array().unwrap().is_empty());
    }

    #[test]
    fn remove_preserves_non_panda_entries() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {"matcher": "shell", "hooks": [{"type": "command", "command": "/usr/bin/other-hook.sh"}]},
                    {"matcher": "shell", "hooks": [{"type": "command", "command": "/usr/local/bin/panda-rewrite.sh"}]}
                ]
            }
        });

        remove_panda_entries(&mut root, "PreToolUse");
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(arr[0]["hooks"][0]["command"].as_str().unwrap().contains("other-hook"));
    }

    #[test]
    fn script_contains_jq_rewrite() {
        let script = generate_codex_rewrite_script("/usr/local/bin/panda");
        assert!(script.contains("/usr/local/bin/panda"));
        assert!(script.contains("rewrite"));
        assert!(script.contains("hookSpecificOutput"));
        assert!(script.contains("tool_input"));
    }
}
