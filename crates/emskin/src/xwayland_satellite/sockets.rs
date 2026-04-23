//! X11 display lock + socket allocation.
//!
//! Ported from niri `src/utils/xwayland/mod.rs` (GPL-3.0-or-later) — see
//! the upstream `setup_connection` / `pick_x11_display` for the original.

use std::fmt;
use std::fs;
use std::io::Write as _;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{SocketAddr, UnixListener};
use std::path::PathBuf;

const X11_TMP_UNIX_DIR: &str = "/tmp/.X11-unix";
/// Upper bound on consecutive slots to scan. Bumped from 50 after a
/// real-world incident where 50 stale `/tmp/.X*-lock` files from prior
/// e2e runs filled the 0..49 range exactly and the old "any
/// AlreadyExists = blocked" scanner gave up. 256 is comfortably above
/// any realistic concurrent-compositor count while still bounded so a
/// genuinely full system fails fast.
const MAX_DISPLAY_ATTEMPTS: u32 = 256;
const MAX_SOCKET_BIND_RETRIES: u32 = 50;

/// Filesystem path of the `/tmp/.X<n>-lock` file for display `:n`.
pub fn x11_lock_path(n: u32) -> String {
    format!("/tmp/.X{n}-lock")
}

/// Filesystem path of the unix-domain socket `/tmp/.X11-unix/X<n>` for
/// display `:n`.
pub fn x11_socket_path(n: u32) -> String {
    format!("{X11_TMP_UNIX_DIR}/X{n}")
}

/// X11 lock-file body format per Xorg convention: space-padded 10-char
/// decimal PID followed by newline (11 bytes total), matching what
/// `_XSERVTransMakeAllCOTSServerListeners` writes.
pub fn format_lock_body(pid: u32) -> String {
    format!("{pid:>10}\n")
}

/// Probe whether a Unix PID still names a live process. Returns `true`
/// when the kernel confirms the pid exists (running or zombie), `false`
/// otherwise. Shared helper so both the stale-lock reclaim path here
/// and the `EmacsState::set_child` zombie-safety tests use one
/// implementation with one SAFETY comment.
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: `libc::kill(pid, 0)` sends no signal; it is a documented
    // probe of pid validity. No side effects on the target process.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[derive(Debug)]
pub enum SetupError {
    Io(std::io::Error),
    NoFreeDisplay,
    X11DirPermissions(String),
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::NoFreeDisplay => write!(f, "no free X11 display number found"),
            Self::X11DirPermissions(m) => write!(f, "X11 directory: {m}"),
        }
    }
}

impl std::error::Error for SetupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for SetupError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// RAII guard that unlinks a filesystem path on drop.
#[derive(Debug)]
pub struct Unlink(pub(crate) PathBuf);

impl Unlink {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Unlink {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Pre-bound X11 sockets awaiting a satellite spawn.
#[derive(Debug)]
pub struct X11Sockets {
    pub display: u32,
    pub display_name: String,
    /// `None` on non-Linux targets (no abstract sockets there).
    pub abstract_fd: Option<OwnedFd>,
    pub unix_fd: OwnedFd,
    pub(crate) _unix_guard: Unlink,
    pub(crate) _lock_guard: Unlink,
}

impl X11Sockets {
    pub fn unix_socket_path(&self) -> PathBuf {
        PathBuf::from(format!("{X11_TMP_UNIX_DIR}/X{}", self.display))
    }

    pub fn lock_path(&self) -> PathBuf {
        PathBuf::from(format!("/tmp/.X{}-lock", self.display))
    }
}

/// Ensure `/tmp/.X11-unix` exists with the expected sticky-bit perms.
/// Mirrors mutter's behaviour; matches niri.
fn ensure_x11_unix_dir() -> Result<(), SetupError> {
    match fs::create_dir(X11_TMP_UNIX_DIR) {
        Ok(()) => {
            // Set sticky + world-writable perms to match convention.
            let perms = std::os::unix::fs::PermissionsExt::from_mode(0o1777);
            let _ = fs::set_permissions(X11_TMP_UNIX_DIR, perms);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(SetupError::Io(e)),
    }
}

/// Atomically claim an X display number by creating `/tmp/.X<N>-lock` with
/// `O_EXCL|O_CREAT`. Stale locks (owner PID is dead, or contents don't
/// parse) are detected and cleared before giving up on a slot — this
/// mirrors what Xorg's `_XSERVTransMakeAllCOTSServerListeners` does and
/// prevents accumulated e2e-test crashes from permanently blocking
/// display numbers.
///
/// Returns `(display, lock_file, guard)`.
fn pick_x11_display(start: u32) -> Result<(u32, fs::File, Unlink), SetupError> {
    for n in start..start + MAX_DISPLAY_ATTEMPTS {
        if let Some((fd, guard)) = try_claim_slot(n)? {
            return Ok((n, fd, guard));
        }
    }
    Err(SetupError::NoFreeDisplay)
}

/// Try to claim display slot `n`. Returns `Ok(Some(..))` on success,
/// `Ok(None)` if the slot is held by a live owner or we couldn't
/// reclaim it, `Err` on a genuinely unexpected I/O error.
fn try_claim_slot(n: u32) -> Result<Option<(fs::File, Unlink)>, SetupError> {
    let lock_path = x11_lock_path(n);
    match try_create_lock(&lock_path) {
        Ok(slot) => Ok(Some(slot)),
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(SetupError::Io(e)),
        Err(_) => {
            if !is_stale_lock(&lock_path) {
                return Ok(None);
            }
            tracing::debug!(
                "reclaiming stale X11 display :{n} (lock file has dead or unparseable owner)"
            );
            if let Err(e) = reclaim_stale_display(n) {
                tracing::warn!("stale-lock reclaim of :{n} failed: {e}");
                return Ok(None);
            }
            Ok(try_create_lock(&lock_path).ok())
        }
    }
}

fn try_create_lock(lock_path: &str) -> std::io::Result<(fs::File, Unlink)> {
    let fd = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o444)
        .open(lock_path)?;
    // Write our own PID so a future peer scanning this slot can stale-
    // check us in reverse. Best-effort — the lock-as-flag is what
    // matters for mutual exclusion.
    let _ = fs::write(lock_path, format_lock_body(std::process::id()));
    Ok((fd, Unlink(PathBuf::from(lock_path))))
}

/// A lock is "stale" if it exists but its owner no longer does. Two
/// cases qualify: the PID inside parses and its owner is gone, or the
/// file is unparseable / empty (a corrupt leftover that no live
/// process considers its own anymore).
fn is_stale_lock(path: &str) -> bool {
    let Ok(contents) = fs::read_to_string(path) else {
        // Can't even read the file (permissions? race?) — don't delete.
        return false;
    };
    let Some(pid) = contents.trim().parse::<u32>().ok().filter(|p| *p > 0) else {
        return true;
    };
    !pid_alive(pid)
}

/// Remove the stale lock plus any leftover unix socket for display `n`.
/// The abstract Linux socket (`@/tmp/.X11-unix/X<n>`) lives in the
/// kernel's abstract namespace and vanishes automatically with its
/// owner's death, so we don't need to touch it.
fn reclaim_stale_display(n: u32) -> std::io::Result<()> {
    fs::remove_file(x11_lock_path(n))?;
    match fs::remove_file(x11_socket_path(n)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(target_os = "linux")]
fn bind_abstract(display: u32) -> std::io::Result<UnixListener> {
    use std::os::linux::net::SocketAddrExt;
    let name = format!("{X11_TMP_UNIX_DIR}/X{display}");
    let addr = SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
    UnixListener::bind_addr(&addr)
}

fn bind_unix(display: u32) -> std::io::Result<(UnixListener, Unlink)> {
    let path = format!("{X11_TMP_UNIX_DIR}/X{display}");
    // Unlink stale leftovers. Niri does the same.
    let _ = fs::remove_file(&path);
    let guard = Unlink(PathBuf::from(&path));
    let addr = SocketAddr::from_pathname(&path)?;
    UnixListener::bind_addr(&addr).map(|listener| (listener, guard))
}

fn open_display_sockets(
    display: u32,
) -> std::io::Result<(Option<UnixListener>, UnixListener, Unlink)> {
    #[cfg(target_os = "linux")]
    let a = Some(bind_abstract(display)?);
    #[cfg(not(target_os = "linux"))]
    let a = None;

    let (u, g) = bind_unix(display)?;
    Ok((a, u, g))
}

/// Scan for a free display starting at `start`, bind lock + sockets, return
/// the bundle. Ported from niri `setup_connection`.
pub fn setup_connection(start: u32) -> Result<X11Sockets, SetupError> {
    ensure_x11_unix_dir()?;

    let mut n = start;
    let mut attempt = 0u32;
    let (display, lock_guard, abstract_listener, unix_listener, unix_guard) = loop {
        let (display, mut lock_fd, lock_guard) = pick_x11_display(n)?;

        // Write our PID — X clients use this as an advisory cue.
        let pid = std::process::id();
        let pid_string = format!("{pid:>10}\n");
        if let Err(e) = lock_fd.write_all(pid_string.as_bytes()) {
            return Err(SetupError::Io(e));
        }
        drop(lock_fd);

        match open_display_sockets(display) {
            Ok((a, u, g)) => break (display, lock_guard, a, u, g),
            Err(e) => {
                if attempt >= MAX_SOCKET_BIND_RETRIES {
                    return Err(SetupError::Io(e));
                }
                // lock_guard drops here → unlinks the lock file we just claimed
                // before we try a higher display number.
                n = display + 1;
                attempt += 1;
                continue;
            }
        }
    };

    let display_name = format!(":{display}");
    let abstract_fd = abstract_listener.map(|l| {
        let l: OwnedFd = l.into();
        // Sanity: fd should be valid.
        debug_assert!(l.as_raw_fd() >= 0);
        l
    });
    let unix_fd: OwnedFd = unix_listener.into();

    Ok(X11Sockets {
        display,
        display_name,
        abstract_fd,
        unix_fd,
        _unix_guard: unix_guard,
        _lock_guard: lock_guard,
    })
}

/// Non-blocking accept-drain used before re-arming event sources. Public
/// because the (future) event-loop integration needs to call it between
/// satellite crashes and rebinding the Generic sources. See niri
/// `satellite.rs:clear_out_pending_connections`.
pub fn clear_out_pending_connections(fd: OwnedFd) -> OwnedFd {
    let listener = UnixListener::from(fd);
    let _ = listener.set_nonblocking(true);
    while listener.accept().is_ok() {}
    let _ = listener.set_nonblocking(false);
    OwnedFd::from(listener)
}
