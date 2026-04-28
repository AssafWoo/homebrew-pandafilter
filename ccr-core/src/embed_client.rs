use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub fn socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("panda").join("embed.sock")
    } else if let Ok(uid) = std::env::var("UID").or_else(|_| {
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .ok_or(std::env::VarError::NotPresent)
    }) {
        PathBuf::from(format!("/tmp/panda-{}", uid)).join("embed.sock")
    } else {
        PathBuf::from("/tmp/panda-embed.sock")
    }
}

pub fn pid_path() -> PathBuf {
    let mut p = socket_path();
    p.set_file_name("embed.pid");
    p
}

pub fn daemon_embed(texts: &[&str], normalize: bool) -> Option<Vec<Vec<f32>>> {
    let sock = socket_path();
    if !sock.exists() {
        try_auto_start();
        return None;
    }

    let stream = match UnixStream::connect(&sock) {
        Ok(s) => s,
        Err(_) => {
            try_auto_start();
            return None;
        }
    };

    stream.set_read_timeout(Some(REQUEST_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT)).ok()?;

    send_request(stream, texts, normalize)
}

fn send_request(
    mut stream: UnixStream,
    texts: &[&str],
    normalize: bool,
) -> Option<Vec<Vec<f32>>> {
    let request = serde_json::json!({
        "texts": texts,
        "normalize": normalize,
    });
    let payload = serde_json::to_vec(&request).ok()?;

    // Length-prefixed framing: 4-byte big-endian length + JSON
    let len = (payload.len() as u32).to_be_bytes();
    stream.write_all(&len).ok()?;
    stream.write_all(&payload).ok()?;

    // Read response
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).ok()?;
    let resp_len = u32::from_be_bytes(len_buf) as usize;

    if resp_len > 100_000_000 {
        return None;
    }

    let mut resp_buf = vec![0u8; resp_len];
    stream.read_exact(&mut resp_buf).ok()?;

    let resp: serde_json::Value = serde_json::from_slice(&resp_buf).ok()?;

    if resp.get("ok")?.as_bool()? {
        let embeddings: Vec<Vec<f32>> = resp
            .get("embeddings")?
            .as_array()?
            .iter()
            .map(|arr| {
                arr.as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect())
            })
            .collect::<Option<Vec<Vec<f32>>>>()?;
        Some(embeddings)
    } else {
        None
    }
}

fn try_auto_start() {
    let pid = pid_path();
    if pid.exists() {
        // Daemon may be starting up — don't spawn another
        return;
    }

    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return,
    };

    let _ = std::process::Command::new(exe)
        .args(["daemon", "start"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
