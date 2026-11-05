//! Wasm component host: runs one program per OS thread; host calls block on
//! the tokio runtime (cheap: ~µs per call, irrelevant next to network RTT).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::runtime::Handle;
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::browser::Tab;
use crate::cdp::Cdp;

wasmtime::component::bindgen!({ path: "../../wit", world: "program" });

use p0::host::{browser, sys, vault as vault_if};
use crate::vault::{Manifest, Vault};

#[derive(Clone)]
pub struct Logger {
    t0: Instant,
    name: Arc<str>,
}

impl Logger {
    pub fn new(name: &str) -> Self {
        Self { t0: Instant::now(), name: name.into() }
    }
    pub fn log(&self, msg: &str) {
        let t = self.t0.elapsed();
        eprintln!("[{:>8.3}s {}] {msg}", t.as_secs_f64(), self.name);
    }
}

pub struct State {
    rt: Handle,
    cdp: Option<Cdp>,
    tabs: Vec<Tab>,
    args: HashMap<String, String>,
    log: Logger,
    vault: Vault,
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl State {
    fn tab(&self, id: u32) -> Result<Tab, String> {
        self.tabs.get(id as usize).cloned().ok_or_else(|| format!("no tab {id}"))
    }
}

fn s<T>(r: Result<T>) -> Result<T, String> {
    r.map_err(|e| format!("{e:#}"))
}

impl State {
    /// P0_TRACE=1 logs every browser call with its latency.
    fn traced<T>(&self, what: &str, f: impl std::future::Future<Output = Result<T>>) -> Result<T, String> {
        let t = Instant::now();
        let r = self.rt.block_on(f);
        if std::env::var_os("P0_TRACE").is_some() {
            let ok = if r.is_ok() { "ok" } else { "ERR" };
            self.log.log(&format!("trace {what} {ok} {:.1}ms", t.elapsed().as_secs_f64() * 1e3));
        }
        s(r)
    }
}

impl browser::Host for State {
    fn tab_open(&mut self, url: String) -> Result<u32, String> {
        let log = self.log.clone();
        let cdp = self.cdp.as_ref().ok_or("no browser")?;
        let tab = s(self.rt.block_on(Tab::open(cdp, &url, move |m| log.log(&format!("page: {m}")))))?;
        self.tabs.push(tab);
        Ok(self.tabs.len() as u32 - 1)
    }
    fn goto(&mut self, tab: u32, url: String) -> Result<(), String> {
        let t = self.tab(tab)?;
        self.traced(&format!("goto {url}"), t.goto(&url))
    }
    fn eval(&mut self, tab: u32, js: String) -> Result<String, String> {
        let t = self.tab(tab)?;
        self.traced(&format!("eval {}b", js.len()), t.eval(&js))
    }
    fn click(&mut self, tab: u32, selector: String, timeout_ms: u32) -> Result<(), String> {
        let t = self.tab(tab)?;
        self.traced(&format!("click {selector}"), t.click(&selector, timeout_ms))
    }
    fn fill(&mut self, tab: u32, selector: String, value: String, timeout_ms: u32) -> Result<(), String> {
        let t = self.tab(tab)?;
        self.traced(&format!("fill {selector}"), t.fill(&selector, &value, timeout_ms))
    }
    fn wait_for(&mut self, tab: u32, selector: String, timeout_ms: u32) -> Result<(), String> {
        let t = self.tab(tab)?;
        self.traced(&format!("wait_for {selector}"), t.wait_for(&selector, timeout_ms))
    }
    fn wait_url(&mut self, tab: u32, needle: String, timeout_ms: u32) -> Result<String, String> {
        let t = self.tab(tab)?;
        self.traced(&format!("wait_url {needle}"), t.wait_url(&needle, timeout_ms))
    }
    fn fill_secret(&mut self, tab: u32, selector: String, key: String, timeout_ms: u32) -> Result<(), String> {
        let t = self.tab(tab)?;
        // Re-check origin right before typing; the wait can span a navigation,
        // so the element is also re-checked against the origin in-page.
        let url = s(self.rt.block_on(t.url()))?;
        let value = s(self.vault.secret_for(&key, &url))?;
        self.traced(&format!("fill_secret {selector}"), t.fill_checked(&selector, &value, &url, timeout_ms))
    }
    fn url(&mut self, tab: u32) -> Result<String, String> {
        let t = self.tab(tab)?;
        s(self.rt.block_on(t.url()))
    }
}

impl sys::Host for State {
    fn log(&mut self, msg: String) {
        self.log.log(&msg);
    }
    fn notify(&mut self, title: String, body: String) {
        crate::notify::send(&title, &body);
    }
    fn arg(&mut self, name: String) -> Option<String> {
        self.args.get(&name).cloned()
    }
    fn now_ms(&mut self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }
    fn sleep_ms(&mut self, ms: u32) {
        std::thread::sleep(Duration::from_millis(ms as u64));
    }
}

impl vault_if::Host for State {
    fn get(&mut self, key: String) -> Result<String, String> {
        s(self.vault.get(&key))
    }
}

pub fn engine() -> Result<Engine> {
    let mut cfg = Config::new();
    cfg.wasm_component_model(true);
    // Cache compiled artifacts on disk: second start of a program skips Cranelift.
    if let Ok(cache) = wasmtime::Cache::from_file(None) {
        cfg.cache(Some(cache));
    }
    Engine::new(&cfg).map_err(w)
}

fn w(e: wasmtime::Error) -> anyhow::Error {
    anyhow!("{e:?}")
}

pub struct Loaded {
    store: Store<State>,
    prog: Program,
    pub manifest: Manifest,
}

/// Compile/instantiate a program and read its manifest (no browser needed yet).
pub fn load(engine: &Engine, rt: Handle, cdp: Option<Cdp>, wasm: &Path, args: HashMap<String, String>, log: Logger) -> Result<Loaded> {
    let t = Instant::now();
    let component = Component::from_file(engine, wasm).map_err(w).with_context(|| format!("load {}", wasm.display()))?;
    let mut linker: Linker<State> = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(w)?;
    Program::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s).map_err(w)?;
    let wasi = WasiCtx::builder().inherit_stderr().inherit_stdout().build();
    let state = State {
        rt,
        cdp,
        tabs: vec![],
        args,
        log: log.clone(),
        vault: Vault::empty(&Manifest::default()),
        wasi,
        table: ResourceTable::new(),
    };
    let mut store = Store::new(engine, state);
    let prog = Program::instantiate(&mut store, &component, &linker).map_err(w)?;
    let manifest: Manifest = serde_json::from_str(&prog.call_manifest(&mut store).map_err(w)?).context("program manifest")?;
    log.log(&format!("loaded {} in {:.1}ms", manifest.name, t.elapsed().as_secs_f64() * 1e3));
    Ok(Loaded { store, prog, manifest })
}

impl Loaded {
    pub fn set_vault(&mut self, v: Vault) {
        self.store.data_mut().vault = v;
    }
    pub fn set_cdp(&mut self, cdp: Cdp) {
        self.store.data_mut().cdp = Some(cdp);
    }
    /// Run to completion. Blocks the calling thread; call from spawn_blocking.
    /// On success the program's tabs are closed; on failure they stay open so a
    /// human can take over (e.g. finish a checkout).
    pub fn run(mut self) -> Result<()> {
        let r = self.prog.call_run(&mut self.store).map_err(w)?.map_err(|e| anyhow!("program failed: {e}"));
        if r.is_ok() {
            let st = self.store.data();
            if let Some(cdp) = &st.cdp {
                for t in &st.tabs {
                    let _ = st.rt.block_on(cdp.call(None, "Target.closeTarget", serde_json::json!({ "targetId": t.target })));
                }
            }
        }
        r
    }
}
