//! Diagnostic: fork → install a seccomp filter → exec `sh -c` directly.
//! Distinguishes "filter install breaks the child" from "a denied syscall
//! blocks shell startup".
//!
//! KEY INSIGHT DRIVING THIS MATRIX: the child's report pipe is inherited by
//! the exec'd shell, so a shell that starts prints its own output THROUGH the
//! same fd. That separates two very different failures:
//!   - "pre-exec-ok" only, killed by signal 5  → shell ran, then its FIRST
//!     syscall (execve itself) was trapped. The filter does not match our
//!     numbers at all — guest/tracer rewriting or a bionic-vs-generic ABI
//!     number mismatch.
//!   - "pre-exec-ok" + "child-ran" + exit 0    → filter is fine; an earlier
//!     failure came from writing to a pipe fd the filter had already closed
//!     (EPIPE after exec).

fn main() {
    println!("-- nnp only, then exec --");
    spawn(|_| Ok(()));

    println!("-- full deny list, then exec --");
    spawn(|_wfd| {
        let prog = sandbox::filter::build_deny_program().unwrap();
        sandbox::filter::install_prog(&prog)
    });

    // Production program with the final RET EPERM replaced by RET ALLOW: if
    // this survives, no jump TARGET is the problem and something about the
    // EPERM action itself (or PRoot's handling of it) kills the child.
    println!("-- full deny list, EPERM tail → ALLOW tail --");
    spawn(|_wfd| {
        let mut prog = sandbox::filter::build_deny_program().unwrap();
        let last = prog.len() - 1;
        prog[last] = sandbox::filter::ret_allow();
        sandbox::filter::install_prog(&prog)
    });

    // Single-test programs built FRESH (correct offsets for their length).
    // If jeq(39)/jeq(40) traps execve, our nr-0 numbering assumption fails.
    for nr in [39u32, 40, 172, 220] {
        println!("-- fresh single deny [{nr}] --");
        spawn(move |_wfd| {
            let prog = sandbox::filter::build_prog_for(&[nr]).unwrap();
            sandbox::filter::install_prog(&prog)
        });
    }

    // Minimal LOAD nr + ALLOW/EPERM, no jumps at all.
    println!("-- load-nr + ALLOW/EPERM only --");
    spawn(|_wfd| {
        let p = sandbox::filter::build_deny_program().unwrap();
        let prog = vec![p[0], p[1], p[2], sandbox::filter::ret_allow(), sandbox::filter::ret_eperm()];
        sandbox::filter::install_prog(&prog)
    });

    // Bisect: which group of 4 is toxic? Groups pass standalone but the full
    // list kills, so test CUMULATIVE prefixes to find where it flips.
    let mut acc: Vec<u32> = Vec::new();
    for chunk in sandbox::filter::DENY_SYSCALLS.chunks(4) {
        acc.extend_from_slice(chunk);
        let leaked: &'static [u32] = Box::leak(acc.clone().into_boxed_slice());
        println!("-- cumulative prefix {:?} --", leaked);
        spawn(move |_wfd| {
            let prog = sandbox::filter::build_prog_for(leaked).unwrap();
            sandbox::filter::install_prog(&prog)
        });
    }
}

/// Spawn a child that runs `pre_exec(wfd)` (after setting nnp), reports the
/// outcome through the pipe, closes the READ end only (keeping wfd open so
/// the exec'd shell can keep writing through it), then execs sh.
fn spawn(pre_exec: impl Fn(i32) -> Result<(), std::io::Error>) {
    unsafe {
        let mut pipefd = [0i32; 2];
        if libc::pipe(pipefd.as_mut_ptr()) != 0 {
            println!("pipe failed");
            return;
        }
        let (rfd, wfd) = (pipefd[0], pipefd[1]);
        let pid = libc::fork();
        if pid < 0 {
            println!("fork failed");
            return;
        }
        if pid == 0 {
            libc::close(rfd);
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                report(wfd, b"nnp-failed");
                libc::_exit(1);
            }
            if let Err(e) = pre_exec(wfd) {
                report(wfd, format!("pre-exec-failed: {e}").as_bytes());
                libc::_exit(1);
            }
            report(wfd, b"pre-exec-ok\n");
            // NOTE: do NOT close wfd here — the exec'd shell writes its
            // output through it. Earlier harnesses closed it and got EPIPE
            // silence that masqueraded as "the filter killed everything".
            let sh = b"/bin/sh\0".as_ptr() as *const libc::c_char;
            let script = b"echo child-ran; id -u\0";
            let args: [*const libc::c_char; 4] = [
                sh,
                b"-c\0".as_ptr() as *const libc::c_char,
                script.as_ptr() as *const libc::c_char,
                std::ptr::null(),
            ];
            let envp: [*const libc::c_char; 3] = [
                b"PATH=/usr/bin:/bin\0".as_ptr() as *const libc::c_char,
                b"HOME=/tmp\0".as_ptr() as *const libc::c_char,
                std::ptr::null(),
            ];
            libc::execve(sh, args.as_ptr(), envp.as_ptr());
            report(wfd, format!("execve-failed: {}", std::io::Error::last_os_error()).as_bytes());
            libc::_exit(2);
        }
        libc::close(wfd);
        let mut buf = [0u8; 256];
        let n = libc::read(rfd, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
        if n > 0 {
            print!("{}", String::from_utf8_lossy(&buf[..n as usize]));
        } else {
            println!("child said: (nothing)");
        }
        let mut status: i32 = 0;
        libc::waitpid(pid, &mut status, 0);
        libc::close(rfd);
        if libc::WIFEXITED(status) {
            println!("child exit = {}", libc::WEXITSTATUS(status));
        } else if libc::WIFSIGNALED(status) {
            println!("child killed by signal {}", libc::WTERMSIG(status));
        }
    }
}

fn report(fd: i32, msg: &[u8]) {
    unsafe { libc::write(fd, msg.as_ptr() as *const libc::c_void, msg.len()) };
}
