use anyhow::{bail, Result};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use panda_core::embed_client;

const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 900; // 15 minutes

#[derive(clap::Subcommand)]
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

fn ensure_dir(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
}

fn read_pid() -> Option<u32> {
    let content = std::fs::read_to_string(embed_client::pid_path()).ok()?;
    content.trim().parse().ok()
}

fn process_alive(pid: u32) -> bool {
    let ret = unsafe { libc::kill(pid as i32, 0) };
    ret == 0 || (ret == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

fn cleanup_daemon_files() {
    let _ = std::fs::remove_file(embed_client::pid_path());
    let _ = std::fs::remove_file(embed_client::socket_path());
}

fn start() -> Result<()> {
    if let Some(pid) = read_pid() {
        if process_alive(pid) {
            println!("panda daemon already running (pid {})", pid);
            return Ok(());
        }
        cleanup_daemon_files();
    }

    let sock_path = embed_client::socket_path();
    let pid_path = embed_client::pid_path();
    ensure_dir(&sock_path);

    // Fork early, before any config loading or thread creation, to avoid
    // UB from forking a multi-threaded Rust process.
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

        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
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

static SHUTDOWN_PIPE: std::sync::OnceLock<(i32, i32)> = std::sync::OnceLock::new();

extern "C" fn sigterm_handler(_sig: libc::c_int) {
    // Write a single byte to the pipe — write(2) is async-signal-safe per POSIX.
    if let Some(&(_, write_fd)) = SHUTDOWN_PIPE.get() {
        unsafe { libc::write(write_fd, b"x" as *const _ as *const libc::c_void, 1) };
    }
}

fn daemon_main(sock_path: PathBuf, pid_path: PathBuf) -> Result<()> {
    ensure_dir(&pid_path);
    std::fs::write(&pid_path, format!("{}", std::process::id()))?;

    let _ = std::fs::remove_file(&sock_path);

    // Apply nice level inside the daemon process only.
    if let Ok(config) = crate::config_loader::load_config() {
        #[cfg(unix)]
        if config.global.nice_level > 0 {
            unsafe {
                *libc::__errno_location() = 0;
                let ret = libc::nice(config.global.nice_level);
                if ret == -1 && *libc::__errno_location() != 0 {
                    eprintln!("warning: nice({}) failed", config.global.nice_level);
                }
            }
        }
        panda_core::summarizer::set_model_name(&config.global.bert_model);
    }
    if panda_core::summarizer::preload_model().is_err() {
        let _ = std::fs::remove_file(&pid_path);
        std::process::exit(1);
    }

    // Set restrictive permissions before binding the socket.
    let old_umask = unsafe { libc::umask(0o077) };
    let listener = UnixListener::bind(&sock_path)?;
    unsafe { libc::umask(old_umask) };
    // Blocking listener — no busy-wait.

    // Self-pipe for async-signal-safe shutdown.
    let mut pipe_fds = [0i32; 2];
    if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } != 0 {
        bail!("pipe() failed");
    }
    SHUTDOWN_PIPE.set((pipe_fds[0], pipe_fds[1])).ok();

    unsafe {
        libc::signal(libc::SIGTERM, sigterm_handler as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, sigterm_handler as *const () as libc::sighandler_t);
    }

    let last_request = Arc::new(AtomicU64::new(now_secs()));
    let listener_fd = {
        use std::os::unix::io::AsRawFd;
        listener.as_raw_fd()
    };

    // Idle timeout watchdog — sends SIGTERM to self to unblock poll().
    let lr = last_request.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(30));
            let idle = now_secs().saturating_sub(lr.load(Ordering::Relaxed));
            if idle > DEFAULT_IDLE_TIMEOUT_SECS {
                unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
                break;
            }
        }
    });

    // Use poll() to wait on both the listener and the shutdown pipe.
    let mut pollfds = [
        libc::pollfd { fd: listener_fd, events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: pipe_fds[0], events: libc::POLLIN, revents: 0 },
    ];

    loop {
        let ret = unsafe { libc::poll(pollfds.as_mut_ptr(), 2, -1) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break;
        }

        // Shutdown pipe readable — time to exit.
        if pollfds[1].revents & libc::POLLIN != 0 {
            break;
        }

        // Listener has a connection.
        if pollfds[0].revents & libc::POLLIN != 0 {
            match listener.accept() {
                Ok((stream, _)) => {
                    last_request.store(now_secs(), Ordering::Relaxed);
                    handle_connection(stream);
                }
                Err(_) => break,
            }
        }
    }

    cleanup_daemon_files();
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

    let normalize = req.get("normalize").and_then(|v| v.as_bool()).unwrap_or(true);
    let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let embeddings = if normalize {
        panda_core::summarizer::embed_direct(text_refs)?
    } else {
        panda_core::summarizer::embed_raw(text_refs)?
    };

    Ok(serde_json::json!({
        "ok": true,
        "embeddings": embeddings,
    }))
}

fn stop() -> Result<()> {
    let pid = match read_pid() {
        Some(p) => p,
        None => {
            println!("panda daemon is not running");
            return Ok(());
        }
    };

    if !process_alive(pid) {
        println!("panda daemon is not running (stale pid file)");
        cleanup_daemon_files();
        return Ok(());
    }

    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }

    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if !process_alive(pid) {
            println!("panda daemon stopped");
            cleanup_daemon_files();
            return Ok(());
        }
    }

    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    cleanup_daemon_files();
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
