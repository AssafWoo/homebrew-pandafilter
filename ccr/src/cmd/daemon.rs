use anyhow::{bail, Result};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use panda_core::embed_client;

const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 900; // 15 minutes

#[derive(clap::Subcommand, Clone)]
pub enum DaemonAction {
    /// Start the embedding daemon in the background
    Start,
    /// Stop the embedding daemon
    Stop,
    /// Show daemon status
    Status,
}

pub fn run(action: DaemonAction) -> Result<()> {
    match action {
        DaemonAction::Start => start(),
        DaemonAction::Stop => stop(),
        DaemonAction::Status => status(),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn ensure_dir(path: &PathBuf) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
}

fn read_pid() -> Option<u32> {
    let content = std::fs::read_to_string(embed_client::pid_path()).ok()?;
    content.trim().parse().ok()
}

fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn start() -> Result<()> {
    if let Some(pid) = read_pid() {
        if process_alive(pid) {
            println!("panda daemon already running (pid {})", pid);
            return Ok(());
        }
        let _ = std::fs::remove_file(embed_client::pid_path());
        let _ = std::fs::remove_file(embed_client::socket_path());
    }

    let sock_path = embed_client::socket_path();
    let pid_path = embed_client::pid_path();
    ensure_dir(&sock_path);

    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            bail!("fork failed");
        }
        if pid > 0 {
            std::thread::sleep(Duration::from_millis(200));
            if let Some(child_pid) = read_pid() {
                println!("panda daemon started (pid {})", child_pid);
            } else {
                println!("panda daemon starting...");
            }
            return Ok(());
        }

        libc::setsid();

        let pid2 = libc::fork();
        if pid2 < 0 {
            std::process::exit(1);
        }
        if pid2 > 0 {
            std::process::exit(0);
        }

        let devnull = libc::open(b"/dev/null\0".as_ptr() as *const _, libc::O_RDWR);
        if devnull >= 0 {
            libc::dup2(devnull, 0);
            libc::dup2(devnull, 1);
            libc::dup2(devnull, 2);
            if devnull > 2 {
                libc::close(devnull);
            }
        }
    }

    daemon_main(sock_path, pid_path)
}

static SIGTERM_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigterm_handler(_sig: libc::c_int) {
    SIGTERM_RECEIVED.store(true, Ordering::SeqCst);
}

fn daemon_main(sock_path: PathBuf, pid_path: PathBuf) -> Result<()> {
    ensure_dir(&pid_path);
    std::fs::write(&pid_path, format!("{}", std::process::id()))?;

    let _ = std::fs::remove_file(&sock_path);

    if let Ok(config) = crate::config_loader::load_config() {
        panda_core::summarizer::set_model_name(&config.global.bert_model);
    }
    if let Err(_) = panda_core::summarizer::preload_model() {
        let _ = std::fs::remove_file(&pid_path);
        std::process::exit(1);
    }

    let listener = UnixListener::bind(&sock_path)?;
    listener.set_nonblocking(true)?;

    unsafe {
        libc::signal(libc::SIGTERM, sigterm_handler as *const () as libc::sighandler_t);
    }

    let last_request = Arc::new(AtomicU64::new(now_secs()));

    // Idle timeout watchdog
    let lr = last_request.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(30));
            let idle = now_secs() - lr.load(Ordering::Relaxed);
            if idle > DEFAULT_IDLE_TIMEOUT_SECS {
                SIGTERM_RECEIVED.store(true, Ordering::SeqCst);
                break;
            }
        }
    });

    while !SIGTERM_RECEIVED.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                last_request.store(now_secs(), Ordering::Relaxed);
                handle_connection(stream);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }

    let _ = std::fs::remove_file(&sock_path);
    let _ = std::fs::remove_file(&pid_path);
    std::process::exit(0);
}

fn handle_connection(mut stream: std::os::unix::net::UnixStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut len_buf = [0u8; 4];
    if stream.read_exact(&mut len_buf).is_err() {
        return;
    }
    let req_len = u32::from_be_bytes(len_buf) as usize;
    if req_len > 10_000_000 {
        return;
    }

    let mut req_buf = vec![0u8; req_len];
    if stream.read_exact(&mut req_buf).is_err() {
        return;
    }

    let response = match process_request(&req_buf) {
        Ok(resp) => resp,
        Err(e) => serde_json::json!({
            "ok": false,
            "error": format!("{}", e),
        }),
    };

    let resp_bytes = match serde_json::to_vec(&response) {
        Ok(b) => b,
        Err(_) => return,
    };

    let len = (resp_bytes.len() as u32).to_be_bytes();
    let _ = stream.write_all(&len);
    let _ = stream.write_all(&resp_bytes);
}

fn process_request(req_buf: &[u8]) -> Result<serde_json::Value> {
    let req: serde_json::Value = serde_json::from_slice(req_buf)?;

    let texts: Vec<String> = req
        .get("texts")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    if texts.is_empty() {
        return Ok(serde_json::json!({
            "ok": true,
            "embeddings": [],
        }));
    }

    let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let embeddings = panda_core::summarizer::embed_batch(&text_refs)?;

    Ok(serde_json::json!({
        "ok": true,
        "embeddings": embeddings,
    }))
}

fn stop() -> Result<()> {
    let pid_path = embed_client::pid_path();
    let sock_path = embed_client::socket_path();

    let pid = match read_pid() {
        Some(p) => p,
        None => {
            println!("panda daemon is not running");
            return Ok(());
        }
    };

    if !process_alive(pid) {
        println!("panda daemon is not running (stale pid file)");
        let _ = std::fs::remove_file(&pid_path);
        let _ = std::fs::remove_file(&sock_path);
        return Ok(());
    }

    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }

    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if !process_alive(pid) {
            println!("panda daemon stopped");
            let _ = std::fs::remove_file(&pid_path);
            let _ = std::fs::remove_file(&sock_path);
            return Ok(());
        }
    }

    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    let _ = std::fs::remove_file(&pid_path);
    let _ = std::fs::remove_file(&sock_path);
    println!("panda daemon killed");
    Ok(())
}

fn status() -> Result<()> {
    let pid = match read_pid() {
        Some(p) => p,
        None => {
            println!("panda daemon is not running");
            return Ok(());
        }
    };

    if !process_alive(pid) {
        println!("panda daemon is not running (stale pid file)");
        return Ok(());
    }

    let sock = embed_client::socket_path();

    let rss = std::fs::read_to_string(format!("/proc/{}/statm", pid))
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .nth(1)?
                .parse::<u64>()
                .ok()
                .map(|pages| pages * 4096 / 1024 / 1024)
        });

    println!("panda daemon running (pid {})", pid);
    println!("  socket: {}", sock.display());
    if let Some(mb) = rss {
        println!("  memory: {} MB", mb);
    }

    Ok(())
}
