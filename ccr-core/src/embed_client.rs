use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RETRIES: u32 = 1;
const RETRY_DELAY: Duration = Duration::from_millis(200);
// Keep daemon wait short: if the socket isn't ready in 2s the daemon is still loading
// its ONNX model (which takes 5-15s). Rather than blocking the hook for 15s we
// return None immediately and let callers fall through to head+tail.
// The daemon continues loading in the background and will be warm for the next call.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_POLL_DELAY: Duration = Duration::from_millis(100);

static SOCKET_DIR: OnceLock<PathBuf> = OnceLock::new();

fn socket_dir() -> &'static PathBuf {
    SOCKET_DIR.get_or_init(|| {
        if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            PathBuf::from(runtime_dir).join("panda")
        } else {
            let uid = unsafe { libc::getuid() };
            PathBuf::from(format!("/tmp/panda-{}", uid))
        }
    })
}

pub fn socket_path() -> PathBuf {
    socket_dir().join("embed.sock")
}

pub fn pid_path() -> PathBuf {
    socket_dir().join("embed.pid")
}

/// Send a lightweight ping to the daemon to verify it is alive and responsive.
/// Returns `true` if the daemon responded with `{"ok": true, "pong": true}` within 2 seconds.
/// Returns `false` if the socket doesn't exist, connection fails, or response is wrong.
pub fn daemon_ping() -> bool {
    let sock = socket_path();
    if !sock.exists() {
        return false;
    }
    let mut stream = match UnixStream::connect(&sock) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let ping_timeout = Duration::from_secs(2);
    let _ = stream.set_read_timeout(Some(ping_timeout));
    let _ = stream.set_write_timeout(Some(ping_timeout));

    let req = serde_json::json!({"ping": true});
    let req_bytes = match serde_json::to_vec(&req) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let len = (req_bytes.len() as u32).to_be_bytes();
    if stream.write_all(&len).is_err() || stream.write_all(&req_bytes).is_err() {
        return false;
    }
    let mut len_buf = [0u8; 4];
    if stream.read_exact(&mut len_buf).is_err() {
        return false;
    }
    let resp_len = u32::from_be_bytes(len_buf) as usize;
    if resp_len > 1024 {
        return false;
    }
    let mut resp_buf = vec![0u8; resp_len];
    if stream.read_exact(&mut resp_buf).is_err() {
        return false;
    }
    let resp: serde_json::Value = match serde_json::from_slice(&resp_buf) {
        Ok(v) => v,
        Err(_) => return false,
    };
    resp.get("ok").and_then(|v| v.as_bool()) == Some(true)
        && resp.get("pong").and_then(|v| v.as_bool()) == Some(true)
}

pub fn daemon_embed(texts: &[&str], normalize: bool) -> Option<Vec<Vec<f32>>> {
    let sock = socket_path();
    let mut started = false;
    if !sock.exists() {
        started = try_auto_start();
        if !started {
            return None;
        }
    }

    let startup_deadline = Instant::now() + STARTUP_TIMEOUT;
    for attempt in 0..=MAX_RETRIES {
        if Instant::now() >= startup_deadline {
            return None;
        }

        let stream = match UnixStream::connect(&sock) {
            Ok(s) => s,
            Err(_) => {
                if !started {
                    started = try_auto_start();
                    if !started {
                        return None;
                    }
                }
                let mut connected = false;
                while Instant::now() < startup_deadline {
                    std::thread::sleep(STARTUP_POLL_DELAY);
                    if let Ok(s) = UnixStream::connect(&sock) {
                        if let Some(result) = send_with_timeouts(s, texts, normalize) {
                            return Some(result);
                        }
                        connected = true;
                        break;
                    }
                }
                if !connected {
                    return None;
                }
                if attempt == MAX_RETRIES {
                    return None;
                }
                std::thread::sleep(RETRY_DELAY);
                continue;
            }
        };

        if let Some(result) = send_with_timeouts(stream, texts, normalize) {
            return Some(result);
        }

        if attempt < MAX_RETRIES {
            std::thread::sleep(RETRY_DELAY);
        }
    }

    None
}

fn send_with_timeouts(
    stream: UnixStream,
    texts: &[&str],
    normalize: bool,
) -> Option<Vec<Vec<f32>>> {
    let _ = stream.set_read_timeout(Some(REQUEST_TIMEOUT));
    let _ = stream.set_write_timeout(Some(REQUEST_TIMEOUT));
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

    if resp_len > 10_000_000 {
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

fn try_auto_start() -> bool {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return false,
    };

    // `daemon start` is idempotent — it checks liveness and exits if already running.
    let mut child = match std::process::Command::new(exe)
        .args(["daemon", "start"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };

    std::thread::sleep(Duration::from_millis(250));
    match child.try_wait() {
        Ok(Some(status)) => status.success(),
        Ok(None) => true,
        Err(_) => true,
    }
}

/// Fire-and-forget daemon start — spawns `panda daemon start` and returns immediately
/// without waiting. Used for proactive warm-up at hook entry so the daemon is loading
/// during handler routing (200-500ms) rather than waiting until the first embed call.
pub fn try_auto_start_detached() -> bool {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return false,
    };
    std::process::Command::new(exe)
        .args(["daemon", "start"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}
