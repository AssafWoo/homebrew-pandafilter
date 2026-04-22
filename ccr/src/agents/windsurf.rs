//! Windsurf (Codeium IDE) agent installer.
//!
//! Windsurf reads hooks from `~/.codeium/windsurf/hooks.json`.
//!
//! Hook registration format (flat array, no matcher):
//!   ```json
//!   {
//!     "hooks": {
//!       "pre_run_command":  [{ "command": "<script_path>" }],
//!       "post_run_command": [{ "command": "PANDA_AGENT=windsurf panda hook" }]
//!     }
//!   }
//!   ```
//!
//! `pre_run_command` hook:
//!   - Receives JSON on stdin with at least `{"command": "..."}`.
//!   - Rewrites the command via `panda rewrite`; outputs nothing (pass-through) when no rewrite.
//!   - Always exits 0 — exit 2 would block the command entirely.
//!
//! `post_run_command` hook:
//!   - Passes the tool output through `panda hook` for compression.
//!   - `PANDA_AGENT=windsurf` tells hook.rs to check `~/.codeium/windsurf/` for integrity.

use super::AgentInstaller;
use std::path::PathBuf;

pub struct WindsurfInstaller;

fn windsurf_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".codeium").join("windsurf"))
}

fn hooks_json_path() -> Option<PathBuf> {
    Some(windsurf_dir()?.join("hooks.json"))
}

impl AgentInstaller for WindsurfInstaller {
    fn name(&self) -> &'static str {
        "Windsurf"
    }

    fn install(&self, panda_bin: &str) -> anyhow::Result<()> {
        let Some(windsurf_dir) = windsurf_dir() else {
            anyhow::bail!("Cannot determine Windsurf config directory");
        };

        // Only install if Windsurf is already present on this machine.
        // Checks for the config dir or the macOS app bundle.
        let windsurf_detected = windsurf_dir.exists()
            || std::path::Path::new("/Applications/Windsurf.app").exists();
        if !windsurf_detected {
            println!("Windsurf not found (no ~/.codeium/windsurf directory) — skipping Windsurf install.");
            println!("If you install Windsurf later, run: panda init --agent windsurf");
            return Ok(());
        }

        std::fs::create_dir_all(&windsurf_dir)?;

        // Write pre_run_command rewrite script
        let script_path = windsurf_dir.join("panda-rewrite.sh");
        let script = generate_windsurf_rewrite_script(panda_bin);
        std::fs::write(&script_path, &script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script_path, perms)?;
        }

        // Write integrity baseline
        if let Err(e) = crate::integrity::write_baseline(&script_path, &windsurf_dir) {
            eprintln!("warning: could not write integrity baseline: {e}");
        }

        // Update ~/.codeium/windsurf/hooks.json
        let Some(hooks_path) = hooks_json_path() else {
            anyhow::bail!("Cannot determine Windsurf hooks.json path");
        };

        let mut root: serde_json::Value = if hooks_path.exists() {
            let content = std::fs::read_to_string(&hooks_path)?;
            serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        let script_str = script_path.to_string_lossy().to_string();
        // PANDA_AGENT=windsurf tells hook.rs to check ~/.codeium/windsurf/ for integrity
        let hook_cmd = format!(
            "PANDA_SESSION_ID=$PPID PANDA_AGENT=windsurf {} hook",
            panda_bin
        );

        // Remove any existing PandaFilter entries before re-inserting
        remove_panda_entries(&mut root, "pre_run_command");
        remove_panda_entries(&mut root, "post_run_command");

        // Insert pre_run_command rewrite entry
        insert_hook_entry(&mut root, "pre_run_command", &script_str);
        // Insert post_run_command compression entry
        insert_hook_entry(&mut root, "post_run_command", &hook_cmd);

        std::fs::write(&hooks_path, serde_json::to_string_pretty(&root)?)?;

        println!("PandaFilter hooks installed (Windsurf):");
        println!("  Rewrite script: {}", script_path.display());
        println!("  Hooks config:   {}", hooks_path.display());

        Ok(())
    }

    fn uninstall(&self) -> anyhow::Result<()> {
        let Some(windsurf_dir) = windsurf_dir() else {
            return Ok(());
        };

        // Remove rewrite script
        let script_path = windsurf_dir.join("panda-rewrite.sh");
        if script_path.exists() {
            std::fs::remove_file(&script_path)?;
            println!("Removed {}", script_path.display());
        }

        // Remove integrity baseline
        let hash_path = windsurf_dir.join(".panda-hook.sha256");
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

            remove_panda_entries(&mut root, "pre_run_command");
            remove_panda_entries(&mut root, "post_run_command");

            std::fs::write(&hooks_path, serde_json::to_string_pretty(&root)?)?;
            println!("Removed PandaFilter entries from {}", hooks_path.display());
        }

        Ok(())
    }
}

/// Generate the Windsurf pre_run_command hook script.
///
/// Windsurf passes the command being run in stdin JSON as `{"command": "..."}`.
/// The script rewrites it via `panda rewrite`; if no rewrite is needed it exits 0
/// silently (Windsurf passes the original command through).
/// Always exits 0 — exit 2 would block execution entirely.
fn generate_windsurf_rewrite_script(panda_bin: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
# PandaFilter Windsurf pre_run_command hook
# Rewrites shell commands for token savings.
# Exits 0 (pass-through) or outputs rewritten command JSON.
INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.command // empty' 2>/dev/null)
if [ -z "$CMD" ]; then
  exit 0
fi
REWRITTEN=$(PANDA_SESSION_ID=$PPID "{panda_bin}" rewrite "$CMD" 2>/dev/null) || exit 0
if [ "$CMD" = "$REWRITTEN" ]; then
  exit 0
fi
jq -n --arg cmd "$REWRITTEN" '{{"command": $cmd}}'
"#,
        panda_bin = panda_bin
    )
}

/// Remove all PandaFilter entries from `hooks.<event>` in `root`.
fn remove_panda_entries(root: &mut serde_json::Value, event: &str) {
    if let Some(arr) = root
        .get_mut("hooks")
        .and_then(|h| h.get_mut(event))
        .and_then(|e| e.as_array_mut())
    {
        arr.retain(|entry| {
            let cmd = entry["command"].as_str().unwrap_or("");
            !cmd.contains("panda") && !cmd.contains("ccr")
        });
    }
}

/// Insert a flat hook entry `{"command": command}` under `hooks.<event>`.
/// Skips insertion if the exact command is already present.
fn insert_hook_entry(root: &mut serde_json::Value, event: &str, command: &str) {
    let entry = serde_json::json!({ "command": command });

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

    let already = event_arr
        .iter()
        .any(|e| e["command"].as_str().unwrap_or("") == command);
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
        insert_hook_entry(&mut root, "pre_run_command", "/usr/local/bin/panda-rewrite.sh");
        insert_hook_entry(
            &mut root,
            "post_run_command",
            "PANDA_AGENT=windsurf /usr/local/bin/panda hook",
        );

        assert_eq!(root["hooks"]["pre_run_command"].as_array().unwrap().len(), 1);
        assert_eq!(root["hooks"]["post_run_command"].as_array().unwrap().len(), 1);

        // Inserting same command again should be a no-op
        insert_hook_entry(&mut root, "pre_run_command", "/usr/local/bin/panda-rewrite.sh");
        assert_eq!(root["hooks"]["pre_run_command"].as_array().unwrap().len(), 1);

        remove_panda_entries(&mut root, "pre_run_command");
        remove_panda_entries(&mut root, "post_run_command");

        assert!(root["hooks"]["pre_run_command"].as_array().unwrap().is_empty());
        assert!(root["hooks"]["post_run_command"].as_array().unwrap().is_empty());
    }

    #[test]
    fn remove_preserves_non_panda_entries() {
        let mut root = serde_json::json!({
            "hooks": {
                "pre_run_command": [
                    {"command": "/usr/bin/other-hook.sh"},
                    {"command": "/usr/local/bin/panda-rewrite.sh"}
                ]
            }
        });

        remove_panda_entries(&mut root, "pre_run_command");
        let arr = root["hooks"]["pre_run_command"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(arr[0]["command"].as_str().unwrap().contains("other-hook"));
    }

    #[test]
    fn script_contains_rewrite_call() {
        let script = generate_windsurf_rewrite_script("/usr/local/bin/panda");
        assert!(script.contains("/usr/local/bin/panda"));
        assert!(script.contains("rewrite"));
        assert!(script.contains(r#"'{"command": $cmd}'"#) || script.contains("command"));
    }
}
