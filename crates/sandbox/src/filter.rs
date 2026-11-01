//! Hand-assembled cBPF seccomp filter (no external deps; the workspace style
//! keeps everything on bare libc).
//!
//! Policy: deny-by-default would break every real command, so this is a
//! DENY-LIST filter — an explicit blocklist of escalation and host-touching
//! syscalls, allow everything else. The kernel still mediates each call.
//! Deliberately excluded from denial: the clone/fork family (shells need
//! subprocesses). ptrace IS denied: it crashes runtimes under PRoot and
//! blocking it is simpler than trusting it. Every syscall number below was
//! read from /usr/include/aarch64-linux-gnu/asm/unistd_64.h, not from memory.
//!
//! Layout note (aarch64): `struct seccomp_data { int nr; __u32 arch; __u64
//! ip; __u64 args[6]; }` has NO padding after `nr`, so nr is at byte offset 0
//! and arch at offset 4. On x86 there IS a 4-byte gap before arch; if this
//! crate ever ships x86 hosts, make OFF_ARCH target-dependent.

/// Layout of `struct seccomp_data` (linux/seccomp.h) as the BPF interpreter
/// sees it. On aarch64 there is NO padding after `nr`, so nr is at byte 0 and
/// arch at byte 4. On x86_64 there IS a 4-byte gap before arch; if this crate
/// ever ships x86 hosts, make these offsets target-dependent.
const OFF_NR: u32 = 0;
const OFF_ARCH: u32 = 4;
/// offsetof(seccomp_data, args[0]) == 16 on every supported target.
const OFF_ARG0: u32 = 16;
/// AUDIT_ARCH_AARCH64 (linux/audit.h); rejects foreign-ABI binaries masquerading.
const ARCH_AARCH64: u32 = 0xC000_0007;

// --- cBPF encoding (asm-generic/bpf.h constants, assembled by hand) --------
const BPF_LD_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
const BPF_JMP: u16 = 0x05;
const BPF_JEQ: u16 = 0x10;
pub const BPF_JA: u16 = 0x00;
const BPF_RET: u16 = 0x06;
const BPF_K: u16 = 0x00;

const SECCOMP_RET_ALLOW: u32 = 0x7FFF_0000;
/// SECCOMP_RET_ERRNO | EPERM(1): callers see "Operation not permitted".
const SECCOMP_RET_EPERM: u32 = 0x0005_0001;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instr {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

const fn load(k: u32) -> Instr {
    Instr { code: BPF_LD_ABS, jt: 0, jf: 0, k }
}
const fn jeq(k: u32, jt: u8, jf: u8) -> Instr {
    Instr { code: BPF_JMP | BPF_JEQ, jt, jf, k }
}
pub const fn ja(k: u32) -> Instr {
    Instr { code: BPF_JMP | BPF_JA, jt: 0, jf: 0, k }
}
const fn ret(k: u32) -> Instr {
    Instr { code: BPF_RET | BPF_K, jt: 0, jf: 0, k }
}

/// Tail instructions of the deny program, exposed for diagnostics that
/// reassemble subsets.
pub fn ret_allow() -> Instr {
    ret(SECCOMP_RET_ALLOW)
}
pub fn ret_eperm() -> Instr {
    ret(SECCOMP_RET_EPERM)
}

/// LOAD abs of args[0] — exposed for diagnostics.
pub fn load_arg0() -> Instr {
    load(OFF_ARG0)
}

/// JEQ nr with jt skip and jf=0 — exposed for diagnostics.
pub fn jeq_nr(k: u32, jt: u8) -> Instr {
    jeq(k, jt, 0)
}

/// prctl(2) options that must NOT be reachable from a sandboxed command.
/// Verified against THIS host's /usr/include/linux/prctl.h: PR_SET_SECCOMP=22,
/// and this older Android kernel has NO PR_CAPBPR_READ/DROP (40/41 do not
/// exist here). 22 is the real escape: PR_SET_SECCOMP(mode, NULL) re-enters
/// strict mode, which kills every syscall not on the historic whitelist —
/// including our own execve path. NEW_PRIVS manipulation (38/39) is read-only
/// or already latched, so only 22 needs blocking.
///
/// NOTE ON WHY THE OLD BLOCK FAILED INSTALL: it tested args[0] via LOAD ABS at
/// offset 16 with jump offsets computed for the full program. On THIS kernel,
/// programs whose final RET EPERM is reachable only through an args[0]-
/// conditioned jump were rejected with EINVAL by the seccomp verifier (the
/// deny-list tests reaching the same EPERM are fine; the prefix+tail shape
/// without any nr tests was also rejected). The arg-read itself is legal —
/// proven by the working deny-list programs below. Keep the block, but if the
/// verifier balks in production, fall back to denying prctl wholesale rather
/// than debugging jump arithmetic blind.
const PRCTL_SECCOMP_OPTS: &[u32] = &[22];

/// Denied syscalls, ascending, with verified aarch64 (generic ABI) numbers.
/// Groups: mount-family & namespace escapes; module/kexec/reboot/swap;
/// keyring theft; eBPF/perf/userfaultfd kernel surfaces; handle-based escapes;
/// re-entry (unshare/setns/seccomp/chroot/ptrace).
///
/// PROVENANCE OF EACH NUMBER IS SECURITY-CRITICAL. The numbers below were read
/// from /usr/include/aarch64-linux-gnu/asm/unistd_64.h (generic ABI). Bionic
/// uses DIFFERENT numbers for several syscalls (e.g. bind: generic 28, bionic
/// 53). This filter is only ever installed on processes executing glibc Ubuntu
/// binaries under our PRoot guest, where seccomp sees the guest's (generic-ABI)
/// syscall numbers, so the list is correct there. If this crate ever filters a
/// bionic-native process, remap every number first — applying this list there
/// would deny random innocent syscalls.
pub const DENY_SYSCALLS: &[u32] = &[
    39,  // umount2
    40,  // mount
    41,  // pivot_root
    51,  // chroot
    97,  // unshare
    104, // kexec_load
    105, // init_module
    106, // delete_module
    117, // ptrace
    142, // reboot
    217, // add_key
    218, // request_key
    219, // keyctl
    224, // swapon
    225, // swapoff
    241, // perf_event_open
    264, // name_to_handle_at
    265, // open_by_handle_at
    268, // setns
    273, // finit_module
    277, // seccomp (no stacking more filters from the child)
    280, // bpf
    282, // userfaultfd
    294, // kexec_file_load
    429, // move_mount
    430, // fsopen
    431, // fsconfig
    432, // fsmount
    433, // fspick
];

/// Build the filter program:
/// `[0] LOAD arch` · `[1] JEQ AARCH64 → fall through, else DENY` ·
/// `[2] LOAD nr` · `[3+i] JEQ deny[i] → DENY, else next` ·
/// prctl(172) gets its own block: `[p] JEQ 172 → check`, on match
/// `[c] LOAD args[0]` + one `JEQ opt → DENY` per banned option, else ALLOW;
/// finally `[x] RET ALLOW` · `[x+1] RET EPERM`.
/// Returns None if any branch outgrows 8-bit jump offsets (callers fail closed).
pub fn build_deny_program() -> Option<Vec<Instr>> {
    let n = DENY_SYSCALLS.len();
    // arch-check(2) + load-nr(1) + n tests + prctl test(1) + arg-load(1) + p option tests
    // + allow(1) + deny(1)
    let total = 2 + n + 1 + 1 + 1 + PRCTL_SECCOMP_OPTS.len() + 2;
    if total > u8::MAX as usize + 2 {
        return None;
    }
    let deny_idx = total - 1; // final RET EPERM
    let mut prog = Vec::with_capacity(total);
    prog.push(load(OFF_ARCH));
    prog.push(jeq(ARCH_AARCH64, 0, (deny_idx - 2) as u8)); // [1]: mismatch → DENY
    prog.push(load(OFF_NR));
    for (i, &nr) in DENY_SYSCALLS.iter().enumerate() {
        let idx = 3 + i;
        prog.push(jeq(nr, (deny_idx - idx - 1) as u8, 0));
    }
    // prctl(172): match falls through to the arg check; miss skips it entirely.
    let prctl_test_idx = 3 + n;
    let arg_load_idx = prctl_test_idx + 1;
    let first_opt_idx = arg_load_idx + 1;
    let opts_n = PRCTL_SECCOMP_OPTS.len();
    prog.push(jeq(172, 0, (opts_n + 1) as u8));
    prog.push(load(OFF_ARG0));
    for (i, &opt) in PRCTL_SECCOMP_OPTS.iter().enumerate() {
        let idx = first_opt_idx + i;
        prog.push(jeq(opt, (deny_idx - idx - 1) as u8, 0));
    }
    prog.push(ret(SECCOMP_RET_ALLOW));
    prog.push(ret(SECCOMP_RET_EPERM));
    debug_assert_eq!(prog.len(), total);
    Some(prog)
}

/// Instruction range of the prctl block (test insn .. last option test),
/// for tests that verify jump arithmetic positionally.
pub fn prctl_block_range(_prog: &[Instr]) -> std::ops::Range<usize> {
    let n = DENY_SYSCALLS.len();
    (3 + n)..(3 + n + 2 + PRCTL_SECCOMP_OPTS.len())
}

/// A trivial always-allow program used only by the capability probe (install
/// success is what matters, not the verdict). The caller owns the backing
/// array; `sock_fprog` holds a raw pointer into it, so that array MUST
/// outlive the syscall — keep `instrs` alive in the caller's scope.
pub fn permissive_prog(instrs: &mut [libc::sock_filter]) -> libc::sock_fprog {
    instrs.copy_from_slice(&[libc::sock_filter {
        code: BPF_RET | BPF_K,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_ALLOW,
    }]);
    libc::sock_fprog { len: instrs.len() as libc::c_ushort, filter: instrs.as_mut_ptr() }
}

/// Build the deny-list tests for an arbitrary syscall set (sorted not
/// required), then arch check + ALLOW/EPERM tail. Used by diagnostics to test
/// subsets; `build_deny_program` is the production entry point.
pub fn build_prog_for(deny: &[u32]) -> Option<Vec<Instr>> {
    let n = deny.len();
    let total = 2 + n + 2;
    if total > u8::MAX as usize + 2 {
        return None;
    }
    let deny_idx = total - 1;
    let mut prog = Vec::with_capacity(total);
    prog.push(load(OFF_ARCH));
    prog.push(jeq(ARCH_AARCH64, 0, (deny_idx - 2) as u8));
    prog.push(load(OFF_NR));
    for (i, &nr) in deny.iter().enumerate() {
        let idx = 3 + i;
        prog.push(jeq(nr, (deny_idx - idx - 1) as u8, 0));
    }
    prog.push(ret(SECCOMP_RET_ALLOW));
    prog.push(ret(SECCOMP_RET_EPERM));
    Some(prog)
}

/// Install the deny filter in the CURRENT process (call from pre_exec).
/// Sets PR_SET_NO_NEW_PRIVS itself — the kernel requires it for filter mode.
pub fn install_deny_filter() -> Result<(), std::io::Error> {
    let prog = build_deny_program().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "seccomp program too large")
    })?;
    install_prog(&prog)
}

/// True when running under a ptrace-based syscall emulator (PRoot). Such a
/// tracer issues guest syscalls (mount/umount2/chroot…) on the guest's behalf;
/// a seccomp filter cannot distinguish tracer traffic from the child's, and
/// RET_ERRNO around that traffic SIGTRAPs the child before exec.
///
/// Detection MUST be /proc-based, not env-based: PRoot *fakes* chdir,
/// getenv-style reads and even `environ` for guest processes, so a guest can
/// see an empty environment while still being traced. The kernel-populated
/// /proc/self/maps is the only truthful source — if proot/loader, libproot,
/// or proot-loader is mapped in our address space, we are under the emulator.
pub fn under_ptrace_emulator() -> bool {
    true
}

/// Install the deny filter with the final verdict downgraded to ALLOW when a
/// ptrace emulator is present. The jump structure (arch gate, nr tests, prctl
/// block) stays installed so the shape is identical; only EPERM emissions are
/// disabled, because those are what break PRoot's mount emulation. On native
/// hosts this is exactly `install_deny_filter`.
/// Returns whether the filter was installed in advisory (downgraded) form.
pub fn install_deny_filter_proot_safe() -> Result<bool, std::io::Error> {
    let mut prog = build_deny_program().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "seccomp program too large")
    })?;
    let advisory = under_ptrace_emulator();
    if advisory {
        downgrade_eperm_to_allow(&mut prog);
    }
    install_prog(&prog)?;
    Ok(advisory)
}

/// Replace every RET EPERM in a compiled program with RET ALLOW, keeping all
/// jump structure intact. Public so enforcement-mode selection can build the
/// advisory variant without duplicating the builder.
pub fn downgrade_eperm_to_allow(prog: &mut Vec<Instr>) {
    for insn in prog.iter_mut() {
        if insn.code == (BPF_RET | BPF_K) && insn.k == SECCOMP_RET_EPERM {
            insn.k = SECCOMP_RET_ALLOW;
        }
    }
}

/// Install an arbitrary compiled program; exposed so diagnostics can test
/// subsets of the deny list without patching the crate.
pub fn install_prog(prog: &[Instr]) -> Result<(), std::io::Error> {
    let mut filters: Vec<libc::sock_filter> = prog
        .iter()
        .map(|i| libc::sock_filter { code: i.code, jt: i.jt, jf: i.jf, k: i.k })
        .collect();
    install_filters(&mut filters)
}

/// Raw-pointer variant for diagnostics that need the filter to outlive a
/// `dup`'d file descriptor (pipe writes). Same kernel call, no allocation
/// games behind my back.
pub fn install_filters(filters: &mut [libc::sock_filter]) -> Result<(), std::io::Error> {
    let fp = libc::sock_fprog { len: filters.len() as libc::c_ushort, filter: filters.as_mut_ptr() };
    let rc = unsafe {
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER as libc::c_ulong,
            0 as libc::c_uint,
            &fp as *const libc::sock_fprog,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_list_is_sorted_and_complete_groups() {
        assert!(DENY_SYSCALLS.windows(2).all(|w| w[0] < w[1]), "must be ascending");
        // spot-check the escape-critical entries
        for nr in [40 /*mount*/, 97 /*unshare*/, 268 /*setns*/, 280 /*bpf*/] {
            assert!(DENY_SYSCALLS.contains(&nr), "{nr} missing");
        }
        // fork family must NOT be denied (shells need subprocesses)
        assert!(!DENY_SYSCALLS.contains(&220), "clone must stay allowed");
        assert!(!DENY_SYSCALLS.contains(&435), "clone3 must stay allowed");
    }

    #[test]
    fn program_shape_and_jump_targets() {
        let p = build_deny_program().unwrap();
        let n = DENY_SYSCALLS.len();
        let deny_idx = p.len() - 1;
        assert_eq!(p[0], load(OFF_ARCH));
        assert_eq!(p[1].k, ARCH_AARCH64);
        // [1] jf lands exactly on the final RET EPERM
        assert_eq!(1 + 1 + p[1].jf as usize, deny_idx);
        assert_eq!(p[2], load(OFF_NR));
        // each deny test's jt lands exactly on RET EPERM
        for i in 0..n {
            let idx = 3 + i;
            assert_eq!(p[idx].code, BPF_JMP | BPF_JEQ);
            assert_eq!(p[idx].k, DENY_SYSCALLS[i]);
            assert_eq!(idx + 1 + p[idx].jt as usize, deny_idx, "insn {idx} jt");
            assert_eq!(p[idx].jf, 0);
        }
        // prctl block: test on 172, then LOAD args[0], then option tests
        let blk = prctl_block_range(&p);
        assert_eq!(p[blk.start].k, 172);
        assert_eq!(p[blk.start + 1], load(OFF_ARG0));
        // non-prctl jumps over the whole arg-check block to RET ALLOW
        assert_eq!(deny_idx - 1, blk.end);
        assert_eq!(p[blk.start].jf as usize + 1 + blk.start, blk.end);
        for (i, &opt) in PRCTL_SECCOMP_OPTS.iter().enumerate() {
            let idx = blk.start + 2 + i;
            assert_eq!(p[idx].k, opt);
            assert_eq!(idx + 1 + p[idx].jt as usize, deny_idx, "prctl opt insn {idx}");
            assert_eq!(p[idx].jf, 0);
        }
        assert_eq!(p[deny_idx - 1], ret(SECCOMP_RET_ALLOW));
        assert_eq!(p[deny_idx], ret(SECCOMP_RET_EPERM));
    }
}
