//! Integration tests for the niri-pattern xwayland-satellite helpers.
//!
//! Covers the pure pieces (socket pre-binding + spawn-command construction).
//! Event-loop integration is out of scope for this file.

use std::fs;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use emskin::xwayland_satellite::sockets::{
    clear_out_pending_connections, format_lock_body, x11_lock_path, Unlink,
};
use emskin::xwayland_satellite::{
    build_spawn_command, setup_connection, test_ondemand, SpawnConfig, X11Sockets,
};

// -----------------------------------------------------------------
// helpers

/// Pick a high, process-unique starting display to avoid clashing with
/// whatever the dev machine already has on `:0..:9`. Uses nanotime bits
/// so parallel test runs collide only with astronomical probability.
fn test_display_start() -> u32 {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // Display >= 100 is well clear of any real X server on a dev machine.
    100 + ((ns as u32 ^ std::process::id()) % 1000)
}

fn tmpdir(tag: &str) -> PathBuf {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!("emskin-xwls-{}-{}-{}", tag, std::process::id(), ns));
    fs::create_dir_all(&d).unwrap();
    d
}

/// Write an executable shell script into `dir/name` that runs `body` and
/// returns it as a path suitable for `test_ondemand` / `build_spawn_command`.
fn write_script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    let mut f = fs::File::create(&p).unwrap();
    writeln!(f, "#!/bin/sh").unwrap();
    writeln!(f, "{body}").unwrap();
    drop(f);
    let mut perm = fs::metadata(&p).unwrap().permissions();
    perm.set_mode(0o755);
    fs::set_permissions(&p, perm).unwrap();
    p
}

// -----------------------------------------------------------------
// X11Sockets

#[test]
fn setup_connection_binds_free_display_and_exposes_paths() {
    let start = test_display_start();
    let sockets: X11Sockets = setup_connection(start).expect("setup_connection should succeed");

    assert!(
        sockets.display >= start,
        "display {} should be >= start {start}",
        sockets.display
    );
    assert_eq!(sockets.display_name, format!(":{}", sockets.display));
    assert!(
        sockets.lock_path().exists(),
        "lock file {:?} should exist",
        sockets.lock_path()
    );
    assert!(
        sockets.unix_socket_path().exists(),
        "unix socket {:?} should exist",
        sockets.unix_socket_path()
    );
    assert!(
        sockets.unix_fd.as_raw_fd() >= 0,
        "unix_fd should be a valid fd"
    );
}

/// Pick a "definitely dead" PID by spawning `true` and waiting on it.
/// The kernel recycles PIDs, but the test only needs the PID to be dead
/// *right now* — `kill(pid, 0)` returns ESRCH for exited-and-reaped
/// processes on Linux. Extremely slim race window where the kernel
/// recycles this specific PID to another process between `wait()` and
/// `pick_x11_display()` call; test would false-pass if it happened, not
/// false-fail, so harmless.
fn spawn_and_reap() -> u32 {
    let mut child = std::process::Command::new("true")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn /bin/true");
    let pid = child.id();
    let _ = child.wait();
    pid
}

/// Write an X11 lock file containing `pid` at the path for display
/// `display`. Reuses the production helpers so test fixtures stay in
/// sync with the format the scanner parses.
fn write_lock(display: u32, pid: u32) -> PathBuf {
    let path = PathBuf::from(x11_lock_path(display));
    // Delete first in case a previous run left one here at this display.
    let _ = fs::remove_file(&path);
    fs::write(&path, format_lock_body(pid)).expect("write lock file");
    path
}

#[test]
fn stale_lock_from_dead_pid_gets_cleaned_up() {
    // Regression guard for "no free X11 display number found" when every
    // lock in the scan window belongs to dead PIDs. pick_x11_display must
    // detect the dead owner and reclaim the slot.
    let start = test_display_start();
    let dead_pid = spawn_and_reap();
    let stale_lock = write_lock(start, dead_pid);
    let _cleanup = Unlink::new(stale_lock.clone());

    let sockets =
        setup_connection(start).expect("stale lock should be reclaimed, not reported as blocked");
    assert_eq!(
        sockets.display, start,
        "setup should reclaim the stale slot at :{start}, got :{}",
        sockets.display
    );

    // The lock file should now hold *our* PID, not the dead one. Read the
    // contents back and assert that it at least parses to something other
    // than the old dead PID.
    let contents = fs::read_to_string(&stale_lock).unwrap_or_default();
    let written_pid = contents.trim().parse::<u32>().ok();
    assert_ne!(
        written_pid,
        Some(dead_pid),
        "reclaimed lock must not still contain the dead PID {dead_pid}"
    );
}

#[test]
fn stale_lock_with_corrupt_content_gets_cleaned_up() {
    // A lock file that fails to parse as a PID is a leftover from something
    // that didn't follow the convention (or got truncated). Treat as stale.
    let start = test_display_start();
    let stale_lock = PathBuf::from(x11_lock_path(start));
    let _ = fs::remove_file(&stale_lock);
    fs::write(&stale_lock, "garbage not-a-pid\n").unwrap();
    let _cleanup = Unlink::new(stale_lock.clone());

    let sockets = setup_connection(start).expect("corrupt lock should be reclaimed as stale");
    assert_eq!(sockets.display, start);
}

#[test]
fn live_pid_lock_is_respected_and_scan_advances() {
    // Converse of the stale case — a lock owned by a LIVE PID must NOT be
    // reclaimed. Use our own PID; we are obviously alive.
    let start = test_display_start();
    let live_lock = write_lock(start, std::process::id());
    let _cleanup = Unlink::new(live_lock.clone());

    let sockets =
        setup_connection(start).expect("scan should advance past the live-PID lock, not fail");
    assert!(
        sockets.display > start,
        "live-PID lock at :{start} must be respected; got display :{} (should be > {start})",
        sockets.display
    );
    // The original lock file must still exist (we didn't delete it).
    assert!(
        live_lock.exists(),
        "live-PID lock file was deleted — the reclaim logic must not touch locks with live owners"
    );
}

#[test]
fn multiple_consecutive_stale_locks_all_reclaimable() {
    // Models the production failure: the first N display slots all have
    // dead locks. pick_x11_display must walk forward, cleaning each stale
    // lock it encounters, and succeed within the scan window.
    let start = test_display_start();
    let dead_pid = spawn_and_reap();

    // Create 5 stale locks at `start..start+5`. Can't use spawn_and_reap
    // for each because PIDs would differ, but having them all point at the
    // same dead PID is still a realistic stale-lock scenario.
    let _cleanups: Vec<_> = (0..5)
        .map(|offset| {
            let path = write_lock(start + offset, dead_pid);
            Unlink::new(path)
        })
        .collect();

    let sockets = setup_connection(start).expect(
        "pick_x11_display should reclaim the first stale slot encountered, not walk past all 5",
    );
    assert_eq!(
        sockets.display, start,
        "first slot :{start} should be reclaimed immediately"
    );
}

#[test]
fn dropping_x11sockets_unlinks_lock_and_socket() {
    let start = test_display_start();
    let sockets = setup_connection(start).unwrap();
    let lock = sockets.lock_path();
    let sock = sockets.unix_socket_path();
    assert!(lock.exists() && sock.exists());

    drop(sockets);

    assert!(!lock.exists(), "lock should be unlinked on drop");
    assert!(!sock.exists(), "unix socket should be unlinked on drop");
}

#[test]
fn unix_fd_accepts_a_client_connection() {
    let start = test_display_start();
    let sockets = setup_connection(start).unwrap();

    // Spawn a tiny client thread — accept() is blocking.
    let path = sockets.unix_socket_path();
    let t = std::thread::spawn(move || UnixStream::connect(&path).unwrap());

    let listener = UnixListener::from(sockets.unix_fd.try_clone().unwrap());
    let (_server, _addr) = listener
        .accept()
        .expect("accept should succeed on our pre-bound socket");
    let _client = t.join().unwrap();
}

#[test]
fn clear_out_pending_connections_drains_but_keeps_listener_usable() {
    // Standalone listener to keep this test independent of display allocation.
    let dir = tmpdir("drain");
    let sock = dir.join("s");
    let listener = UnixListener::bind(&sock).unwrap();

    // Two queued clients.
    let _c1 = UnixStream::connect(&sock).unwrap();
    let _c2 = UnixStream::connect(&sock).unwrap();

    let fd = clear_out_pending_connections(listener.into());
    // A third client should still be able to connect + be accepted through the same fd.
    let _c3 = UnixStream::connect(&sock).unwrap();
    let listener = UnixListener::from(fd);
    let (_accepted, _) = listener.accept().expect("listener should still accept");
}

// -----------------------------------------------------------------
// test_ondemand

#[test]
fn test_ondemand_true_when_binary_exits_zero() {
    let dir = tmpdir("ondemand-ok");
    let bin = write_script(&dir, "sat", "exit 0");
    assert!(test_ondemand(&bin));
}

#[test]
fn test_ondemand_false_when_binary_exits_nonzero() {
    let dir = tmpdir("ondemand-fail");
    let bin = write_script(&dir, "sat", "exit 1");
    assert!(!test_ondemand(&bin));
}

#[test]
fn test_ondemand_false_when_binary_missing() {
    assert!(!test_ondemand(std::path::Path::new(
        "/nonexistent/definitely-not-a-real-binary"
    )));
}

// -----------------------------------------------------------------
// build_spawn_command

fn spawn_config(binary: PathBuf) -> SpawnConfig {
    SpawnConfig {
        binary,
        wayland_socket: PathBuf::from("/run/user/1000/wayland-emskin"),
        xdg_runtime_dir: PathBuf::from("/run/user/1000"),
    }
}

#[test]
fn spawn_command_argv_prefix_is_display_then_listenfds() {
    let dir = tmpdir("argv");
    let bin = write_script(&dir, "sat", "exit 0");
    let start = test_display_start();
    let sockets = setup_connection(start).unwrap();
    let cfg = spawn_config(bin);

    let cmd = build_spawn_command(&cfg, &sockets);
    let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
    let args_str: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    assert_eq!(args_str[0], sockets.display_name, "first arg must be :N");

    // -listenfd appears at least once and is always followed by a parseable fd.
    let mut saw_unix = false;
    let mut i = 1;
    while i < args_str.len() {
        if args_str[i] == "-listenfd" {
            let fd: i32 = args_str
                .get(i + 1)
                .expect("-listenfd must be followed by an fd")
                .parse()
                .expect("fd arg must be numeric");
            assert!(fd >= 0);
            saw_unix = true;
            i += 2;
        } else {
            i += 1;
        }
    }
    assert!(saw_unix, "expected at least one -listenfd argument");
}

#[test]
fn spawn_command_env_sets_wayland_display_and_runtime_dir() {
    let dir = tmpdir("env");
    let bin = write_script(&dir, "sat", "exit 0");
    let start = test_display_start();
    let sockets = setup_connection(start).unwrap();
    let cfg = spawn_config(bin);

    let cmd = build_spawn_command(&cfg, &sockets);
    let envs: Vec<(String, Option<String>)> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();

    let wayland = envs
        .iter()
        .find(|(k, _)| k == "WAYLAND_DISPLAY")
        .expect("WAYLAND_DISPLAY must be set");
    assert_eq!(
        wayland.1.as_deref(),
        Some("/run/user/1000/wayland-emskin"),
        "WAYLAND_DISPLAY should match spawn_config",
    );

    let runtime = envs
        .iter()
        .find(|(k, _)| k == "XDG_RUNTIME_DIR")
        .expect("XDG_RUNTIME_DIR must be set");
    assert_eq!(runtime.1.as_deref(), Some("/run/user/1000"));
}

#[test]
fn spawn_command_env_removes_display() {
    let dir = tmpdir("env-rm");
    let bin = write_script(&dir, "sat", "exit 0");
    let start = test_display_start();
    let sockets = setup_connection(start).unwrap();
    let cfg = spawn_config(bin);

    let cmd = build_spawn_command(&cfg, &sockets);
    let removed: bool = cmd
        .get_envs()
        .any(|(k, v)| k.to_string_lossy() == "DISPLAY" && v.is_none());
    assert!(
        removed,
        "DISPLAY should be marked for removal so the child doesn't inherit host :0"
    );
}

#[test]
fn spawn_command_uses_configured_binary() {
    let dir = tmpdir("bin");
    let bin = write_script(&dir, "sat", "exit 0");
    let start = test_display_start();
    let sockets = setup_connection(start).unwrap();
    let cfg = spawn_config(bin.clone());

    let cmd = build_spawn_command(&cfg, &sockets);
    assert_eq!(
        std::path::Path::new(cmd.get_program()),
        bin,
        "program should be the configured binary path"
    );
    // Sanity: let the future GREEN impl drop before we exit the test — both
    // guard files must be unlinked so we don't pollute /tmp on assertion
    // failures.
    drop(cmd);
    drop(sockets);
    let _ = Duration::from_millis(0); // keep `time` import used in this test block
}
