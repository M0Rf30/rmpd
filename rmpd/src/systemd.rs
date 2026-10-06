// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minimal, dependency-free systemd integration, mirroring what MPD does
//! through libsystemd:
//!
//! - `sd_notify` readiness (`READY=1`, `STOPPING=1`) over `$NOTIFY_SOCKET`
//!   (MPD `src/Main.cxx` sends `READY=1` once startup completed; its units use
//!   `Type=notify`);
//! - socket activation via `$LISTEN_PID` / `$LISTEN_FDS`
//!   (MPD `src/Listen.cxx`, `listen_systemd_activation`).
//!
//! The environment-reading wrappers are thin; the logic lives in functions
//! that take their inputs as parameters so it can be tested without touching
//! the (process-global) environment.

use std::io;
use std::net::TcpListener;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::warn;

/// First file descriptor passed by systemd (`SD_LISTEN_FDS_START`).
pub const SD_LISTEN_FDS_START: RawFd = 3;

/// Environment variable holding the notification socket address.
const NOTIFY_SOCKET: &str = "NOTIFY_SOCKET";

/// Whether the process was started by a supervisor that expects
/// `sd_notify` messages.
pub fn notify_socket_present() -> bool {
    std::env::var_os(NOTIFY_SOCKET).is_some_and(|v| !v.is_empty())
}

/// Send `message` (e.g. `"READY=1"`) to the datagram socket `socket`, which is
/// an absolute filesystem path or, on Linux, an abstract socket name prefixed
/// with `@` (same syntax as `$NOTIFY_SOCKET`).
///
/// A no-op returning `Ok(())` on platforms other than Linux.
#[cfg(target_os = "linux")]
pub fn notify_to(socket: &str, message: &str) -> io::Result<()> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};

    let sock = UnixDatagram::unbound()?;
    if let Some(name) = socket.strip_prefix('@') {
        let addr = SocketAddr::from_abstract_name(name.as_bytes())?;
        sock.send_to_addr(message.as_bytes(), &addr)?;
    } else if socket.starts_with('/') {
        sock.send_to(message.as_bytes(), socket)?;
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NOTIFY_SOCKET must be an absolute path or an @abstract name",
        ));
    }
    Ok(())
}

/// See the Linux variant; systemd notification is not supported elsewhere.
#[cfg(not(target_os = "linux"))]
pub fn notify_to(_socket: &str, _message: &str) -> io::Result<()> {
    Ok(())
}

/// `sd_notify(0, message)`: send `message` to `$NOTIFY_SOCKET`, doing nothing
/// when it is unset. Failures are logged, never fatal.
pub fn notify(message: &str) {
    let Some(socket) = std::env::var(NOTIFY_SOCKET).ok().filter(|s| !s.is_empty()) else {
        return;
    };
    if let Err(e) = notify_to(&socket, message) {
        warn!("sd_notify({message:?}) to {socket:?} failed: {e}");
    }
}

/// Tell systemd the service finished starting up (`READY=1`).
pub fn notify_ready() {
    notify("READY=1");
}

/// Tell systemd the service is shutting down (`STOPPING=1`). Only the first
/// call sends anything, so every shutdown path can call it unconditionally.
pub fn notify_stopping() {
    static SENT: AtomicBool = AtomicBool::new(false);
    if !SENT.swap(true, Ordering::SeqCst) {
        notify("STOPPING=1");
    }
}

/// Sockets inherited from systemd, split by kind.
#[derive(Debug, Default)]
pub struct Activated {
    /// Listening TCP sockets (IPv4 or IPv6), already non-blocking.
    pub tcp: Vec<TcpListener>,
    /// Listening Unix stream sockets, already non-blocking.
    pub unix: Vec<UnixListener>,
}

/// Number of passed descriptors according to the `LISTEN_PID` / `LISTEN_FDS`
/// values, like `sd_listen_fds()`: `Ok(0)` when the variables are unset or
/// were meant for another process (`LISTEN_PID` != `pid`), `Err` when they are
/// malformed.
pub fn parse_listen_fds(
    listen_pid: Option<&str>,
    listen_fds: Option<&str>,
    pid: u32,
) -> Result<usize, String> {
    let Some(listen_pid) = listen_pid else {
        return Ok(0);
    };
    let target: u32 = listen_pid
        .parse()
        .map_err(|_| format!("invalid LISTEN_PID {listen_pid:?}"))?;
    if target != pid {
        return Ok(0);
    }
    let Some(listen_fds) = listen_fds else {
        return Ok(0);
    };
    let n: usize = listen_fds
        .parse()
        .map_err(|_| format!("invalid LISTEN_FDS {listen_fds:?}"))?;
    let in_range = RawFd::try_from(n)
        .ok()
        .and_then(|n| n.checked_add(SD_LISTEN_FDS_START))
        .is_some();
    if !in_range {
        return Err(format!("LISTEN_FDS {n} is out of range"));
    }
    Ok(n)
}

/// Take the sockets systemd passed to this process (socket activation), or
/// `None` when the process was not socket-activated.
///
/// The `LISTEN_*` variables are removed from the environment afterwards, as
/// `sd_listen_fds(true)` does, so they are not leaked to child processes.
///
/// Must be called before any other thread is started: it mutates the
/// process environment.
pub fn take_activated_listeners() -> io::Result<Option<Activated>> {
    let listen_pid = std::env::var("LISTEN_PID").ok();
    let listen_fds = std::env::var("LISTEN_FDS").ok();

    // SAFETY: called from `main` before the async runtime (or any other
    // thread) exists, so nothing can race on the environment.
    unsafe {
        std::env::remove_var("LISTEN_PID");
        std::env::remove_var("LISTEN_FDS");
        std::env::remove_var("LISTEN_FDNAMES");
    }

    let n = parse_listen_fds(
        listen_pid.as_deref(),
        listen_fds.as_deref(),
        std::process::id(),
    )
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if n == 0 {
        return Ok(None);
    }

    let end = SD_LISTEN_FDS_START + RawFd::try_from(n).unwrap_or(0);
    // SAFETY: systemd guarantees descriptors SD_LISTEN_FDS_START.. are open
    // and ours alone (LISTEN_PID matched this process).
    unsafe { adopt_fds(SD_LISTEN_FDS_START..end) }.map(Some)
}

/// Classify and take ownership of each descriptor: set `FD_CLOEXEC` and
/// non-blocking mode, then wrap TCP and Unix stream listeners. Anything else
/// (datagram sockets, non-sockets, sockets that are not listening) is an
/// error, as such a socket could never serve MPD clients.
///
/// # Safety
/// Every descriptor must be open and exclusively owned by the caller; they
/// are closed on drop (including on the error path).
pub unsafe fn adopt_fds(fds: impl IntoIterator<Item = RawFd>) -> io::Result<Activated> {
    let mut out = Activated::default();
    for fd in fds {
        // SAFETY: forwarded from this function's contract.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let kind = classify(&owned)
            .map_err(|e| io::Error::new(e.kind(), format!("unsupported socket on fd {fd}: {e}")))?;
        set_cloexec(&owned)?;
        match kind {
            Kind::Tcp => {
                let l = TcpListener::from(owned);
                l.set_nonblocking(true)?;
                out.tcp.push(l);
            }
            Kind::Unix => {
                let l = UnixListener::from(owned);
                l.set_nonblocking(true)?;
                out.unix.push(l);
            }
        }
    }
    Ok(out)
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Tcp,
    Unix,
}

fn classify(fd: &OwnedFd) -> io::Result<Kind> {
    use nix::sys::socket::{AddressFamily, SockType, SockaddrLike, SockaddrStorage};
    use nix::sys::socket::{getsockname, getsockopt, sockopt};

    let unsupported = |msg: String| io::Error::new(io::ErrorKind::InvalidInput, msg);

    let addr: SockaddrStorage = getsockname(fd.as_raw_fd()).map_err(io::Error::from)?;
    let family = addr
        .family()
        .ok_or_else(|| unsupported("unknown address family".to_owned()))?;
    let ty = getsockopt(fd, sockopt::SockType).map_err(io::Error::from)?;
    if ty != SockType::Stream {
        return Err(unsupported(format!("not a stream socket ({ty:?})")));
    }
    #[cfg(target_os = "linux")]
    if !getsockopt(fd, sockopt::AcceptConn).map_err(io::Error::from)? {
        return Err(unsupported("socket is not listening".to_owned()));
    }
    match family {
        AddressFamily::Inet | AddressFamily::Inet6 => Ok(Kind::Tcp),
        AddressFamily::Unix => Ok(Kind::Unix),
        other => Err(unsupported(format!("unsupported address family {other:?}"))),
    }
}

fn set_cloexec(fd: &OwnedFd) -> io::Result<()> {
    use nix::fcntl::{FcntlArg, FdFlag, fcntl};
    fcntl(fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))
        .map(|_| ())
        .map_err(io::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::IntoRawFd;
    use std::path::PathBuf;

    fn unique_path(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rmpd-sd-{tag}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn parse_listen_fds_matches_sd_listen_fds() {
        // Not activated at all.
        assert_eq!(parse_listen_fds(None, None, 42), Ok(0));
        assert_eq!(parse_listen_fds(None, Some("2"), 42), Ok(0));
        // Meant for another process (e.g. inherited across fork/exec).
        assert_eq!(parse_listen_fds(Some("41"), Some("2"), 42), Ok(0));
        // Ours.
        assert_eq!(parse_listen_fds(Some("42"), Some("2"), 42), Ok(2));
        assert_eq!(parse_listen_fds(Some("42"), Some("0"), 42), Ok(0));
        assert_eq!(parse_listen_fds(Some("42"), None, 42), Ok(0));
        // Malformed.
        assert!(parse_listen_fds(Some("x"), Some("1"), 42).is_err());
        assert!(parse_listen_fds(Some("42"), Some("-1"), 42).is_err());
        assert!(parse_listen_fds(Some("42"), Some("many"), 42).is_err());
        assert!(parse_listen_fds(Some("42"), Some("99999999999"), 42).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn notify_to_delivers_datagram_to_path_socket() {
        let path = unique_path("notify");
        let rx = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        rx.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();

        notify_to(path.to_str().unwrap(), "READY=1").unwrap();

        let mut buf = [0u8; 64];
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1");
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn notify_to_delivers_datagram_to_abstract_socket() {
        use std::os::linux::net::SocketAddrExt;
        use std::os::unix::net::{SocketAddr, UnixDatagram};

        let name = format!("rmpd-test-notify-{}", std::process::id());
        let addr = SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let rx = UnixDatagram::bind_addr(&addr).unwrap();
        rx.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();

        notify_to(&format!("@{name}"), "STOPPING=1").unwrap();

        let mut buf = [0u8; 64];
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"STOPPING=1");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn notify_to_rejects_relative_addresses_and_missing_peers() {
        assert_eq!(
            notify_to("relative/socket", "READY=1").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(notify_to("/nonexistent/rmpd-notify.sock", "READY=1").is_err());
    }

    #[test]
    fn adopt_fds_wraps_tcp_and_unix_listeners() {
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp_addr = tcp.local_addr().unwrap();
        let path = unique_path("unix");
        let unix = UnixListener::bind(&path).unwrap();

        let fds = [tcp.into_raw_fd(), unix.into_raw_fd()];
        // SAFETY: the descriptors were just released by into_raw_fd().
        let activated = unsafe { adopt_fds(fds) }.unwrap();
        assert_eq!(activated.tcp.len(), 1);
        assert_eq!(activated.unix.len(), 1);
        assert_eq!(activated.tcp[0].local_addr().unwrap(), tcp_addr);

        // The adopted sockets are non-blocking and close-on-exec, and still serve.
        assert_eq!(
            activated.tcp[0].accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let flags = nix::fcntl::fcntl(&activated.tcp[0], nix::fcntl::FcntlArg::F_GETFD).unwrap();
        assert_ne!(flags & nix::libc::FD_CLOEXEC, 0, "FD_CLOEXEC must be set");

        let _client = std::net::TcpStream::connect(tcp_addr).unwrap();
        let _uclient = std::os::unix::net::UnixStream::connect(&path).unwrap();
        // Connections are queued in the backlog; poll briefly for accept.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match activated.tcp[0].accept() {
                Ok(_) => break,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline, "accept timed out");
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(e) => panic!("accept failed: {e}"),
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn adopt_fds_rejects_datagram_sockets() {
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        // SAFETY: the descriptor was just released by into_raw_fd().
        let err = unsafe { adopt_fds([udp.into_raw_fd()]) }.unwrap_err();
        assert!(err.to_string().contains("not a stream socket"), "{err}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn adopt_fds_rejects_unlistening_sockets() {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, socket};
        let fd = socket(
            AddressFamily::Inet,
            SockType::Stream,
            SockFlag::empty(),
            None,
        )
        .unwrap();
        // SAFETY: ownership of the descriptor moves into adopt_fds.
        let err = unsafe { adopt_fds([fd.into_raw_fd()]) }.unwrap_err();
        assert!(err.to_string().contains("not listening"), "{err}");
    }
}
