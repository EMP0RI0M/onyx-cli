//! Live capability probe: prints what this kernel actually grants, the
//! selected level, then demonstrates a sandboxed run and a denied syscall.

fn main() {
    let (caps, diag) = sandbox::probe_diag();
    println!("caps  = {caps:?}");
    if !caps.seccomp_filter {
        println!("diag  = seccomp rc={} errno={} ({})", diag[0], diag[1], strerror(diag[1]));
    }
    let level = sandbox::select_level(&caps);
    println!("level = {}", level.name());

    for (name, lvl) in [
        ("L1", sandbox::Level::RestrictedEnv),
        ("L2", sandbox::Level::SeccompOnly),
        ("L3", sandbox::Level::Namespace),
    ] {
        match sandbox::run(lvl, "echo hi; id -u", None) {
            Ok((out, code)) => println!("{name} run rc={code}: {:?}", out.trim()),
            Err(e) => println!("{name} run ERR: {e}"),
        }
        match sandbox::run(lvl, "umount /proc 2>&1; echo exit=$?", None) {
            Ok((out, code)) => println!("{name} deny-test rc={code}: {:?}", out.trim()),
            Err(e) => println!("{name} deny-test ERR: {e}"),
        }
    }
}

fn strerror(errno: i32) -> &'static str {
    match errno {
        1 => "EPERM",
        9 => "EBADF",
        12 => "ENOMEM",
        14 => "EFAULT",
        22 => "EINVAL",
        _ => "unknown",
    }
}
