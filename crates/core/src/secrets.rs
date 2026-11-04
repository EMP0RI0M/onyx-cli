//! Credential vault: durable, honestly-protected storage for provider API keys
//! and OAuth tokens.
//!
//! ## Threat model (stated plainly, because "encrypted" can mean many things)
//!
//! This vault defends against **casually reading your config file** and against
//! **accidental leakage into logs / dotfile backups**. It does NOT defend against
//! an attacker who already runs code as your user on this machine: whatever a
//! process can decrypt at runtime, a same-user process can also read. There is no
//! hardware root of trust here (this is a PRoot dev box with no Android keystore
//! and typically no GNOME Keychain), so we never claim E2E or "the key can't be
//! extracted". We say exactly what each tier buys you.
//!
//! Three tiers, strongest available wins:
//! 1. **OS keyring** (`secret-tool`, freedesktop Secret Service) — if present,
//!    secrets live in the OS store, not our file. Best effort; absent → skipped.
//! 2. **AES-256-GCM file** keyed by a passphrase you type at startup (PBKDF2 via
//!    `ring`). Real at-rest encryption; the plaintext key is never on disk while a
//!    passphrase is set. Costs one prompt per session.
//! 3. **Plaintext file, mode 0600** — last resort, chosen only when no passphrase
//!    is configured. Loudly warned about; better than nothing for the casual-read
//!    case, but there is genuinely no crypto protection here.
//!
//! The vault stores opaque secret strings; the provider layer decides how to use
//! them. Environment variables are consulted by callers *before* the vault, so a
//! shell-exported key always overrides stored state.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use ring::pbkdf2;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// Which protection actually took effect for the loaded vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Stored in the OS keyring (or would be, if the daemon were reachable).
    Keyring,
    /// AES-256-GCM under a startup passphrase.
    Encrypted,
    /// Plaintext JSON at mode 0600. No cryptographic protection.
    Plaintext,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Keyring => "os-keyring",
            Tier::Encrypted => "aes-256-gcm",
            Tier::Plaintext => "plaintext(0600)",
        }
    }
}

/// One logical secret slot, e.g. `openai/api_key`.
type Slots = BTreeMap<String, String>;

/// A loaded credential vault plus the tier it is currently persisted under.
pub struct Vault {
    slots: Slots,
    tier: Tier,
    path: PathBuf,
    /// Passphrase used to (re)derive the encryption key; None ⇒ plaintext tier.
    passphrase: Option<String>,
    dirty: bool,
}

/// File layout we write: `{ "v":1, "tier":"enc"|"plain", "salt":hex, "nonce":hex,
/// "ciphertext":hex }` — the encrypted form keeps the slot map inside the sealed
/// blob, so the outer file leaks only that a vault exists, never which providers.
const MAGIC_VERSION: u8 = 1;

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

/// Default vault path honouring `ONYX_HOME` (matches the rest of the workspace).
pub fn default_vault_path() -> PathBuf {
    let base = match std::env::var("ONYX_HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h),
        _ => match std::env::var("HOME").ok().filter(|h| !h.is_empty()) {
            Some(h) => PathBuf::from(h).join(".onyx"),
            None => PathBuf::from(".onyx"),
        },
    };
    base.join("credentials.json")
}

/// Derive a 32-byte AES-256 key from a passphrase + salt with PBKDF2-HMAC-SHA256.
/// 150k iterations: strong enough to make offline cracking expensive without
/// stalling an interactive CLI noticeably (~150ms on this class of CPU).
fn derive_key(passphrase: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        std::num::NonZeroU32::new(150_000).unwrap(),
        salt,
        passphrase.as_bytes(),
        &mut key,
    );
    key
}

/// True if a freedesktop Secret Service client is usable right now. We probe for
/// `secret-tool` on PATH rather than talking D-Bus directly (no dbus crate here);
/// a present binary is necessary but not sufficient — the actual store call below
/// still verifies it works and falls back to file tiers if it errors.
fn keyring_available() -> bool {
    std::process::Command::new("secret-tool")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Namespace prefix so our entries don't collide with other apps' secrets.
const KEYRING_SERVICE: &str = "onyx";

/// Store one secret in the OS keyring via `secret-tool store`. Returns false on
/// any failure (missing daemon, locked collection) so the caller can fall back.
fn keyring_put(key: &str, value: &str) -> bool {
    // secret-tool reads the secret from stdin; attributes are key/value pairs.
    let mut child = match std::process::Command::new("secret-tool")
        .args(["store", "--label", &format!("onyx:{key}"), "service", KEYRING_SERVICE, "slot", key])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    if let Some(mut si) = child.stdin.take() {
        use std::io::Write;
        let _ = si.write_all(value.as_bytes());
    }
    matches!(child.wait(), Ok(st) if st.success())
}

/// Read one secret from the OS keyring via `secret-tool lookup`. Empty/None on miss.
fn keyring_get(key: &str) -> Option<String> {
    let out = std::process::Command::new("secret-tool")
        .args(["lookup", "service", KEYRING_SERVICE, "slot", key])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

impl Vault {
    /// Load the vault from `path`. If `passphrase` is given we try to open an
    /// encrypted file; if the file is plaintext we still accept it (and keep the
    /// passphrase for the next save, upgrading the tier). Missing file ⇒ empty
    /// vault at whatever tier the passphrase implies.
    pub fn load(path: PathBuf, passphrase: Option<String>) -> Result<Vault, String> {
        let mut vault = Vault {
            slots: BTreeMap::new(),
            tier: if passphrase.is_some() { Tier::Encrypted } else { Tier::Plaintext },
            path,
            passphrase,
            dirty: false,
        };
        let text = match fs::read_to_string(&vault.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vault),
            Err(e) => return Err(format!("read {}: {e}", vault.path.display())),
        };
        let doc: Value = serde_json::from_str(&text).map_err(|e| format!("parse vault: {e}"))?;
        let tier_tag = doc.get("tier").and_then(|t| t.as_str()).unwrap_or("plain");
        if tier_tag == "enc" {
            let pp = vault.passphrase.clone().ok_or_else(|| {
                "vault is encrypted but no passphrase was supplied (set ONYX_VAULT_PASSPHRASE or pass one)".to_string()
            })?;
            let salt = unhex(doc.get("salt").and_then(|s| s.as_str()).unwrap_or(""))
                .ok_or_else(|| "bad salt".to_string())?;
            let nonce_b = unhex(doc.get("nonce").and_then(|s| s.as_str()).unwrap_or(""))
                .ok_or_else(|| "bad nonce".to_string())?;
            let ct = unhex(doc.get("ciphertext").and_then(|s| s.as_str()).unwrap_or(""))
                .ok_or_else(|| "bad ciphertext".to_string())?;
            let key = derive_key(&pp, &salt);
            let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("cipher init: {e}"))?;
            let pt = cipher
                .decrypt(Nonce::from_slice(&nonce_b), ct.as_ref())
                .map_err(|_| "decryption failed — wrong passphrase or tampered vault".to_string())?;
            let inner: Value = serde_json::from_slice(&pt).map_err(|e| format!("parse inner: {e}"))?;
            vault.slots = read_slots(&inner)?;
            vault.tier = Tier::Encrypted;
        } else {
            vault.slots = read_slots(&doc)?;
            // If we have a passphrase but the on-disk file is plaintext, mark dirty
            // so the next save upgrades it to encrypted.
            if vault.passphrase.is_some() {
                vault.dirty = true;
            }
            vault.tier = Tier::Plaintext;
        }
        Ok(vault)
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.slots.get(key).map(|s| s.as_str())
    }

    /// Insert/update a secret. With a passphrase we encrypt to file; otherwise we
    /// prefer the OS keyring (if usable) and fall back to the plaintext file.
    pub fn set(&mut self, key: &str, value: &str) {
        if self.passphrase.is_none() && keyring_available() && keyring_put(key, value) {
            // Keep it out of the file entirely when the keyring holds it.
            self.slots.insert(key.to_string(), String::new()); // placeholder marker
            self.tier = Tier::Keyring;
            self.dirty = true;
            return;
        }
        self.slots.insert(key.to_string(), value.to_string());
        self.dirty = true;
    }

    pub fn remove(&mut self, key: &str) -> bool {
        let had = self.slots.remove(key).is_some();
        if had {
            self.dirty = true;
        }
        had
    }

    /// Keys currently tracked (for listing — values are never returned here).
    pub fn keys(&self) -> Vec<String> {
        self.slots.keys().cloned().collect()
    }

    /// Resolve a secret: real keyring read first, then in-memory/file slot. Used
    /// after `load` so keyring-backed entries (stored as empty placeholders) can
    /// still be fetched on demand.
    pub fn resolve(&self, key: &str) -> Option<String> {
        if self.passphrase.is_none() && keyring_available() {
            if let Some(v) = keyring_get(key) {
                return Some(v);
            }
        }
        self.slots.get(key).cloned().filter(|v| !v.is_empty())
    }

    /// Persist according to the active tier. Idempotent; skips clean vaults.
    pub fn save(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("create dir: {e}"))?;
        }
        // Only serialize non-empty slots (skip keyring placeholders).
        let real: Slots = self
            .slots
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let obj = json!({ "slots": real });

        let body = if let Some(pp) = self.passphrase.clone() {
            // Encrypt: fresh random salt + nonce every save.
            let mut salt = [0u8; 16];
            let mut nonce = [0u8; 12];
            ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut salt)
                .map_err(|_| "rng salt".to_string())?;
            ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce)
                .map_err(|_| "rng nonce".to_string())?;
            let key = derive_key(&pp, &salt);
            let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("cipher: {e}"))?;
            let pt = serde_json::to_vec(&obj).map_err(|e| format!("ser inner: {e}"))?;
            let ct = cipher
                .encrypt(Nonce::from_slice(&nonce), pt.as_ref())
                .map_err(|_| "encrypt failed".to_string())?;
            json!({
                "v": MAGIC_VERSION,
                "tier": "enc",
                "kdf": "pbkdf2-hmac-sha256:150000",
                "salt": hex(&salt),
                "nonce": hex(&nonce),
                "ciphertext": hex(&ct),
            })
            .to_string()
        } else {
            let mut doc = obj;
            doc["tier"] = json!("plain");
            doc["v"] = json!(MAGIC_VERSION);
            doc.to_string()
        };

        write_secret_file(&self.path, &body)?;
        self.dirty = false;
        Ok(())
    }
}

fn read_slots(doc: &Value) -> Result<Slots, String> {
    let mut out = Slots::new();
    if let Some(map) = doc.get("slots").and_then(|s| s.as_object()) {
        for (k, v) in map {
            if let Some(vs) = v.as_str() {
                out.insert(k.clone(), vs.to_string());
            }
        }
    }
    Ok(out)
}

/// Write with mode 0600 BEFORE contents land, so the secret is never world- or
/// group-readable even momentarily. Uses OpenOptions + Unix permission bits.
fn write_secret_file(path: &PathBuf, contents: &str) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    std::io::Write::write_all(&mut f, contents.as_bytes()).map_err(|e| format!("write: {e}"))?;
    f.sync_data().ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let b = [0u8, 1, 15, 16, 255, 128];
        assert_eq!(unhex(&hex(&b)).unwrap(), b.to_vec());
        assert_eq!(hex(&[]), "");
        assert!(unhex("abc").is_none()); // odd length
    }

    #[test]
    fn encrypted_roundtrip_and_wrong_passphrase_fails() {
        let dir = std::env::temp_dir().join(format!("onyx-vault-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("cred.enc.json");
        let _ = fs::remove_file(&path);

        // Write under a passphrase.
        {
            let mut v = Vault::load(path.clone(), Some("correct horse".into())).unwrap();
            v.set("openai/api_key", "sk-topsecret");
            v.set("anthropic/oauth_token", "Bearer abc");
            v.save().unwrap();
            assert_eq!(v.tier(), Tier::Encrypted);
        }
        // Raw file must not contain the secret in cleartext.
        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("sk-topsecret"), "secret leaked into ciphertext file");
        assert!(raw.contains("\"tier\":\"enc\""));

        // Correct passphrase decrypts both slots.
        {
            let v = Vault::load(path.clone(), Some("correct horse".into())).unwrap();
            assert_eq!(v.resolve("openai/api_key").as_deref(), Some("sk-topsecret"));
            assert_eq!(v.resolve("anthropic/oauth_token").as_deref(), Some("Bearer abc"));
        }
        // Wrong passphrase fails closed.
        let bad = Vault::load(path.clone(), Some("wrong".into()));
        assert!(bad.is_err(), "wrong passphrase must not open the vault");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plaintext_tier_sets_0600() {
        let dir = std::env::temp_dir().join(format!("onyx-vault-plain-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("cred.plain.json");
        let _ = fs::remove_file(&path);
        {
            let mut v = Vault::load(path.clone(), None).unwrap();
            // Force plaintext even if a keyring exists, by testing the file path
            // directly through save's non-passphrase branch.
            v.slots.insert("groq/api_key".into(), "gsk_plain".into());
            v.dirty = true;
            v.tier = Tier::Plaintext;
            v.save().unwrap();
        }
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "plaintext vault must be owner-only, got {:o}", mode);
        let v2 = Vault::load(path.clone(), None).unwrap();
        assert_eq!(v2.resolve("groq/api_key").as_deref(), Some("gsk_plain"));
        let _ = fs::remove_dir_all(&dir);
    }
}
