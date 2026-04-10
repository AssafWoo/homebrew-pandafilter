use anyhow::Result;
use panda_core::tokens;
use std::process::Command;

/// Execute a command raw (no filtering), record analytics, write tee.
pub fn run(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("panda proxy: no command specified");
    }

    let cmd_name = args[0].clone();

    let output = Command::new(&args[0])
        .args(&args[1..])
        .output()
        .map_err(|e| anyhow::anyhow!("failed to execute '{}': {}", cmd_name, e))?;

    let raw_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Print raw output (preserve stdout/stderr distinction via print!/eprint!)
    if !raw_stdout.is_empty() {
        print!("{}", raw_stdout);
    }
    if !raw_stderr.is_empty() {
        eprint!("{}", raw_stderr);
    }

    // Write tee
    let combined = raw_stdout.clone() + &raw_stderr;
    write_tee(&cmd_name, &combined);

    // Record analytics with savings = 0 (no filtering applied)
    let token_count = tokens::count_tokens(&combined);
    let analytics = panda_core::analytics::Analytics::compute_with_command(
        token_count,
        token_count,
        Some(cmd_name),
    );
    append_analytics(&analytics);

    let code = output.status.code().unwrap_or(1);
    if code != 0 {
        std::process::exit(code);
    }

    Ok(())
}

fn write_tee(cmd: &str, content: &str) {
    let Some(tee_dir) = dirs::data_local_dir().map(|d| d.join("panda").join("tee")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&tee_dir);

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let safe_cmd = cmd
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>();

    let path = tee_dir.join(format!("{}_{}_proxy.log", ts, safe_cmd));
    let _ = std::fs::write(&path, content);
}

fn append_analytics(analytics: &panda_core::analytics::Analytics) {
    if let Some(data_dir) = dirs::data_local_dir() {
        let panda_dir = data_dir.join("panda");
        let _ = std::fs::create_dir_all(&panda_dir);
        let path = panda_dir.join("analytics.jsonl");
        if let Ok(json) = serde_json::to_string(analytics) {
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "{}", json)
                });
        }
    }
}
