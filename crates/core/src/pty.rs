//! Pseudo-terminals: what `-t` actually buys, and what it costs.
//!
//! A pipe and a terminal are not the same thing, and programs can tell.
//! `isatty(1)` decides whether `ls` colours its output, whether `apt` draws
//! a progress bar, whether a shell prints a prompt at all, and whether
//! Ctrl-C reaches the process as a signal or as the byte `0x03`. A
//! container whose stdio is a pipe is a container where an interactive
//! shell does not work, however correct everything else is.
//!
//! # Where the pty is allocated, and the limitation that follows
//!
//! On the **host**, not inside the container.
//!
//! `runc` does the other thing: the container opens `/dev/ptmx` in its own
//! `devpts` instance after the pivot and passes the master back over a
//! unix socket (`--console-socket`). That is more correct and it needs the
//! child to build a `SCM_RIGHTS` control message in the window where it
//! may not allocate.
//!
//! Allocating on the host instead means the file descriptors work exactly
//! as they should — a descriptor has no namespace — but the slave's
//! *pathname* belongs to the host's `devpts`, not the container's. So
//! `ttyname(0)` inside the container names a `/dev/pts/N` that is a
//! different terminal in the container's own instance, or none at all, and
//! `ps` shows `?` for the tty column. Programs that ask "am I a terminal"
//! are unaffected; programs that ask "which terminal am I" are. That is
//! the trade, and it is written here rather than discovered later.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use crate::sys::Error as Errno;

pub struct Pty {
    /// Held by whoever is relaying bytes.
    pub master: OwnedFd,
    /// Becomes the container's stdin, stdout and stderr.
    pub slave: OwnedFd,
}

/// Allocate a pty pair.
pub fn open() -> Result<Pty, Errno> {
    // SAFETY: each call is a plain syscall on a descriptor we own.
    unsafe {
        let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if m < 0 {
            return Err(last());
        }
        let master = OwnedFd::from_raw_fd(m);
        if libc::grantpt(m) < 0 || libc::unlockpt(m) < 0 {
            return Err(last());
        }
        // `ptsname_r` rather than `ptsname`: the latter returns a pointer
        // into a static buffer, which is a data race waiting for the first
        // caller who allocates two ptys from two threads.
        let mut name = [0u8; 128];
        if libc::ptsname_r(m, name.as_mut_ptr().cast(), name.len()) != 0 {
            return Err(last());
        }
        // NOT O_CLOEXEC: this is about to become the container's stdio
        // across an execve.
        let s = libc::open(name.as_ptr().cast(), libc::O_RDWR | libc::O_NOCTTY);
        if s < 0 {
            return Err(last());
        }
        Ok(Pty {
            master,
            slave: OwnedFd::from_raw_fd(s),
        })
    }
}

/// Tell the pty how big it is.
///
/// Without this a shell believes it has the default 24x80 and wraps its
/// lines in the wrong place, which looks like corruption rather than like
/// a missing ioctl.
pub fn set_size(master: &OwnedFd, rows: u16, cols: u16) -> Result<(), Errno> {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
    if rc < 0 { Err(last()) } else { Ok(()) }
}

/// The size of a terminal, if `fd` is one.
pub fn size_of(fd: i32) -> Option<(u16, u16)> {
    let mut ws: libc::winsize = unsafe { core::mem::zeroed() };
    let rc = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
    (rc == 0 && ws.ws_row > 0).then_some((ws.ws_row, ws.ws_col))
}

pub fn is_tty(fd: i32) -> bool {
    unsafe { libc::isatty(fd) == 1 }
}

/// Make the already-dup'd slave this process's controlling terminal.
///
/// Called by the child, between `clone3` and `execve`, so it is two
/// syscalls and nothing else.
///
/// Both are required and neither is obvious:
///
/// - `setsid()` makes a new session with no controlling terminal. Without
///   it the child keeps the *caller's* terminal, and a Ctrl-C typed at the
///   outer shell goes to both.
/// - `TIOCSCTTY` then attaches this one. Without it the pty has no
///   foreground process group, so the kernel never turns `^C` into
///   `SIGINT` and an interactive program cannot be interrupted at all.
///
/// # Safety
///
/// May only be called on the child branch of a clone, after the slave has
/// been dup'd onto 0, 1 and 2.
pub unsafe fn make_controlling() -> Result<(), Errno> {
    // `setsid` fails EPERM when we are already a process-group leader,
    // which is harmless here: it means a session already exists and the
    // TIOCSCTTY below is what matters.
    unsafe { libc::setsid() };
    let rc = unsafe { libc::ioctl(0, libc::TIOCSCTTY, 0) };
    if rc < 0 { Err(last()) } else { Ok(()) }
}

/// The caller's terminal, put in raw mode and restored on drop.
///
/// Raw mode is what moves the decisions from the outer terminal to the
/// container: no line buffering, so a keystroke arrives immediately; no
/// echo, so the container's own echo is the only one; no signal
/// generation, so `^C` is delivered to the container as a byte and
/// becomes `SIGINT` *there*.
///
/// `Drop` is the whole design. Every path out of an interactive session —
/// the command exiting, an error, a panic, the user killing the container
/// from another window — has to restore the terminal, and a shell left in
/// raw mode is a shell the user has to `reset` blind.
pub struct RawMode {
    fd: i32,
    saved: libc::termios,
}

impl RawMode {
    pub fn enter(fd: i32) -> Option<Self> {
        if !is_tty(fd) {
            return None;
        }
        let mut saved: libc::termios = unsafe { core::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
            return None;
        }
        let mut raw = saved;
        unsafe { libc::cfmakeraw(&mut raw) };
        // Block until at least one byte, with no timer: a relay should
        // sleep rather than spin.
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return None;
        }
        Some(Self { fd, saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // TCSADRAIN, not TCSANOW: whatever the container wrote last should
        // reach the screen before the terminal changes under it.
        unsafe { libc::tcsetattr(self.fd, libc::TCSADRAIN, &self.saved) };
    }
}

/// Copy between the caller's terminal and the pty until the pty ends.
///
/// Returns when the master reports EOF or an error, which happens when the
/// last process holding the slave exits.
///
/// Three details that are each a bug if missed:
///
/// - **`EIO` from the master is the normal ending**, not a failure. Linux
///   reports the last slave closing that way, and treating it as an error
///   prints a spurious message every time a shell exits.
/// - **stdin closing must not end the session.** A pipeline like
///   `echo hi | rustainer run -i …` closes stdin immediately, and the
///   container is entitled to keep running. The loop stops polling stdin
///   and carries on.
/// - **The window size is re-read on every wakeup**, because the
///   supervisor has called `setsid` and so never receives `SIGWINCH`.
///   Polling it is a single ioctl and means a resize is picked up within
///   one timeout rather than never.
pub fn relay(master: &OwnedFd, stdin: i32, stdout: i32, mut tee: impl FnMut(&[u8])) {
    let mut buf = [0u8; 8192];
    // Polled until it ends, terminal or not: `echo hi | rustainer run -i`
    // is a pipe, and the container is entitled to read it.
    let mut stdin_open = true;
    let mut last_size = size_of(stdout);
    if let Some((r, c)) = last_size {
        let _ = set_size(master, r, c);
    }

    loop {
        let mut fds = [
            libc::pollfd {
                fd: master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if stdin_open { stdin } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, 250) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }

        let size = size_of(stdout);
        if size != last_size {
            last_size = size;
            if let Some((r, c)) = size {
                let _ = set_size(master, r, c);
            }
        }

        if fds[0].revents != 0 {
            match unsafe { libc::read(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } {
                // EOF, or EIO which is how Linux reports the last slave
                // closing. Both mean the session is over.
                n if n <= 0 => return,
                n => {
                    let got = &buf[..n as usize];
                    tee(got);
                    write_all(stdout, got);
                }
            }
        }
        if fds[1].revents != 0 {
            match unsafe { libc::read(stdin, buf.as_mut_ptr().cast(), buf.len()) } {
                n if n <= 0 => stdin_open = false,
                n => write_all(master.as_raw_fd(), &buf[..n as usize]),
            }
        }
    }
}

fn write_all(fd: i32, mut b: &[u8]) {
    while !b.is_empty() {
        let n = unsafe { libc::write(fd, b.as_ptr().cast(), b.len()) };
        if n <= 0 {
            // EINTR is worth retrying; anything else means the far end is
            // gone and there is nobody to tell.
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        b = &b[n as usize..];
    }
}

fn last() -> Errno {
    Errno::from_raw_os_error(unsafe { *libc::__errno_location() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn a_pty_pair_carries_bytes_and_the_slave_is_a_terminal() {
        let pty = open().expect("a pty");
        assert!(is_tty(pty.slave.as_raw_fd()), "the slave must be a tty");
        // The master is a tty too, but the property that matters is the
        // slave's: that is what the container sees on fd 0.
        set_size(&pty.master, 40, 100).expect("set size");
        assert_eq!(size_of(pty.slave.as_raw_fd()), Some((40, 100)));

        // A byte written to the master arrives at the slave. Written with
        // echo already off, or the master would read its own byte back.
        let mut t: libc::termios = unsafe { core::mem::zeroed() };
        unsafe {
            libc::tcgetattr(pty.slave.as_raw_fd(), &mut t);
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(pty.slave.as_raw_fd(), libc::TCSANOW, &t);
        }
        crate::sys::write(&pty.master, b"hello\n").unwrap();
        let mut f = std::fs::File::from(pty.slave.try_clone().unwrap());
        let mut buf = [0u8; 6];
        f.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello\n");
    }

    #[test]
    fn a_pipe_is_not_a_terminal() {
        // The whole reason `-t` exists: programs ask, and the answer
        // differs.
        let (r, _w) = crate::sys::pipe().unwrap();
        assert!(!is_tty(r.as_raw_fd()));
        assert_eq!(size_of(r.as_raw_fd()), None);
    }

    #[test]
    fn raw_mode_declines_politely_when_there_is_no_terminal() {
        // `run -t` with stdout redirected to a file must not fail; it just
        // has no terminal of its own to put into raw mode.
        let (r, _w) = crate::sys::pipe().unwrap();
        assert!(RawMode::enter(r.as_raw_fd()).is_none());
    }
}
