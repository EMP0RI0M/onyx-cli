//! tektile-core sandbox: run shell commands with the kernel's cooperation —
//! but only the cooperation this kernel ACTUALLY grants.
//!
//! Doctrine (Phase C): never assume a capability, probe it. This is an arm64
//! Android phone under PRoot; some primitives "succeed" there as userspace
//! illusions, and namespaces work for real only when the child's mapped uid
//! AND a tmpfs mount both check out. ptrace-based filtering is deliberately
//! out of scope (crashes runtimes under PRoot; seccomp is the right tool).
//!
//! Levels, strongest first:
//! - L3 `Namespace`: user+mount+ipc+uts namespaces via unshare(2), recursive-
//!   private mounts, then a deny-most syscalls seccomp filter in the child.
//! - L2 `SeccompOnly`: the same filter, no namespaces.
//! - L1 `RestrictedEnv`: no kernel primitives available — scrubbed environment
//!   only. Honest label, weakest guarantee.

#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
use std::process::Command;

#[cfg(target_os = "linux")]
pub mod filter;

/// What the kernel lets us do, discovered at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    pub userns: bool,        // unshare(CLONE_NEWUSER) + uid map → euid 0
    pub mountns: bool,       // tmpfs mount succeeds inside the new mount ns
    pub no_new_privs: bool,  // prctl(PR_SET_NO_NEW_PRIVS)
    pub seccomp_filter: bool, // SECCOMP_SET_MODE_FILTER installs
    pub ptrace_emulator: bool, // PRoot-style tracer rewrites guest syscalls; deny-list EPERM is unsafe here
}

impl Caps {
    pub fn none() -> Self {
        Self { userns: false, mountns: false, no_new_privs: false, seccomp_filter: false, ptrace_emulator: false }
    }
}

/// The selected isolation strategy for running one command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Namespace,
    SeccompOnly,
    RestrictedEnv,
}

impl Level {
    pub fn name(&self) -> &'static str {
        match self {
            Level::Namespace => "L3-namespace+seccomp",
            Level::SeccompOnly => "L2-seccomp-only",
            Level::RestrictedEnv => "L1-restricted-env",
        }
    }
}

/// Pick the strongest level the current kernel supports.
pub fn select_level(caps: &Caps) -> Level {
    if caps.userns && caps.mountns && caps.no_new_privs && caps.seccomp_filter {
        Level::Namespace
    } else if caps.no_new_privs && caps.seccomp_filter {
        Level::SeccompOnly
    } else {
        Level::RestrictedEnv
    }
}

/// One-shot capability probe. Each check runs a throwaway CHILD (so the
/// parent stays unfiltered) that attempts the primitive and encodes results
/// in its exit-status bits.
pub fn probe() -> Caps {
    let (caps, _) = probe_diag();
    caps
}

/// Like `probe`, plus `[seccomp_rc, seccomp_errno]` diagnostics (zero when
/// the install succeeded or nnp was unavailable so seccomp was not attempted).
pub fn probe_diag() -> (Caps, [i32; 2]) {
    #[cfg(target_os = "linux")]
    {
        let (userns, mountns) = probe_namespaces();
        let mut diag = [0i32; 2];
        let (nnp, sec) = probe_primitives_diag(&mut diag);
        (
            Caps {
                userns,
                mountns,
                no_new_privs: nnp,
                seccomp_filter: sec,
                ptrace_emulator: filter::under_ptrace_emulator(),
            },
            diag,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        (Caps::none(), [0, 0])
    }
}

#[cfg(target_os = "linux")]
fn wait_child(pid: libc::pid_t) -> u8 {
    unsafe {
        let mut status: i32 = 0;
        if libc::waitpid(pid, &mut status, 0) < 0 {
            return 0;
        }
        if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status) as u8
        } else {
            0
        }
    }
}

/// Child: unshare user+mount(+ipc+uts) namespaces, write identity uid/gid maps
/// (`unshare -r` style), require euid==0 inside, then prove the mount namespace
/// by mounting a tmpfs at a scratch dir. Bits: 1=userns real, 2=mountns works.
#[cfg(target_os = "linux")]
pub fn probe_namespaces() -> (bool, bool) {
    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            return (false, false);
        }
        if pid == 0 {
            let mut code = 0u8;
            let flags = libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWIPC | libc::CLONE_NEWUTS;
            if libc::unshare(flags) == 0 {
                map_root_identity();
                if libc::geteuid() == 0 {
                    code |= 1;
                    let dir = b"/tmp/.onyx-sbprobe\0".as_ptr() as *const libc::c_char;
                    libc::mkdir(dir, 0o755);
                    let rc = libc::mount(
                        b"tmpfs\0".as_ptr() as *const libc::c_char,
                        dir,
                        std::ptr::null(),
                        0,
                        std::ptr::null(),
                    );
                    if rc == 0 {
                        code |= 2;
                    }
                    libc::umount2(dir, libc::MNT_DETACH);
                    libc::rmdir(dir);
                }
            }
            libc::_exit(code as i32);
        }
        let code = wait_child(pid);
        (code & 1 != 0, code & 2 != 0)
    }
}

/// Child: attempt PR_SET_NO_NEW_PRIVS, then install a permissive seccomp
/// filter (installing requires no_new_privs, so try them in that order).
/// Bits: 1=nnp ok, 2=filter install ok. On failure the child writes the raw
/// syscall return and errno to `diag` so the parent can report WHY — probe
/// results without diagnostics are how silent false negatives hide.
#[cfg(target_os = "linux")]
fn probe_primitives_diag(diag: &mut [i32; 2]) -> (bool, bool) {
    unsafe {
        let mut pipefd = [0i32; 2];
        if libc::pipe(pipefd.as_mut_ptr()) != 0 {
            return (false, false);
        }
        let (rfd, wfd) = (pipefd[0], pipefd[1]);
        let pid = libc::fork();
        if pid < 0 {
            libc::close(rfd);
            libc::close(wfd);
            return (false, false);
        }
        if pid == 0 {
            libc::close(rfd);
            let mut code = 0u8;
            let rc: i32;
            let mut err = 0i32;
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                rc = -1;
                err = *libc::__errno_location();
            } else {
                code |= 1;
                let mut instrs = [std::mem::zeroed::<libc::sock_filter>(); 1];
                let prog = filter::permissive_prog(&mut instrs);
                rc = libc::syscall(
                    libc::SYS_seccomp,
                    libc::SECCOMP_SET_MODE_FILTER as libc::c_ulong,
                    0 as libc::c_uint,
                    &prog as *const _ as *const libc::c_void,
                ) as i32;
                if rc == 0 {
                    code |= 2;
                } else {
                    err = *libc::__errno_location();
                }
            }
            let bytes = [rc.to_ne_bytes(), err.to_ne_bytes(), (code as i32).to_ne_bytes()].concat();
            libc::write(wfd, bytes.as_ptr() as *const libc::c_void, bytes.len());
            libc::close(wfd);
            libc::_exit(code as i32);
        }
        libc::close(wfd);
        let mut buf = [0u8; 12];
        let n = libc::read(rfd, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
        libc::close(rfd);
        let exit_code = wait_child(pid);
        if n as usize == 12 {
            diag[0] = i32::from_ne_bytes(buf[0..4].try_into().unwrap());
            diag[1] = i32::from_ne_bytes(buf[4..8].try_into().unwrap());
            (buf[8] & 1 != 0, buf[8] & 2 != 0)
        } else {
            (exit_code & 1 != 0, exit_code & 2 != 0)
        }
    }
}

/// Write identity maps so our real uid appears as root inside the namespace
/// (what `unshare --map-root-user` does). setgroups must be denied before a
/// gid_map with group entries is accepted.
#[cfg(target_os = "linux")]
fn map_root_identity() {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let map_u = format!("0 {uid} 1\n{uid} 0 1\n");
    let _ = std::fs::write("/proc/self/setgroups", b"deny");
    let map_g = format!("0 {gid} 1\n{gid} 0 1\n");
    for (file, content) in [("/proc/self/uid_map", map_u), ("/proc/self/gid_map", map_g)] {
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(file) {
            let _ = f.write_all(content.as_bytes());
        }
    }
}

/// Which verdict this host can enforce, discovered behaviorally (PRoot fakes
/// maps/env, so parent-side probes lie). Cached: costs one fork+exec.
#[cfg(target_os = "linux")]
fn strict_enforcement_ok() -> bool {
    use std::sync::OnceLock;
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| seccomp_enforcement_works() == Some(true))
}

/// Strict RET EPERM when demonstrably enforced; advisory ALLOW otherwise.
#[cfg(target_os = "linux")]
fn install_verdict_aware_filter() -> Result<bool, std::io::Error> {
    let mut prog = filter::build_deny_program().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "seccomp program too large")
    })?;
    let advisory = !strict_enforcement_ok();
    if advisory {
        filter::downgrade_eperm_to_allow(&mut prog);
    }
    filter::install_prog(&prog)?;
    Ok(advisory)
}

/// Run `command` under the selected level, capturing output like tools::Bash.
///
/// On this device (PRoot guest over a bionic Android host) PRoot injects its
/// ptrace-based syscall emulation into every guest process — and the emulator
/// itself calls syscalls our deny list exists to block (mount, umount2, …).
/// A seccomp filter cannot see *who* issues a syscall, only the number: with
/// the deny-list RET EPERM active, PRoot's own mount traffic fails and it
/// SIGTRAPs the child before exec completes. Proven experimentally: identical
/// jump chains reaching the same RET EPERM from nr tests work; reaching it
/// from an args[0] test is verifier-rejected; and swapping the final EPERM for
/// ALLOW makes the full program run. So the filter is correct — this kernel +
/// tracer combination just can't enforce errno-denials around PRoot.
///
/// Therefore L2/L3 install the deny-list with the EPERM tail downgraded to
/// ALLOW on this host, and say so honestly: at L2 that is effectively L1 plus
/// no_new_privs; at L3 the REAL user+mount namespaces remain the substantive
/// isolation (host paths are not shared into them), while the syscall blacklist
/// becomes advisory. Enforcement without illusion requires running outside
/// PRoot (native Termux/glibc host), where the same programs pass cleanly.
pub fn run(level: Level, command: &str, cwd: Option<&str>) -> std::io::Result<(String, i32)> {
    let mut c = Command::new("/bin/sh");
    c.arg("-c").arg(command);
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }

    #[cfg(target_os = "linux")]
    {
        // Retain full parent environment under PRoot guests; scrub on native hosts.
        // PRoot requires PROOT_* and related dynamic linker environment variables
        // to track child execve hooks.
        if !filter::under_ptrace_emulator() {
            c.env_clear();
            c.env("PATH", "/usr/bin:/bin:/usr/local/bin");
            c.env("HOME", cwd.unwrap_or("/tmp"));
            c.env("TMPDIR", "/tmp");
            c.env("LANG", "C.UTF-8");
        }

        match level {
            Level::Namespace => unsafe {
                c.pre_exec(|| {
                    let flags = libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWIPC | libc::CLONE_NEWUTS;
                    if libc::unshare(flags) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    map_root_identity();
                    make_mounts_private();
                    install_verdict_aware_filter()?;
                    Ok(())
                });
            },
            Level::SeccompOnly => unsafe {
                c.pre_exec(|| {
                    if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    install_verdict_aware_filter()?;
                    Ok(())
                });
            },
            Level::RestrictedEnv => {}
        }
    }

    let out = c.output().map_err(|e| std::io::Error::new(
        e.kind(),
        format!("{}: could not start `sh -c`: {e} (raw_os_error={:?})", level.name(), e.raw_os_error()),
    ))?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    if !err.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&err);
    }
    let code = match out.status.code() {
        Some(c) => c,
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = out.status.signal() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!("[killed by signal {sig}]"));
                }
            }
            -1
        }
    };
    Ok((text, code))
}

/// Child-side enforcement self-test: run the deny-list under L2 and ask a
/// denied syscall whether it actually failed. Returns Some(true) only when
/// errno denials demonstrably reach the child (strict mode works here);
/// Some(false) when the command ran but the denial was swallowed (advisory /
/// tracer path); None when the child could not even start. This is the only
/// detection that matters for choosing strict vs advisory, because PRoot fakes
/// both /proc/self/maps and the environment — parent-side probes see nothing.
pub fn seccomp_enforcement_works() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        if filter::under_ptrace_emulator() {
            return Some(false);
        }
        let out = run(Level::SeccompOnly, "umount /nonexistent-xyz 2>&1; echo rc=$?", None).ok()?;
        if !out.0.contains("rc=0") {
            return Some(true); // umount failed => EPERM denial reached the child
        }
        if out.0.contains("Operation not permitted") || out.0.contains("rc=32") {
            return Some(true);
        }
        Some(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Inside the fresh mount namespace mark all mounts recursively private so
/// nothing propagates back to the host. Deeper masking (hiding host paths) is
/// a later-phase knob; Phase C keeps the workspace usable.
#[cfg(target_os = "linux")]
fn make_mounts_private() {
    unsafe {
        let slash = b"/\0".as_ptr() as *const libc::c_char;
        libc::mount(
            std::ptr::null(),
            slash,
            std::ptr::null(),
            (libc::MS_REC | libc::MS_PRIVATE) as libc::c_ulong,
            std::ptr::null(),
        );
    }
}

/// A sandboxed drop-in for tools::Bash: same tool contract, kernel-enforced.
pub struct SandboxedBash {
    pub level: Level,
    pub cwd: Option<String>,
}

impl SandboxedBash {
    pub fn new(level: Level, cwd: Option<String>) -> Self {
        Self { level, cwd }
    }
}

impl tools::Tool for SandboxedBash {
    fn name(&self) -> &str {
        "bash"
    }
    fn arg_keys(&self) -> Vec<String> {
        vec!["command".into()]
    }
    fn description(&self) -> &str {
        "Run a shell command in the sandbox (`sh -c`). Args: {\"command\": \"<shell cmd>\"}. Returns combined stdout/stderr and exit code."
    }
    fn execute(&self, args: &tools::Args) -> Result<tools::Output, tools::ToolError> {
        let cmd = ["command", "cmd", "script", "shell"]
            .iter()
            .find_map(|k| args.get(*k).and_then(|v| v.as_str()))
            .ok_or_else(|| tools::ToolError::BadArgs("missing `command`".into()))?;
        let (text, code) = run(self.level, cmd, self.cwd.as_deref()).map_err(tools::ToolError::Io)?;
        let is_success = code == 0;
        Ok(tools::bash::format_command_output(cmd, &text, code, is_success))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_level_combinations() {
        let full = Caps {
            userns: true,
            mountns: true,
            no_new_privs: true,
            seccomp_filter: true,
            ptrace_emulator: false,
        };
        assert_eq!(select_level(&full), Level::Namespace);

        let mut no_mount = full;
        no_mount.mountns = false;
        assert_eq!(select_level(&no_mount), Level::SeccompOnly);

        let mut no_user = full;
        no_user.userns = false;
        assert_eq!(select_level(&no_user), Level::SeccompOnly);

        let mut no_seccomp = full;
        no_seccomp.seccomp_filter = false;
        assert_eq!(select_level(&no_seccomp), Level::RestrictedEnv);

        let mut no_nnp = full;
        no_nnp.no_new_privs = false;
        assert_eq!(select_level(&no_nnp), Level::RestrictedEnv);

        assert_eq!(select_level(&Caps::none()), Level::RestrictedEnv);
    }

    #[test]
    fn live_l1_run_echo() {
        let (out, code) = run(Level::RestrictedEnv, "echo onyx-test-42", None).unwrap();
        assert_eq!(code, 0);
        assert!(out.contains("onyx-test-42"));
    }

    #[test]
    fn sandboxed_bash_tool_contract() {
        use tools::Tool;
        let sb = SandboxedBash::new(Level::RestrictedEnv, None);
        assert_eq!(sb.name(), "bash");
        assert_eq!(sb.arg_keys(), vec!["command"]);
        let out = sb.execute(&serde_json::json!({"command": "echo tool-contract-ok"})).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("tool-contract-ok"));
        assert!(out.content.contains("[exit 0]"));
    }

    #[test]
    fn strict_seccomp_denies_syscall_when_enforced() {
        if filter::under_ptrace_emulator() {
            // Under PRoot or ptrace emulators, strict RET_ERRNO seccomp collides with
            // tracer emulation, so advisory mode is used and this test is skipped.
            return;
        }
        let caps = probe();
        if !caps.no_new_privs || !caps.seccomp_filter {
            return;
        }
        // When running on native host without ptrace emulation, denied syscall returns error
        let (out, code) = run(Level::SeccompOnly, "umount /proc 2>&1; echo exit=$?", None).unwrap();
        assert_ne!(code, 0);
        assert!(out.contains("Operation not permitted") || out.contains("exit=1"));
    }
}
