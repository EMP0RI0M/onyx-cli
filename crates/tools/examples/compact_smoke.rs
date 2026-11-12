use tools::compress;

fn main() {
    for (cmd, path) in [
        ("git status", "/tmp/gs_real.txt"),
        ("ls -la crates/tools/src", "/tmp/ls.txt"),
        ("grep -rn pub fn crates/tools/src", "/tmp/gr2.txt"),
    ] {
        let raw = std::fs::read_to_string(path).unwrap_or_default();
        let (out, s) = compress::compact(cmd, &raw);
        println!("{cmd}\n  before={} after={} -> {s:?}", raw.len(), out.len());
        if out.len() < raw.len() {
            for line in out.lines().take(8) { println!("  | {line}"); }
        }
        println!();
    }
}
