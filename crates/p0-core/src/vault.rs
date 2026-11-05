//! Per-program vault: one Keychain item (service "p0", account "vault:<program>")
//! holding a flat JSON object. Unlocked with Touch ID once per `p0 run`, then
//! held in host memory for the program's lifetime (zeroized on drop).
//!
//! Secret fields never cross into the wasm guest: the guest asks the host to
//! type them into the page (`fill-secret`), and only on origins the program's
//! manifest declares.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/p0-keychain"));

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    pub name: String,
    /// Origins where secrets may be typed, e.g. "https://secure*.store.apple.com".
    #[serde(default)]
    pub origins: Vec<String>,
    #[serde(default)]
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub optional: bool,
}

pub struct Vault {
    values: BTreeMap<String, Zeroizing<String>>,
    manifest: Manifest,
}

fn helper() -> Result<PathBuf> {
    let dir = PathBuf::from(std::env::var("HOME").context("HOME")?).join(".p0/bin");
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let path = dir.join("p0-keychain");
    // Only ever exec bytes identical to the ones embedded at build time.
    if std::fs::read(&path).ok().as_deref() != Some(HELPER) {
        let tmp = dir.join(format!(".p0-keychain.{}", std::process::id()));
        std::fs::write(&tmp, HELPER)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o700))?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(path)
}

fn account(program: &str) -> String {
    format!("vault:{program}")
}

fn run(args: &[&str], stdin: Option<&[u8]>) -> Result<Zeroizing<Vec<u8>>> {
    let mut c = Command::new(helper()?)
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(data) = stdin {
        c.stdin.take().unwrap().write_all(data)?;
    }
    let o = c.wait_with_output()?;
    if !o.status.success() {
        let e = String::from_utf8_lossy(&o.stderr);
        bail!("{}", e.trim().trim_start_matches("ERROR:"));
    }
    Ok(Zeroizing::new(o.stdout))
}

pub fn exists(program: &str) -> bool {
    run(&["exists", &account(program)], None).is_ok_and(|o| o.starts_with(b"yes"))
}

pub fn store(program: &str, values: &BTreeMap<String, Zeroizing<String>>) -> Result<()> {
    let plain: BTreeMap<&str, &str> = values.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let json = Zeroizing::new(serde_json::to_vec(&plain)?);
    run(&["store", &account(program)], Some(&json)).map(drop)
}

pub fn delete(program: &str) -> Result<()> {
    run(&["delete", &account(program)], None).map(drop)
}

/// Touch ID prompt, then read + decode.
pub fn unlock_raw(program: &str, reason: &str) -> Result<BTreeMap<String, Zeroizing<String>>> {
    let raw = run(&["read-protected", &account(program), reason], None)
        .with_context(|| format!("unlock vault {program:?} (create it with `p0 vault init`)"))?;
    let map: BTreeMap<String, String> = serde_json::from_slice(&raw).context("vault is not a JSON object")?;
    Ok(map.into_iter().map(|(k, v)| (k, Zeroizing::new(v))).collect())
}

/// Touch ID prompt, read, and check every required manifest field is present.
pub fn unlock(manifest: &Manifest, reason: &str) -> Result<Vault> {
    let values = unlock_raw(&manifest.name, reason)?;
    check(Vault { values, manifest: manifest.clone() })
}

/// Testing only: dummy vault from a JSON file (same validation as `unlock`).
pub fn from_file(manifest: &Manifest, path: &std::path::Path) -> Result<Vault> {
    let raw = Zeroizing::new(std::fs::read(path)?);
    let map: BTreeMap<String, String> = serde_json::from_slice(&raw).context("vault file is not a JSON object")?;
    let values = map.into_iter().map(|(k, v)| (k, Zeroizing::new(v))).collect();
    check(Vault { values, manifest: manifest.clone() })
}

fn check(v: Vault) -> Result<Vault> {
    let missing: Vec<_> = v
        .manifest
        .fields
        .iter()
        .filter(|f| !f.optional && v.values.get(&f.key).is_none_or(|s| s.is_empty()))
        .map(|f| f.key.as_str())
        .collect();
    if !missing.is_empty() {
        bail!("vault {:?} missing: {} (run `p0 vault init`)", v.manifest.name, missing.join(", "));
    }
    Ok(v)
}

impl Vault {
    pub fn empty(manifest: &Manifest) -> Self {
        Self { values: BTreeMap::new(), manifest: manifest.clone() }
    }

    fn field(&self, key: &str) -> Option<&Field> {
        self.manifest.fields.iter().find(|f| f.key == key)
    }

    /// Plaintext to the guest: non-secret fields only.
    pub fn get(&self, key: &str) -> Result<String> {
        match self.field(key) {
            Some(f) if f.secret => bail!("{key} is secret; use fill_secret"),
            None => bail!("{key} not declared in manifest"),
            _ => {}
        }
        Ok(self.values.get(key).map(|v| v.to_string()).unwrap_or_default())
    }

    /// For host-side typing into the page. Checks the page origin first.
    pub fn secret_for(&self, key: &str, page_url: &str) -> Result<Zeroizing<String>> {
        if self.field(key).is_none() {
            bail!("{key} not declared in manifest");
        }
        let origin = origin_of(page_url);
        if !self.manifest.origins.iter().any(|pat| glob(pat, &origin)) {
            bail!("refusing to type {key} into {origin}: not in manifest origins");
        }
        Ok(self.values.get(key).cloned().unwrap_or_default())
    }
}

fn origin_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    format!("{scheme}://{host}")
}

/// One `*` allowed; it matches within the host only (no '/').
fn glob(pat: &str, s: &str) -> bool {
    match pat.split_once('*') {
        None => pat == s,
        Some((pre, post)) => {
            s.len() >= pre.len() + post.len()
                && s.starts_with(pre)
                && s.ends_with(post)
                && !s[pre.len()..s.len() - post.len()].contains('/')
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origins() {
        assert!(glob("https://secure*.store.apple.com", "https://secure6.store.apple.com"));
        assert!(glob("https://www.apple.com", "https://www.apple.com"));
        assert!(!glob("https://secure*.store.apple.com", "https://evil.com/.store.apple.com"));
        assert_eq!(origin_of("https://secure6.store.apple.com/shop/checkout?x=1"), "https://secure6.store.apple.com");
    }
}
