use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use panda_core::embed_client;

// 8 hours — covers a full work session so the daemon stays warm across all
// Claude Code invocations during the day without burning resources overnight.
// Override with PANDA_NO_IDLE_EXIT=1 to disable idle-exit entirely (the OS
// will reclaim the process on logout / shutdown instead).
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 28800; // 8 hours

#[derive(clap::Subcommand)]
pub enum DaemonAction {
    /// Start the embedding daemon in the background
    Start,
    /// Stop the embedding daemon
    Stop,
    /// Show daemon status
    Status,
    /// Install the daemon as a macOS LaunchAgent (auto-start at login)
    #[cfg(target_os = "macos")]
    InstallService,
    /// Uninstall the macOS LaunchAgent
    #[cfg(target_os = "macos")]
    UninstallService,
}

pub fn run(action: DaemonAction) -> Result<()> {
    match action {
        DaemonAction::Start => start(),
        DaemonAction::Stop => stop(),
        DaemonAction::Status => status(),
        #[cfg(target_os = "macos")]
        DaemonAction::InstallService => install_service(),
        #[cfg(target_os = "macos")]
        DaemonAction::UninstallService => uninstall_service(),
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

fn daemon_socket_connectable() -> bool {
    UnixStream::connect(embed_client::socket_path()).is_ok()
}

fn cleanup_daemon_files() {
    let _ = std::fs::remove_file(embed_client::pid_path());
    let _ = std::fs::remove_file(embed_client::socket_path());
}

fn start() -> Result<()> {
    if let Some(pid) = read_pid() {
        if process_alive(pid) {
            if daemon_socket_connectable() {
                println!("panda daemon already running (pid {})", pid);
                return Ok(());
            }
            // Process alive but socket not ready — may still be starting up.
            // Wait briefly before concluding it's stale.
            for _ in 0..6 {
                std::thread::sleep(Duration::from_millis(500));
                if daemon_socket_connectable() {
                    println!("panda daemon already running (pid {})", pid);
                    return Ok(());
                }
            }
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
            // Wait briefly so the grandchild can write the PID before we report.
            // With the early PID write fix, 200 ms is enough for the file I/O
            // even if model loading takes much longer.
            std::thread::sleep(Duration::from_millis(400));
            if let Some(child_pid) = read_pid() {
                if daemon_socket_connectable() {
                    println!("panda daemon started (pid {})", child_pid);
                } else {
                    println!("panda daemon started (pid {}) — loading embedding model...", child_pid);
                    println!("Run 'panda daemon status' to check when the socket is ready.");
                }
            } else {
                println!("panda daemon is starting...");
                println!("Run 'panda daemon status' in a few seconds to confirm.");
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
    use std::os::unix::io::AsRawFd;

    panda_core::summarizer::set_daemon_mode();
    ensure_dir(&pid_path);

    // Hold an exclusive flock on the PID file for the daemon's lifetime.
    // If a concurrent `daemon start` races past start()'s liveness check
    // (the window is wide — preload_model takes seconds), only one
    // daemon_main wins the lock; the other exits silently. The kernel
    // releases the lock on process death, so no cleanup is required.
    let pid_lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&pid_path)?;
    if unsafe { libc::flock(pid_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        std::process::exit(0);
    }

    let _ = std::fs::remove_file(&sock_path);

    // Write PID immediately after acquiring the exclusive lock so that
    // `panda daemon status` can observe the process during model loading
    // (which can take several seconds on first run — before this fix the
    // PID file was empty during startup, making status report "not running").
    // We'll overwrite with the same PID value again after binding the socket
    // to preserve the original invariant: PID file belongs to a socket owner.
    std::fs::write(&pid_path, format!("{}", std::process::id()))?;

    // Apply nice level inside the daemon process only.
    if let Ok(config) = crate::config_loader::load_config() {
        #[cfg(unix)]
        if config.global.nice_level > 0 {
            unsafe { libc::nice(config.global.nice_level) };
        }
        panda_core::summarizer::set_model_name(&config.global.bert_model);
        panda_core::summarizer::set_ort_threads(config.global.ort_threads);
    }
    if panda_core::summarizer::preload_model().is_err() {
        std::process::exit(1);
    }

    // Set restrictive permissions before binding the socket.
    let old_umask = unsafe { libc::umask(0o077) };
    let listener = UnixListener::bind(&sock_path)?;
    unsafe { libc::umask(old_umask) };

    // Overwrite with the same PID now that the socket is bound.
    // The flock above guarantees mutual exclusion; the PID recorded here
    // always belongs to the process that owns the socket.
    std::fs::write(&pid_path, format!("{}", std::process::id()))?;
    // Keep the lock fd alive until process exit.
    let _pid_lock = pid_lock;
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
    // Set PANDA_NO_IDLE_EXIT=1 to disable idle-exit entirely; the daemon will
    // then run until the OS kills it (e.g. on logout or reboot).
    let no_idle_exit = std::env::var_os("PANDA_NO_IDLE_EXIT")
        .map(|v| v == "1")
        .unwrap_or(false);
    let lr = last_request.clone();
    std::thread::spawn(move || {
        if no_idle_exit {
            return;
        }
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
                    std::thread::spawn(move || handle_connection(stream));
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

// ── Daemon-side embedding cache ───────────────────────────────────────────────
// Keyed by hash(text, normalize). Dev loops re-send the same lines constantly
// (recurring warnings, paths, test names), so hit rates in real sessions are
// high and the cache turns most embed requests into pure memory lookups.
// Override capacity with PANDA_EMBED_CACHE_CAP (0 disables the cache).

static EMBED_CACHE: std::sync::OnceLock<std::sync::Mutex<panda_core::embed_cache::EmbedCache>> =
    std::sync::OnceLock::new();

fn embed_cache_capacity() -> usize {
    std::env::var("PANDA_EMBED_CACHE_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(panda_core::embed_cache::DEFAULT_CAPACITY)
}

fn embed_cache() -> Option<&'static std::sync::Mutex<panda_core::embed_cache::EmbedCache>> {
    let cap = embed_cache_capacity();
    if cap == 0 {
        return None;
    }
    Some(EMBED_CACHE.get_or_init(|| {
        std::sync::Mutex::new(panda_core::embed_cache::EmbedCache::new(cap))
    }))
}

fn embed_uncached(texts: Vec<&str>, normalize: bool) -> Result<Vec<Vec<f32>>> {
    Ok(if normalize {
        panda_core::summarizer::embed_direct(texts)?
    } else {
        panda_core::summarizer::embed_raw(texts)?
    })
}

/// Embed `texts`, serving repeats from the cache and computing only misses.
/// The cache lock is released while the model runs so concurrent connections
/// with full cache hits are never blocked behind an inference pass.
fn embed_cached(texts: &[&str], normalize: bool) -> Result<Vec<Vec<f32>>> {
    let cache = match embed_cache() {
        Some(c) => c,
        None => return embed_uncached(texts.to_vec(), normalize),
    };

    let (mut found, miss_indices) = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .lookup_batch(texts, normalize);

    if !miss_indices.is_empty() {
        let miss_texts: Vec<&str> = miss_indices.iter().map(|&i| texts[i]).collect();
        let computed = embed_uncached(miss_texts, normalize)?;

        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        for (&i, emb) in miss_indices.iter().zip(computed.into_iter()) {
            guard.insert(panda_core::embed_cache::cache_key(texts[i], normalize), emb.clone());
            found[i] = Some(emb);
        }
    }

    // Every slot is Some now: hits were filled by lookup_batch, misses just above.
    Ok(found.into_iter().map(|e| e.unwrap_or_default()).collect())
}

fn process_request(req_buf: &[u8]) -> Result<serde_json::Value> {
    let req: serde_json::Value = serde_json::from_slice(req_buf)?;

    // Health-check ping — lightweight round-trip to verify daemon is alive and responsive.
    // Includes cache stats so `panda daemon status` and benchmarks can observe hit rates.
    if req.get("ping").and_then(|v| v.as_bool()) == Some(true) {
        let (size, hits, misses) = embed_cache()
            .map(|c| {
                let g = c.lock().unwrap_or_else(|e| e.into_inner());
                (g.len() as u64, g.hits, g.misses)
            })
            .unwrap_or((0, 0, 0));
        return Ok(serde_json::json!({
            "ok": true,
            "pong": true,
            "cache_size": size,
            "cache_hits": hits,
            "cache_misses": misses,
        }));
    }

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
    let embeddings = embed_cached(&text_refs, normalize)?;

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

#[cfg(target_os = "linux")]
fn read_rss_mb(pid: u32) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{}/statm", pid)).ok()?;
    let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096 / 1024 / 1024)
}

#[cfg(target_os = "macos")]
fn read_rss_mb(pid: u32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let kb: u64 = std::str::from_utf8(&out.stdout).ok()?.trim().parse().ok()?;
    Some(kb / 1024)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_rss_mb(_pid: u32) -> Option<u64> {
    None
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
        cleanup_daemon_files();
        return Ok(());
    }

    if !daemon_socket_connectable() {
        // Process is alive but socket isn't ready yet — it's still loading the
        // embedding model. Don't clean up; just report the transient state.
        println!("panda daemon is starting... (loading embedding model, pid {})", pid);
        println!("  Run `panda daemon status` again in a few seconds.");
        return Ok(());
    }

    let sock = embed_client::socket_path();

    let rss = read_rss_mb(pid);

    println!("panda daemon running (pid {})", pid);
    println!("  socket: {}", sock.display());
    if let Some(mb) = rss {
        println!("  memory: {} MB", mb);
    }

    Ok(())
}

// ── macOS LaunchAgent service installer ───────────────────────────────────────

#[cfg(target_os = "macos")]
fn launchagents_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join("Library").join("LaunchAgents"))
}

#[cfg(target_os = "macos")]
fn plist_path() -> Option<std::path::PathBuf> {
    launchagents_dir().map(|d| d.join("com.pandafilter.daemon.plist"))
}

#[cfg(target_os = "macos")]
fn install_service() -> Result<()> {
    let exe = std::env::current_exe()
        .context("cannot determine panda binary path")?
        .to_string_lossy()
        .to_string();

    let plist_dir = launchagents_dir()
        .context("cannot determine ~/Library/LaunchAgents")?;
    std::fs::create_dir_all(&plist_dir)?;
    let dest = plist_dir.join("com.pandafilter.daemon.plist");

    // Build plist inline so install works even without the assets/ directory.
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
    "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.pandafilter.daemon</string>

    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>daemon</string>
        <string>start</string>
    </array>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <true/>

    <key>ThrottleInterval</key>
    <integer>30</integer>

    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin</string>
        <key>PANDA_NO_IDLE_EXIT</key>
        <string>1</string>
    </dict>

    <key>StandardOutPath</key>
    <string>{log}</string>

    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        exe = exe,
        log = dirs::home_dir()
            .unwrap_or_default()
            .join("Library/Logs/pandafilter-daemon.log")
            .display(),
    );

    std::fs::write(&dest, &plist)
        .with_context(|| format!("failed to write plist to {}", dest.display()))?;

    // launchctl load is idempotent; if already loaded it prints a warning but exits 0.
    let status = std::process::Command::new("launchctl")
        .args(["load", &dest.to_string_lossy()])
        .status()
        .context("failed to run launchctl load")?;

    if status.success() {
        println!("panda daemon installed as LaunchAgent.");
        println!("  plist: {}", dest.display());
        println!("  The daemon will now start automatically at login and restart if it crashes.");
        println!("  To uninstall: panda daemon uninstall-service");
    } else {
        println!("plist written to {} but launchctl load failed (exit {}).", dest.display(), status);
        println!("Try: launchctl load {}", dest.display());
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_service() -> Result<()> {
    let dest = match plist_path() {
        Some(p) => p,
        None => anyhow::bail!("cannot determine ~/Library/LaunchAgents"),
    };

    if dest.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &dest.to_string_lossy()])
            .status();
        std::fs::remove_file(&dest)
            .with_context(|| format!("failed to remove {}", dest.display()))?;
        println!("panda daemon LaunchAgent uninstalled.");
    } else {
        println!("panda daemon LaunchAgent is not installed.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_request_ping_returns_pong() {
        let req = serde_json::json!({"ping": true});
        let req_bytes = serde_json::to_vec(&req).unwrap();
        let resp = process_request(&req_bytes).unwrap();
        assert_eq!(resp.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(resp.get("pong").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn process_request_empty_texts_returns_ok() {
        let req = serde_json::json!({"texts": []});
        let req_bytes = serde_json::to_vec(&req).unwrap();
        let resp = process_request(&req_bytes).unwrap();
        assert_eq!(resp.get("ok").and_then(|v| v.as_bool()), Some(true));
    }
}
