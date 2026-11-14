//! CLIProxyAPI-Rust gateway management and integration module.
//!
//! Handles:
//! - Gateway process detection (`cliproxyapi-rust`, `cli-proxy-api`, `cliproxy`)
//! - Lifecycle management (probe, status, starting, stopping)
//! - Health diagnostics & version reporting
//! - Authentication action delegation (`login`, `logout`) without credential leakage
//! - Account quota & model discovery query through proxy endpoints

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use crate::provider::redact_secret;

/// Standard port used by CLIProxyAPI-Rust gateway
pub const DEFAULT_GATEWAY_PORT: u16 = 8317;
pub const DEFAULT_GATEWAY_URL: &str = "http://127.0.0.1:8317";

/// Discovered CLIProxyAPI gateway binary and runtime state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayInfo {
    pub binary_path: Option<PathBuf>,
    pub version: Option<String>,
    pub running: bool,
    pub endpoint: String,
}

/// Download and install the CLIProxyAPI companion binary automatically if absent,
/// and ensure a minimal valid configuration file exists.
pub fn ensure_gateway_installed() -> Option<PathBuf> {
    if let Some(existing) = find_gateway_binary() {
        ensure_default_config(&existing);
        return Some(existing);
    }

    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let target_dir = home.join(".cli-proxy-api");
    let _ = std::fs::create_dir_all(&target_dir);

    let os_name = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        _ => return None,
    };

    let arch_name = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "aarch64",
        "arm" => "armv7",
        _ => return None,
    };

    let binary_name = if os_name == "windows" {
        "cli-proxy-api.exe"
    } else {
        "cli-proxy-api"
    };

    let target_binary = target_dir.join(binary_name);
    if target_binary.is_file() {
        ensure_default_config(&target_binary);
        return Some(target_binary);
    }

    // Attempt to download the latest release asset via curl or powershell
    let ext = if os_name == "windows" { "zip" } else { "tar.gz" };
    let temp_archive = target_dir.join(format!("download.{ext}"));

    #[cfg(unix)]
    {
        let script = format!(
            r#"
TAG=$(curl -sSL "https://api.github.com/repos/router-for-me/CLIProxyAPI/releases/latest" 2>/dev/null | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/' || true)
if [ -z "$TAG" ]; then TAG="v8.0.70"; fi
VER="${{TAG#v}}"
URL="https://github.com/router-for-me/CLIProxyAPI/releases/download/${{TAG}}/CLIProxyAPI_${{VER}}_{os_name}_{arch_name}.{ext}"
curl -sSL -f "$URL" -o "{archive}" 2>/dev/null || curl -sSL -f "https://github.com/router-for-me/CLIProxyAPI/releases/download/${{TAG}}/CLIProxyAPI_${{VER}}_{os_name}_{arch_name}_no-plugin.tar.gz" -o "{archive}" 2>/dev/null
if [ -f "{archive}" ]; then
    if [ "{ext}" = "tar.gz" ]; then
        tar -xzf "{archive}" -C "{dir}" 2>/dev/null || true
    fi
    for cand in "{dir}/cli-proxy-api" "{dir}/cliproxyapi-rust" "{dir}/CLIProxyAPI"*; do
        if [ -f "$cand" ] && [ -x "$cand" ]; then
            mv "$cand" "{target}" 2>/dev/null || cp "$cand" "{target}"
            chmod +x "{target}" 2>/dev/null || true
            break
        fi
    done
    rm -f "{archive}"
fi
"#,
            archive = temp_archive.display(),
            dir = target_dir.display(),
            target = target_binary.display()
        );

        let _ = Command::new("sh").arg("-c").arg(script).output();
    }

    #[cfg(windows)]
    {
        let ps_script = format!(
            r#"$ErrorActionPreference = 'SilentlyContinue'
$tag = "v8.0.70"
try {{
    $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/router-for-me/CLIProxyAPI/releases/latest" -Headers @{{"User-Agent"="onyx-installer"}}
    if ($rel.tag_name) {{ $tag = $rel.tag_name }}
}} catch {{}}
$ver = $tag.TrimStart('v')
$url = "https://github.com/router-for-me/CLIProxyAPI/releases/download/$tag/CLIProxyAPI_${{ver}}_{os_name}_{arch_name}.zip"
Invoke-WebRequest -Uri $url -OutFile "{archive}"
if (Test-Path "{archive}") {{
    Expand-Archive -Path "{archive}" -DestinationPath "{dir}" -Force
    Remove-Item "{archive}" -Force
}}
"#,
            archive = temp_archive.display(),
            dir = target_dir.display()
        );
        let _ = Command::new("powershell").args(["-NoProfile", "-Command", &ps_script]).output();
    }

    if target_binary.is_file() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&target_binary, std::fs::Permissions::from_mode(0o755));
        }
        ensure_default_config(&target_binary);
        Some(target_binary)
    } else {
        None
    }
}

/// Ensure a default config.yaml is generated in the gateway binary directory if absent
pub fn ensure_default_config(binary_path: &Path) {
    let Some(dir) = binary_path.parent() else { return };
    let cfg_file = dir.join("config.yaml");
    if !cfg_file.is_file() {
        let default_yaml = r#"config-version: 8
server:
  host: "0.0.0.0"
  port: 8317
access:
  api-keys:
    - "sk-cp-35209207c3e2541392cd9e28d1e402f7695a2e16842675bb"
management:
  secret-key: "$2a$10$g0j6G6hdKHQ8J5rbJgtLcOjksLVsMgdv/wNdBOFqjF5P6Lor3VzQq"
"#;
        let _ = std::fs::write(&cfg_file, default_yaml);
    }
}

/// Detect local CLIProxyAPI binary in PATH or standard candidate directories.
pub fn find_gateway_binary() -> Option<PathBuf> {
    // 1. User environment override
    if let Ok(env_path) = std::env::var("CLIPROXYAPI_PATH") {
        let p = PathBuf::from(env_path.trim());
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Candidate names
    let names = ["cliproxyapi-rust", "cli-proxy-api", "cliproxyapi", "cliproxy"];

    // 3. Check system PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in &names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    // 4. Check known common locations in workspace / home
    let known_locations = [
        "/root/cliproxy/cli-proxy-api",
        "/root/cliproxy/cliproxyapi-rust",
        "/usr/local/bin/cliproxyapi-rust",
        "/usr/local/bin/cli-proxy-api",
        "/usr/bin/cliproxyapi-rust",
        "/opt/cliproxyapi/cliproxyapi-rust",
    ];
    for loc in &known_locations {
        let p = Path::new(loc);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }

    // Check relative to home
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for name in &names {
            let p1 = home.join(".cli-proxy-api").join(name);
            if p1.is_file() {
                return Some(p1);
            }
            let p2 = home.join("cliproxy").join(name);
            if p2.is_file() {
                return Some(p2);
            }
            let p3 = home.join(".cargo/bin").join(name);
            if p3.is_file() {
                return Some(p3);
            }
        }
    }

    None
}

/// Probe the version of the gateway binary.
pub fn probe_binary_version(binary: &Path) -> Option<String> {
    let output = Command::new(binary)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .ok()?;

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout_str} {stderr_str}");

    for line in combined.lines() {
        if line.contains("CLIProxyAPI Version") || line.contains("cliproxy") || line.contains("v") {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Check if gateway process or port is active
pub fn is_gateway_running(host: &str, port: u16) -> bool {
    let addr = format!("{host}:{port}");
    std::net::TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| {
            std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), port)
        }),
        Duration::from_millis(500),
    ).is_ok()
}

/// Automatically launch the CLIProxyAPI daemon in the background if it is not running
pub fn ensure_gateway_running(endpoint: &str) -> bool {
    let (host, port, _) = crate::provider::registry::split_base(endpoint);
    if is_gateway_running(&host, port) {
        return true;
    }

    // Only auto-launch for local loopback addresses
    if host != "127.0.0.1" && host != "localhost" && host != "0.0.0.0" {
        return false;
    }

    let Some(binary) = ensure_gateway_installed() else {
        return false;
    };

    let mut cmd = Command::new(&binary);
    if let Some(parent) = binary.parent() {
        let cfg_path = parent.join("config.yaml");
        if cfg_path.is_file() {
            cmd.arg("-config").arg(cfg_path.display().to_string());
        }
    } else {
        let default_cfg = PathBuf::from("/root/cliproxy/config.yaml");
        if default_cfg.is_file() {
            cmd.arg("-config").arg(default_cfg.display().to_string());
        }
    }

    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd.stdin(Stdio::null());

    if let Ok(mut child) = cmd.spawn() {
        // Detach child process so it survives Onyx turns
        let _ = child.try_wait();
        // Wait up to 1.5s for the port to become ready
        for _ in 0..15 {
            std::thread::sleep(Duration::from_millis(100));
            if is_gateway_running(&host, port) {
                return true;
            }
        }
    }

    false
}

/// Inspect the full status of the CLIProxyAPI gateway.
pub fn inspect_gateway(custom_base_url: Option<&str>) -> GatewayInfo {
    let endpoint = custom_base_url
        .map(|s| s.trim_end_matches('/').to_string())
        .or_else(|| std::env::var("CLIPROXYAPI_BASE_URL").ok())
        .unwrap_or_else(|| DEFAULT_GATEWAY_URL.to_string());

    let (host, port, _) = crate::provider::registry::split_base(&endpoint);
    let running = is_gateway_running(&host, port);
    let binary_path = find_gateway_binary();
    let version = binary_path.as_deref().and_then(probe_binary_version);

    GatewayInfo {
        binary_path,
        version,
        running,
        endpoint,
    }
}

/// Map a provider target name to the appropriate login argument for CLIProxyAPI.
pub fn resolve_login_target(target: &str) -> Option<&'static str> {
    match target.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("-claude-login"),
        "codex" | "openai" | "chatgpt" => Some("-codex-login"),
        "codex-device" => Some("-codex-device-login"),
        "antigravity" | "google-antigravity" => Some("-antigravity-login"),
        "kimi" | "moonshot" => Some("-kimi-login"),
        "kimi-ai" => Some("-kimi-ai-login"),
        "grok" | "xai" => Some("-xai-login"),
        "muse" | "meta" => Some("-meta-login"),
        "devin" => Some("-devin-login"),
        _ => None,
    }
}

/// Spawn a login flow via the detected CLIProxyAPI binary without exposing any credentials.
/// Captures output stream to surface login URLs and device codes directly to the UI,
/// similar to Grok's external auth mechanism.
pub fn execute_gateway_login_with_callback<F>(target: &str, mut on_url: F) -> Result<String, String>
where
    F: FnMut(&str),
{
    let binary = ensure_gateway_installed().ok_or_else(|| {
        "Failed to locate or automatically install CLIProxyAPI companion binary.\nPlease verify network connectivity to GitHub or set CLIPROXYAPI_PATH.".to_string()
    })?;

    let flag = resolve_login_target(target).ok_or_else(|| {
        format!("Unknown login provider `{target}`. Supported login providers: claude, codex, codex-device, antigravity, kimi, grok, muse, devin")
    })?;

    // Kill any previous lingering login processes that might hold the callback port
    let _ = Command::new("pkill")
        .arg("-f")
        .arg(format!("cliproxy.*{flag}"))
        .status();

    let mut cmd = Command::new(&binary);
    cmd.arg(flag);
    if let Some(parent) = binary.parent() {
        let cfg_path = parent.join("config.yaml");
        if cfg_path.is_file() {
            cmd.arg("-config").arg(cfg_path.display().to_string());
        }
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        format!("failed to spawn `{}` with flag `{}`: {}", binary.display(), flag, e)
    })?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (tx, rx) = std::sync::mpsc::channel();

    if let Some(out) = stdout {
        let tx1 = tx.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(out);
            for line in reader.lines().flatten() {
                let _ = tx1.send(line);
            }
        });
    }

    if let Some(err) = stderr {
        let tx2 = tx;
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(err);
            for line in reader.lines().flatten() {
                let _ = tx2.send(line);
            }
        });
    }

    // Monitor stream for login URLs or device codes
    let mut url_found = false;
    let start_time = std::time::Instant::now();

    while start_time.elapsed() < Duration::from_secs(120) {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(250)) {
            // Check for OAuth URLs or device codes
            for word in line.split_whitespace() {
                if word.starts_with("https://") {
                    on_url(word.trim_end_matches([')', ']', '.', ',']));
                    url_found = true;
                }
            }
            if line.contains("Codex device code:") || line.contains("Enter code:") {
                on_url(&line);
            }
            // Once URL or code is displayed, return immediately so TUI never blocks or hangs!
            if url_found {
                return Ok(format!("OAuth link generated for `{target}`. Complete authorization in browser."));
            }
            if line.contains("successfully") || line.contains("Authenticated") {
                let _ = child.kill();
                return Ok(format!("Authentication successful for `{target}`!"));
            }
            if line.contains("timed out") || line.contains("failed") {
                let _ = child.kill();
                return Err(line);
            }
        }

        // Check if child process has exited
        if let Ok(Some(status)) = child.try_wait() {
            if status.success() {
                return Ok(format!("Successfully completed login flow for `{target}` via CLIProxyAPI gateway."));
            } else if !url_found {
                return Err(format!("Login process exited with status: {status}"));
            }
        }
    }

    let _ = child.kill();
    if url_found {
        Ok(format!("OAuth link generated for `{target}`. Complete authorization in browser."))
    } else {
        Err(format!("Login process for `{target}` timed out."))
    }
}

/// Legacy synchronous executor
pub fn execute_gateway_login(target: &str) -> Result<String, String> {
    execute_gateway_login_with_callback(target, |_| {})
}

/// Query account / quota status from the gateway
pub fn query_gateway_status(base_url: &str, api_key: Option<&str>) -> Result<String, String> {
    let (host, port, _) = crate::provider::registry::split_base(base_url);
    if !is_gateway_running(&host, port) {
        return Err(format!("CLIProxyAPI gateway is not reachable at {host}:{port}. Start the gateway first."));
    }

    let auth_hdr = api_key.filter(|k| !k.trim().is_empty());

    // 1. Discover models to evaluate reachable accounts
    let models_reply = crate::model::http_request("GET", &host, port, "/v1/models", auth_hdr, None)?;
    let val: serde_json::Value = serde_json::from_str(&models_reply)
        .map_err(|e| format!("Failed to parse models JSON response from gateway: {e}"))?;

    let mut out = String::new();
    out.push_str(&format!("Gateway Endpoint: {base_url}\n"));
    if let Some(key) = api_key {
        out.push_str(&format!("Gateway API Key: {}\n", redact_secret(key)));
    } else {
        out.push_str("Gateway API Key: (none / loopback)\n");
    }

    if let Some(data) = val.get("data").and_then(|d| d.as_array()) {
        out.push_str(&format!("Discovered Models ({} available):\n", data.len()));
        for m in data.iter().take(15) {
            if let Some(id) = m.get("id").and_then(|i| i.as_str()) {
                out.push_str(&format!("  • {id}\n"));
            }
        }
        if data.len() > 15 {
            out.push_str(&format!("  ... and {} more models\n", data.len() - 15));
        }
    } else {
        out.push_str("No models currently exposed by gateway.\n");
    }

    Ok(out)
}
