//! Integration tests for `panda init --agent codex`.

use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn panda() -> Command {
    Command::cargo_bin("panda").unwrap()
}

fn codex_dir(home: &TempDir) -> PathBuf {
    home.path().join(".codex")
}

fn hooks_json(home: &TempDir) -> PathBuf {
    codex_dir(home).join("hooks.json")
}

fn rewrite_script(home: &TempDir) -> PathBuf {
    codex_dir(home).join("panda-rewrite.sh")
}

fn run_codex_init(home: &TempDir) {
    fs::create_dir_all(codex_dir(home)).unwrap();
    panda()
        .args(["init", "--agent", "codex"])
        .env("HOME", home.path())
        .assert()
        .success();
}

fn run_codex_uninstall(home: &TempDir) {
    panda()
        .args(["init", "--agent", "codex", "--uninstall"])
        .env("HOME", home.path())
        .assert()
        .success();
}

fn read_hooks(home: &TempDir) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(hooks_json(home)).unwrap()).unwrap()
}

#[test]
fn codex_init_installs_bash_hooks_and_current_rewrite_contract() {
    let home = TempDir::new().unwrap();
    run_codex_init(&home);

    let root = read_hooks(&home);
    for event in ["PreToolUse", "PostToolUse"] {
        let entries = root["hooks"][event].as_array().unwrap();
        let panda_entry = entries
            .iter()
            .find(|entry| {
                entry["hooks"].as_array().is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        hook["command"]
                            .as_str()
                            .is_some_and(|command| command.contains("panda"))
                    })
                })
            })
            .unwrap_or_else(|| panic!("{event} Panda hook missing"));
        assert_eq!(panda_entry["matcher"], "^Bash$");
    }

    let script = fs::read_to_string(rewrite_script(&home)).unwrap();
    assert!(script.contains("\"hookEventName\": \"PreToolUse\""));
    assert!(script.contains("\"permissionDecision\": \"allow\""));
    assert!(script.contains("\"updatedInput\": $updated"));
    assert!(!script.contains("\"decision\": \"allow\""));
}

#[cfg(unix)]
#[test]
fn codex_init_marks_rewrite_script_executable() {
    use std::os::unix::fs::PermissionsExt;

    let home = TempDir::new().unwrap();
    run_codex_init(&home);

    let mode = fs::metadata(rewrite_script(&home))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0);
}

#[test]
fn codex_init_migrates_legacy_entries_and_preserves_other_handlers() {
    let home = TempDir::new().unwrap();
    fs::create_dir_all(codex_dir(&home)).unwrap();
    let existing = serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "shell",
                "hooks": [
                    {"type": "command", "command": "/usr/bin/other-hook"},
                    {"type": "command", "command": "/usr/local/bin/pandac-format"},
                    {"type": "command", "command": "/opt/hooks/ccr-report"},
                    {"type": "command", "command": "/old/panda-rewrite.sh"}
                ]
            }],
            "PostToolUse": [{
                "matcher": "shell",
                "hooks": [
                    {"type": "command", "command": "/usr/bin/post-review"},
                    {"type": "command", "command": "PANDA_AGENT=codex /old/panda hook"}
                ]
            }]
        }
    });
    fs::write(
        hooks_json(&home),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    run_codex_init(&home);
    run_codex_init(&home);

    let root = read_hooks(&home);
    for (event, preserved) in [
        ("PreToolUse", "/usr/bin/other-hook"),
        ("PostToolUse", "/usr/bin/post-review"),
    ] {
        let entries = root["hooks"][event].as_array().unwrap();
        let commands: Vec<&str> = entries
            .iter()
            .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
            .filter_map(|hook| hook["command"].as_str())
            .collect();

        assert!(commands.contains(&preserved));
        if event == "PreToolUse" {
            assert!(commands.contains(&"/usr/local/bin/pandac-format"));
            assert!(commands.contains(&"/opt/hooks/ccr-report"));
        }
        assert_eq!(
            commands
                .iter()
                .filter(|command| {
                    if event == "PreToolUse" {
                        command.contains("panda-rewrite.sh")
                    } else {
                        command.contains("PANDA_AGENT=codex")
                    }
                })
                .count(),
            1
        );
        assert!(entries.iter().any(|entry| entry["matcher"] == "^Bash$"));
    }
}

#[test]
fn codex_uninstall_removes_only_panda_handlers() {
    let home = TempDir::new().unwrap();
    fs::create_dir_all(codex_dir(&home)).unwrap();
    let existing = serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "^Bash$",
                "hooks": [
                    {"type": "command", "command": "/usr/bin/other-hook"},
                    {"type": "command", "command": "/old/panda-rewrite.sh"}
                ]
            }],
            "PostToolUse": [{
                "matcher": "^Bash$",
                "hooks": [
                    {"type": "command", "command": "/usr/bin/post-review"},
                    {"type": "command", "command": "PANDA_AGENT=codex /old/panda hook"}
                ]
            }]
        }
    });
    fs::write(
        hooks_json(&home),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    run_codex_uninstall(&home);

    let root = read_hooks(&home);
    for (event, preserved) in [
        ("PreToolUse", "/usr/bin/other-hook"),
        ("PostToolUse", "/usr/bin/post-review"),
    ] {
        let commands: Vec<&str> = root["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
            .filter_map(|hook| hook["command"].as_str())
            .collect();

        assert_eq!(commands, vec![preserved]);
    }
}
