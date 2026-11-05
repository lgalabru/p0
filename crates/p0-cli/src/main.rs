use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use p0_core::{browser, cdp::Cdp, host, vault};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(name = "p0", about = "Run Rust/wasm automation programs against a warm Chrome")]
struct Cli {
    /// Chrome remote debugging port
    #[arg(long, global = true, default_value_t = browser::DEFAULT_PORT)]
    port: u16,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Launch the p0 Chrome (dedicated profile, throttling off)
    Chrome,
    /// Run a program (.wasm component). Unlocks its vault with Touch ID first.
    Run {
        wasm: PathBuf,
        /// Program arguments, e.g. --set dry-run
        #[arg(long = "set", value_parser = parse_kv)]
        set: Vec<(String, String)>,
        /// Testing only: read the vault from a JSON file instead of the Keychain
        /// (use dummy data; never real secrets on disk)
        #[arg(long)]
        vault_file: Option<PathBuf>,
    },
    /// Manage a program's vault (Keychain item, Touch ID gated)
    Vault {
        #[command(subcommand)]
        cmd: VaultCmd,
    },
    /// Send a test notification
    Notify { title: String, body: String },
}

#[derive(Subcommand)]
enum VaultCmd {
    /// Prompt for every field the program's manifest declares (secrets hidden)
    Init { wasm: PathBuf },
    /// Replace the vault with a JSON object read from stdin
    Import { wasm: PathBuf },
    /// Show fields (secrets masked)
    Show { wasm: PathBuf },
    /// Delete the vault
    Rm { wasm: PathBuf },
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((k, v)) => Ok((k.into(), v.into())),
        None => Ok((s.into(), "1".into())),
    }
}

fn manifest_of(wasm: &Path) -> Result<vault::Manifest> {
    let engine = host::engine()?;
    let rt = tokio::runtime::Handle::current();
    let l = host::load(&engine, rt, None, wasm, HashMap::new(), host::Logger::new("manifest"))?;
    Ok(l.manifest)
}

fn mask(v: &str) -> String {
    let n = v.chars().count();
    if n <= 4 { "•".repeat(n) } else { format!("{}{}", "•".repeat(n - 4), v.chars().skip(n - 4).collect::<String>()) }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Chrome => {
            browser::launch_chrome(cli.port).await?;
            eprintln!("Chrome up on :{} (profile {})", cli.port, browser::profile_dir().display());
        }
        Cmd::Run { wasm, set, vault_file } => {
            let name = wasm.file_stem().and_then(|s| s.to_str()).unwrap_or("program").to_owned();
            let args: HashMap<_, _> = set.into_iter().collect();
            let rt = tokio::runtime::Handle::current();
            let log = host::Logger::new(&name);
            let port = cli.port;
            // Load + Touch ID on a blocking thread, Chrome launch in parallel:
            // the user's finger and Chrome startup overlap.
            let chrome = tokio::spawn(async move {
                browser::launch_chrome(port).await?;
                let cdp = Cdp::connect(port).await?;
                if let Some(w) = browser::check_flags(port) {
                    eprintln!("WARNING: {w}");
                }
                anyhow::Ok(cdp)
            });
            let mut loaded = tokio::task::spawn_blocking({
                let rt = rt.clone();
                move || -> Result<host::Loaded> {
                    let engine = host::engine()?;
                    let mut l = host::load(&engine, rt, None, &wasm, args, log)?;
                    let wasm = wasm.clone();
                    let m = &l.manifest;
                    if let Some(f) = vault_file {
                        eprintln!("WARNING: using test vault file {}", f.display());
                        l.set_vault(vault::from_file(m, &f)?);
                    } else if vault::exists(&m.name) {
                        // The only Touch ID prompt of the run.
                        let v = vault::unlock(m, &format!("unlock the p0 vault for {}", m.name))?;
                        l.set_vault(v);
                    } else if m.fields.iter().any(|f| !f.optional) {
                        // Fail before prompting: nothing to unlock.
                        bail!("no vault for {:?}; run `p0 vault init {}`", m.name, wasm.display());
                    } else {
                        l.set_vault(vault::Vault::empty(m));
                    }
                    Ok(l)
                }
            })
            .await??;
            loaded.set_cdp(chrome.await??);
            tokio::task::spawn_blocking(move || loaded.run()).await.context("program thread")??;
        }
        Cmd::Vault { cmd } => vault_cmd(cmd)?,
        Cmd::Notify { title, body } => p0_core::notify::send(&title, &body),
    }
    Ok(())
}

fn vault_cmd(cmd: VaultCmd) -> Result<()> {
    match cmd {
        VaultCmd::Init { wasm } => {
            let m = manifest_of(&wasm)?;
            if m.fields.is_empty() {
                bail!("{} declares no vault fields", m.name);
            }
            let mut existing: BTreeMap<String, Zeroizing<String>> = BTreeMap::new();
            if vault::exists(&m.name) {
                eprintln!("vault {:?} exists; unlocking to edit (Enter keeps current value)", m.name);
                let v = vault::unlock_raw(&m.name, "edit the p0 vault")?;
                existing = v;
            }
            eprintln!("vault {:?}: secrets are hidden as you type", m.name);
            let stdin = std::io::stdin();
            let mut out = BTreeMap::new();
            for f in &m.fields {
                let cur = existing.get(&f.key);
                let hint = match (cur, f.secret) {
                    (Some(c), true) if !c.is_empty() => format!(" [{}]", mask(c)),
                    (Some(c), false) if !c.is_empty() => format!(" [{}]", c.as_str()),
                    _ if f.optional => " (optional)".into(),
                    _ => String::new(),
                };
                let prompt = format!("{}{hint}: ", f.label);
                let val = if f.secret {
                    Zeroizing::new(rpassword::prompt_password(&prompt)?)
                } else {
                    eprint!("{prompt}");
                    std::io::stderr().flush()?;
                    let mut l = String::new();
                    stdin.lock().read_line(&mut l)?;
                    Zeroizing::new(l.trim_end_matches(['\n', '\r']).to_owned())
                };
                let val = if val.is_empty() { cur.cloned().unwrap_or_default() } else { val };
                if val.is_empty() && !f.optional {
                    bail!("{} is required", f.key);
                }
                out.insert(f.key.clone(), val);
            }
            vault::store(&m.name, &out)?;
            eprintln!("saved vault {:?} ({} fields)", m.name, out.len());
        }
        VaultCmd::Import { wasm } => {
            let m = manifest_of(&wasm)?;
            let mut buf = Zeroizing::new(String::new());
            std::io::stdin().read_to_string(&mut buf)?;
            let map: BTreeMap<String, String> = serde_json::from_str(&buf).context("stdin must be a JSON object of strings")?;
            let unknown: Vec<_> = map.keys().filter(|k| !m.fields.iter().any(|f| &f.key == *k)).cloned().collect();
            if !unknown.is_empty() {
                bail!("unknown keys for {}: {}", m.name, unknown.join(", "));
            }
            let map = map.into_iter().map(|(k, v)| (k, Zeroizing::new(v))).collect();
            vault::store(&m.name, &map)?;
            eprintln!("saved vault {:?}", m.name);
        }
        VaultCmd::Show { wasm } => {
            let m = manifest_of(&wasm)?;
            let v = vault::unlock_raw(&m.name, "show the p0 vault")?;
            for f in &m.fields {
                let val = v.get(&f.key).map(|s| s.as_str()).unwrap_or("");
                let shown = if f.secret { mask(val) } else { val.to_owned() };
                let flag = if val.is_empty() && !f.optional { "  ← MISSING" } else { "" };
                println!("{:<22} {shown}{flag}", f.key);
            }
        }
        VaultCmd::Rm { wasm } => {
            let m = manifest_of(&wasm)?;
            vault::delete(&m.name)?;
            eprintln!("deleted vault {:?}", m.name);
        }
    }
    Ok(())
}
