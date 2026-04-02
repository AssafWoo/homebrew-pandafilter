//! Gemini CLI agent installer.
//!
//! Gemini CLI reads hooks from `~/.gemini/hooks.json`.
//!
//! Input format:
//!   `{"tool_name": "run_shell_command", "tool_input": {"command": "..."}}`
//!
//! Output (rewrite):
//!   `{"decision": "allow", "hookSpecificOutput": {"tool_input": {"command": "rewritten"}}}`
//!
//! Output (no-op):
//!   `{"decision": "allow"}`
//!
//! Exit 0 on ALL error paths — Gemini CLI does not tolerate non-zero exit from hooks.

use super::AgentInstaller;
use std::path::PathBuf;

pub struct GeminiInstaller;

fn gemini_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".gemini"))
}

fn hooks_json_path() -> Option<PathBuf> {
    Some(gemini_dir()?.join("hooks.json"))
}

impl AgentInstaller for GeminiInstaller {
    fn name(&self) -> &'static str {
        "Gemini CLI"
    }

    fn install(&self, ccr_bin: &str) -> anyhow::Result<()> {
        let Some(gemini_dir) = gemini_dir() else {
            anyhow::bail!("Cannot determine Gemini config directory");
        };
        std::fs::create_dir_all(&gemini_dir)?;

        // Write hook script
        let script_path = gemini_dir.join("ccr-rewrite.sh");
        let script = generate_gemini_script(ccr_bin);
        std::fs::write(&script_path, &script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script_path, perms)?;
        }

        // Update ~/.gemini/hooks.json
        let Some(hooks_path) = hooks_json_path() else {
            anyhow::bail!("Cannot determine Gemini hooks path");
        };

        let mut root: serde_json::Value = if hooks_path.exists() {
            let content = std::fs::read_to_string(&hooks_path)?;
            serde_json::from_str(&content).unwrap_or(serde_json::json!({"version": 1}))
        } else {
            serde_json::json!({"version": 1})
        };

        // Strip existing CCR entries
        for event in &["preToolUse", "postToolUse"] {
            if let Some(arr) = root
                .get_mut("hooks")
                .and_then(|h| h.get_mut(*event))
                .and_then(|e| e.as_array_mut())
            {
                arr.retain(|e| !e["command"].as_str().unwrap_or("").contains("ccr"));
            }
        }

        // PreToolUse rewrite hook
        insert_hook_entry(
            &mut root,
            "preToolUse",
            serde_json::json!({
                "command": script_path.to_string_lossy(),
                "matcher": "run_shell_command"
            }),
        );

        // PostToolUse compressor
        let hook_cmd = format!(
            "CCR_SESSION_ID=$PPID CCR_AGENT=gemini {} hook",
            ccr_bin
        );
        insert_hook_entry(
            &mut root,
            "postToolUse",
            serde_json::json!({"command": hook_cmd, "matcher": "run_shell_command"}),
        );

        std::fs::write(&hooks_path, serde_json::to_string_pretty(&root)?)?;

        println!("CCR hooks installed (Gemini CLI):");
        println!("  Script:  {}", script_path.display());
        println!("  Config:  {}", hooks_path.display());

        Ok(())
    }

    fn uninstall(&self) -> anyhow::Result<()> {
        let Some(gemini_dir) = gemini_dir() else {
            return Ok(());
        };

        let script_path = gemini_dir.join("ccr-rewrite.sh");
        if script_path.exists() {
            std::fs::remove_file(&script_path)?;
            println!("Removed {}", script_path.display());
        }

        let Some(hooks_path) = hooks_json_path() else {
            return Ok(());
        };
        if hooks_path.exists() {
            let content = std::fs::read_to_string(&hooks_path)?;
            let mut root: serde_json::Value =
                serde_json::from_str(&content).unwrap_or(serde_json::json!({}));
            for event in &["preToolUse", "postToolUse"] {
                if let Some(arr) = root
                    .get_mut("hooks")
                    .and_then(|h| h.get_mut(*event))
                    .and_then(|e| e.as_array_mut())
                {
                    arr.retain(|e| !e["command"].as_str().unwrap_or("").contains("ccr"));
                }
            }
            std::fs::write(&hooks_path, serde_json::to_string_pretty(&root)?)?;
            println!("Removed CCR entries from {}", hooks_path.display());
        }

        Ok(())
    }
}

/// Generate the Gemini CLI PreToolUse hook script.
/// Always exits 0 — Gemini CLI terminates on non-zero hook exit.
fn generate_gemini_script(ccr_bin: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
# CCR Gemini CLI PreToolUse hook
# Rewrites run_shell_command tool invocations for token savings.
# ALWAYS exits 0 — Gemini CLI terminates on non-zero hook exit.
INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')
if [ -z "$CMD" ]; then
  echo '{{"decision": "allow"}}'
  exit 0
fi
REWRITTEN=$(CCR_SESSION_ID=$PPID "{ccr_bin}" rewrite "$CMD" 2>/dev/null) || {{
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
        ccr_bin = ccr_bin
    )
}

fn insert_hook_entry(root: &mut serde_json::Value, event: &str, entry: serde_json::Value) {
    let root_obj = match root.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    root_obj.entry("version").or_insert(serde_json::json!(1));
    let hooks = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .expect("hooks must be an object");
    let arr = hooks
        .entry(event)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .expect("event must be an array");
    let cmd = entry["command"].as_str().unwrap_or("").to_string();
    let already = arr
        .iter()
        .any(|e| e["command"].as_str().unwrap_or("") == cmd);
    if !already {
        arr.push(entry);
    }
}
