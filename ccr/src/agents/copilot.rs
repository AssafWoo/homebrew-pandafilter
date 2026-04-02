//! VS Code Copilot agent installer.
//!
//! Copilot uses a PostToolUse hook that receives `{"tool_name": "shell", "tool_input": {"command": "..."}}`
//! and can return `{"updatedInput": {"command": "rewritten"}}` to rewrite the command.

use super::AgentInstaller;

pub struct CopilotInstaller;

impl AgentInstaller for CopilotInstaller {
    fn name(&self) -> &'static str {
        "VS Code Copilot"
    }

    fn install(&self, ccr_bin: &str) -> anyhow::Result<()> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;

        // VS Code extension hook directory
        let hook_dir = home.join(".vscode").join("extensions").join(".ccr-hook");
        std::fs::create_dir_all(&hook_dir)?;

        // PreToolUse rewrite script — Copilot snake_case format
        let script = generate_copilot_script(ccr_bin);
        let script_path = hook_dir.join("ccr-rewrite.sh");
        std::fs::write(&script_path, &script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script_path, perms)?;
        }

        // Update VS Code settings.json to register the hook
        let vscode_dir = home.join(".vscode");
        let settings_path = vscode_dir.join("settings.json");
        std::fs::create_dir_all(&vscode_dir)?;

        let mut settings: serde_json::Value = if settings_path.exists() {
            let content = std::fs::read_to_string(&settings_path)?;
            serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        // Register under github.copilot.advanced hooks
        let hook_cmd = format!("CCR_SESSION_ID=$PPID CCR_AGENT=copilot {} hook", ccr_bin);
        let copilot_advanced = settings
            .get_mut("github.copilot.advanced")
            .and_then(|v| v.as_object_mut())
            .is_some();

        if !copilot_advanced {
            if let Some(obj) = settings.as_object_mut() {
                obj.entry("github.copilot.advanced")
                    .or_insert_with(|| serde_json::json!({}));
            }
        }

        if let Some(adv) = settings
            .get_mut("github.copilot.advanced")
            .and_then(|v| v.as_object_mut())
        {
            adv.insert(
                "ccr.postToolUseHook".to_string(),
                serde_json::Value::String(hook_cmd.clone()),
            );
            adv.insert(
                "ccr.preToolUseScript".to_string(),
                serde_json::Value::String(script_path.to_string_lossy().to_string()),
            );
        }

        std::fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;

        println!("CCR hooks installed (VS Code Copilot):");
        println!("  Script:  {}", script_path.display());
        println!("  Settings: {}", settings_path.display());
        println!();
        println!("Note: Copilot hook activation depends on your VS Code extension configuration.");
        println!("      See https://github.com/assafwoo/ccr for integration details.");

        Ok(())
    }

    fn uninstall(&self) -> anyhow::Result<()> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;

        let hook_dir = home.join(".vscode").join("extensions").join(".ccr-hook");
        let script_path = hook_dir.join("ccr-rewrite.sh");

        if script_path.exists() {
            std::fs::remove_file(&script_path)?;
            println!("Removed {}", script_path.display());
        }

        let settings_path = home.join(".vscode").join("settings.json");
        if settings_path.exists() {
            let content = std::fs::read_to_string(&settings_path)?;
            let mut settings: serde_json::Value =
                serde_json::from_str(&content).unwrap_or(serde_json::json!({}));
            if let Some(adv) = settings
                .get_mut("github.copilot.advanced")
                .and_then(|v| v.as_object_mut())
            {
                adv.remove("ccr.postToolUseHook");
                adv.remove("ccr.preToolUseScript");
            }
            std::fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
            println!("Removed CCR entries from {}", settings_path.display());
        }

        Ok(())
    }
}

/// Generate the Copilot PreToolUse hook script.
///
/// Input format:  `{"tool_name": "shell", "tool_input": {"command": "..."}}`
/// Output format: `{"updatedInput": {"command": "rewritten"}}`
/// No-op:         exit 0 with empty output
fn generate_copilot_script(ccr_bin: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
# CCR VS Code Copilot PreToolUse hook
# Rewrites shell commands for token savings.
INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')
[ -z "$CMD" ] && exit 0
REWRITTEN=$(CCR_SESSION_ID=$PPID "{ccr_bin}" rewrite "$CMD" 2>/dev/null) || exit 0
[ "$CMD" = "$REWRITTEN" ] && exit 0
jq -n --arg cmd "$REWRITTEN" '{{"updatedInput": {{"command": $cmd}}}}'
"#,
        ccr_bin = ccr_bin
    )
}
