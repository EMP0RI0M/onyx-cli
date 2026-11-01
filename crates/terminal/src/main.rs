//! tektile-term — a Rust PTY terminal + WebSocket bridge, built as a HYBRID
//! binary that also links the hand-written AArch64 assembly math kernel.
//!
//!   tektile-term                        -> interactive pty shell ($SHELL)
//!   tektile-term <shell>                -> interactive pty shell (given path)
//!   tektile-term --serve [<bind>] [-- cmd...] -> WebSocket PTY server (ws_bridge drop-in)
//!   tektile-term --kernel-check         -> run the asm kernel, prove it's linked & correct
//!
//! The assembly is the compute engine; Rust owns process/pty/terminal control.

mod pty;
mod serve;

use pty::RawStdin;
use std::env;
use std::io::Result;
use std::os::raw::c_int;

// ── Kernel self-check: prove the hand-written asm dot product is linked + correct. ──
fn kernel_check() -> i32 {
    // Fixed case (len multiple of 16) exercises only the NEON bulk loop.
    let a = vec![3.0f32; 64];
    let b = vec![4.0f32; 64];
    let fixed = kernel::dot(&a, &b);
    if fixed != 768.0 {
        eprintln!("kernel: FAIL fixed case dot={fixed}");
        return 1;
    }

    // Large mixed case (sin·cos) vs the portable reference.
    let n = 1usize << 20;
    let x: Vec<f32> = (0..n).map(|i| (i as f32).sin()).collect();
    let y: Vec<f32> = (0..n).map(|i| (i as f32).cos()).collect();
    let d = kernel::dot(&x, &y);
    let r = kernel::dot_ref(&x, &y);
    let rel = (d - r).abs() / r.abs().max(1e-9);
    println!("kernel: AArch64 asm dot linked OK (n={n}) asm={d:.6} ref={r:.6} rel={rel:.2e}");
    if rel < 1e-3 {
        0
    } else {
        eprintln!("kernel: FAIL rel={rel}");
        1
    }
}

// ── Interactive shell: spawn a child on a slave pty, pump bytes both ways. ──────
fn run_shell(shell: &str) -> Result<()> {
    let (master, pid) = pty::spawn_argv(&[shell.to_string()])?;

    // Raw mode for our real terminal; if stdin isn't a tty this fails, which is
    // fine (piped mode) — we continue line-buffered.
    let _raw = RawStdin::new(0).ok();

    let mut buf = [0u8; 4096];
    let mut stdin_open = true;

    loop {
        let mut pfds: [libc::pollfd; 2] = [
            libc::pollfd {
                fd: if stdin_open { 0 } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd { fd: master, events: libc::POLLIN | libc::POLLHUP, revents: 0 },
        ];
        let pr = unsafe { libc::poll(pfds.as_mut_ptr(), 2, -1) };
        if pr < 0 {
            if pty::errno() == libc::EINTR {
                continue;
            }
            break;
        }

        // stdin -> master
        if stdin_open && pfds[0].revents & libc::POLLIN != 0 {
            let n = unsafe {
                libc::read(0, buf.as_mut_ptr() as *mut std::os::raw::c_void, buf.len())
            };
            if n <= 0 {
                // stdin EOF (piped/redirected): stop polling stdin but KEEP draining
                // the master so the shell's final output still reaches stdout.
                stdin_open = false;
            } else {
                pty::write_all(master, &buf[..n as usize]);
            }
        }

        // master -> stdout
        if pfds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            let n =
                unsafe { libc::read(master, buf.as_mut_ptr() as *mut std::os::raw::c_void, buf.len()) };
            if n <= 0 {
                break; // child exited, slave fds closed -> master EOF/EIO
            }
            pty::write_all(1, &buf[..n as usize]);
        }
    }

    let mut status: c_int = 0;
    unsafe {
        libc::waitpid(pid, &mut status, 0);
        libc::close(master);
    }
    Ok(())
}

fn usage() {
    println!(
        "usage:\n  \
         tektile-term [shell]                     interactive pty shell\n  \
         tektile-term --serve [bind] [-- cmd...]  WebSocket PTY server (ws_bridge drop-in)\n  \
         tektile-term --kernel-check              verify the asm math kernel"
    );
}

fn main() {
    let argv: Vec<String> = env::args().collect();
    let mut i = 1;

    // --serve may appear anywhere in the leading flag position.
    if let Some(pos) = argv.iter().position(|a| a == "--kernel-check") {
        let _ = pos;
        std::process::exit(kernel_check());
    }

    if argv.get(i).map(String::as_str) == Some("--serve") {
        i += 1;
        let mut bind = "0.0.0.0:8765".to_string();
        // If the next token isn't `--` and looks like a bind (contains ':' or is host:port), take it.
        if let Some(a) = argv.get(i) {
            if a != "--" {
                bind = a.clone();
                i += 1;
            }
        }
        // Optional `-- cmd args...`
        if argv.get(i).map(String::as_str) == Some("--") {
            i += 1;
        }
        let mut command: Vec<String> = argv[i..].to_vec();
        if command.is_empty() {
            command = vec!["bash".to_string(), "-l".to_string()];
        }
        serve::run(serve::ServeOpts {
            bind,
            command,
            rows: 40,
            cols: 120,
        });
        return;
    }

    if argv.get(i).map(String::as_str).is_some_and(|a| a == "-h" || a == "--help") {
        usage();
        return;
    }

    let shell = argv
        .get(i)
        .cloned()
        .or_else(|| env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/sh".to_string());

    if let Err(e) = run_shell(&shell) {
        eprintln!("tektile-term: pty shell: {e}");
        std::process::exit(1);
    }
}
