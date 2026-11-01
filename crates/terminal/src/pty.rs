//! PTY plumbing shared by interactive mode and the WebSocket server.
//! Uses libc directly (no pty crate) — openpty → fork → setsid → TIOCSCTTY → execvp.

use std::ffi::CString;
use std::io::{Error, Result};
use std::os::raw::{c_char, c_int, c_ulong, c_void};
use std::ptr;

// Linux ioctl request codes (ARM/generic — same numeric values as x86 Linux).
const TIOCSCTTY: c_ulong = 0x540E;
const TIOCSWINSZ: c_ulong = 0x5414;

pub fn errno() -> c_int {
    unsafe { *libc::__errno_location() }
}

/// Set the pty window size (rows/cols). Mirrors ws_bridge.set_winsize.
pub fn set_winsize(fd: c_int, rows: u16, cols: u16) {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        libc::ioctl(fd, TIOCSWINSZ, &ws as *const libc::winsize as c_ulong);
    }
}

/// Low-level: fork a child attached to a fresh pty slave, exec `argv` with
/// a curated env (TERM/LANG/HOME), return (master_fd, child_pid) to the parent.
pub fn spawn_argv(argv: &[String]) -> Result<(c_int, c_int)> {
    let mut master: c_int = -1;
    let mut slave: c_int = -1;
    let rc =
        unsafe { libc::openpty(&mut master, &mut slave, ptr::null_mut(), ptr::null(), ptr::null()) };
    if rc != 0 {
        return Err(Error::last_os_error());
    }

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let e = Error::last_os_error();
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
        return Err(e);
    }

    if pid == 0 {
        // ---- child ----
        unsafe {
            libc::close(master);
            libc::setsid();
            libc::ioctl(slave, TIOCSCTTY, 0 as c_int);
            libc::dup2(slave, 0);
            libc::dup2(slave, 1);
            libc::dup2(slave, 2);
            if slave > 2 {
                libc::close(slave);
            }
        }
        // Curated env so TUIs render correctly inside the pty.
        std::env::set_var("TERM", "xterm-256color");
        std::env::set_var("LANG", "C.UTF-8");
        if let Ok(home) = std::env::var("HOME") {
            std::env::set_var("HOME", home);
        }
        let cargs: Vec<CString> = argv.iter().map(|s| CString::new(s.as_str()).unwrap()).collect();
        let mut ptrs: Vec<*const c_char> = cargs.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        unsafe {
            libc::execvp(ptrs[0], ptrs.as_mut_ptr());
            libc::_exit(127);
        }
    }

    // ---- parent ----
    unsafe { libc::close(slave) };
    Ok((master, pid))
}

/// Non-blocking mode for draining the master without a dedicated thread.
pub fn set_nonblock(fd: c_int) {
    let fl = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    unsafe { libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK) };
}

/// Put a terminal fd into raw mode; restoring on drop.
pub struct RawStdin {
    saved: libc::termios,
}

impl RawStdin {
    pub fn new(fd: c_int) -> Result<Self> {
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
            return Err(Error::last_os_error());
        }
        let mut raw = saved;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(Error::last_os_error());
        }
        Ok(Self { saved })
    }
}

impl Drop for RawStdin {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.saved);
        }
    }
}

/// Write all of `buf` to `fd`, tolerating short writes. Best-effort for pty/pipe.
pub fn write_all(fd: c_int, buf: &[u8]) {
    let mut off = 0usize;
    while off < buf.len() {
        let n = unsafe {
            libc::write(fd, buf.as_ptr().add(off) as *const c_void, buf.len() - off)
        };
        if n <= 0 {
            return;
        }
        off += n as usize;
    }
}
