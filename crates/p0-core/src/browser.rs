//! Tab operations on top of the raw CDP client.
//!
//! Speed rules:
//! - wait + act happen in ONE in-page evaluation (no host round-trip between
//!   "element appeared" and "click").
//! - waits are MutationObserver-driven in the page, never host polling.
//! - navigation completes on DOMContentLoaded, not `load`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::cdp::Cdp;

pub const DEFAULT_PORT: u16 = 9222;

/// In-page helper: resolve with the element once `sel` matches (or reject on timeout).
const WAIT_JS: &str = r#"(sel, ms) => new Promise((res, rej) => {
  // "text=Continue as Guest" matches a visible button/link/label by exact text.
  const q = sel.startsWith("text=")
    ? () => { const t = sel.slice(5).trim();
        return [...document.querySelectorAll("button,a,[role=button],label,input[type=submit]")]
          .find(e => (e.innerText || e.value || "").trim() === t && e.offsetParent !== null); }
    : () => document.querySelector(sel);
  const hit = q(); if (hit) return res(hit);
  const mo = new MutationObserver(() => { const e = q(); if (e) { mo.disconnect(); clearTimeout(t); res(e); } });
  mo.observe(document.documentElement || document, { childList: true, subtree: true, attributes: true });
  const t = setTimeout(() => { mo.disconnect(); rej(new Error("timeout waiting for " + sel)); }, ms);
})"#;

#[derive(Clone)]
pub struct Tab {
    cdp: Cdp,
    pub session: String,
    pub target: String,
}

impl Tab {
    pub async fn open(cdp: &Cdp, url: &str, log: impl Fn(String) + Send + 'static) -> Result<Self> {
        // Own window: a background *tab* gets its timers throttled to 1/s (then
        // 1/min), which stalls in-page loops and SPA checkouts by ~10s per step.
        // A separate visible window is not backgrounded.
        let t = cdp
            .call(None, "Target.createTarget", json!({ "url": "about:blank", "newWindow": true, "background": false }))
            .await?;
        let target = t["targetId"].as_str().context("targetId")?.to_owned();
        let a = cdp
            .call(None, "Target.attachToTarget", json!({ "targetId": target, "flatten": true }))
            .await?;
        let session = a["sessionId"].as_str().context("sessionId")?.to_owned();
        let tab = Self { cdp: cdp.clone(), session, target };
        tab.call("Page.enable", json!({})).await?;
        tab.call("Runtime.enable", json!({})).await?;
        // A background tab gets its timers throttled (1/s, then 1/min) unless Chrome
        // was started by `p0 chrome`. Make the page believe it is focused and active
        // so SPA timers (checkout steps!) run at full speed regardless of launch flags.
        tab.call("Emulation.setFocusEmulationEnabled", json!({ "enabled": true })).await?;
        let _ = tab.call("Page.setWebLifecycleState", json!({ "state": "active" })).await;
        tab.stream_console(log);
        tab.goto(url).await?;
        Ok(tab)
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.cdp.call(Some(&self.session), method, params).await
    }

    /// Forward page `console.log("[p0] ...")` lines to the host log, so in-page
    /// hot loops can report without round-trips.
    fn stream_console(&self, log: impl Fn(String) + Send + 'static) {
        let mut rx = self.cdp.subscribe();
        let session = self.session.clone();
        tokio::spawn(async move {
            while let Ok(ev) = rx.recv().await {
                if ev.session.as_deref() != Some(&session) || ev.method != "Runtime.consoleAPICalled" {
                    continue;
                }
                let args = ev.params["args"].as_array().cloned().unwrap_or_default();
                let first = args.first().and_then(|a| a["value"].as_str()).unwrap_or("");
                if let Some(rest) = first.strip_prefix("[p0]") {
                    let tail: Vec<String> = args[1..]
                        .iter()
                        .map(|a| a.get("value").map(|v| v.to_string()).unwrap_or_default())
                        .collect();
                    log(format!("{}{}", rest.trim_start(), if tail.is_empty() { String::new() } else { format!(" {}", tail.join(" ")) }));
                }
            }
        });
    }

    pub async fn goto(&self, url: &str) -> Result<()> {
        let mut rx = self.cdp.subscribe();
        let r = self.call("Page.navigate", json!({ "url": url })).await?;
        if let Some(e) = r.get("errorText").and_then(Value::as_str) {
            bail!("navigate {url}: {e}");
        }
        let deadline = tokio::time::sleep(Duration::from_secs(60));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => bail!("timeout loading {url}"),
                ev = rx.recv() => {
                    let ev = ev.map_err(|e| anyhow!("event stream: {e}"))?;
                    if ev.session.as_deref() == Some(&self.session) && ev.method == "Page.domContentEventFired" {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Evaluate an expression; promises are awaited; returns JSON text.
    pub async fn eval(&self, js: &str) -> Result<String> {
        let r = self
            .call(
                "Runtime.evaluate",
                json!({ "expression": js, "awaitPromise": true, "returnByValue": true, "userGesture": true }),
            )
            .await?;
        if let Some(ex) = r.get("exceptionDetails") {
            let msg = ex["exception"]["description"].as_str().or(ex["text"].as_str()).unwrap_or("exception");
            bail!("{msg}");
        }
        Ok(r["result"].get("value").cloned().unwrap_or(Value::Null).to_string())
    }

    /// Run `body` (JS using `el`) once `sel` matches. Retries across navigations
    /// (an in-flight evaluation dies with its execution context).
    async fn with_el(&self, sel: &str, timeout_ms: u32, body: &str) -> Result<String> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        loop {
            let left = deadline.saturating_duration_since(Instant::now()).as_millis() as u64;
            if left == 0 {
                bail!("timeout waiting for {sel}");
            }
            let js = format!("({WAIT_JS})({}, {left}).then(el => {{ {body} }})", json!(sel));
            match self.eval(&js).await {
                Ok(v) => return Ok(v),
                Err(e) if is_nav_error(&e) => continue,
                Err(e) => return Err(e),
            }
        }
    }

    pub async fn wait_for(&self, sel: &str, timeout_ms: u32) -> Result<()> {
        self.with_el(sel, timeout_ms, "return true;").await.map(drop)
    }

    pub async fn click(&self, sel: &str, timeout_ms: u32) -> Result<()> {
        self.with_el(sel, timeout_ms, "el.scrollIntoView({block:'center'}); el.click(); return true;")
            .await
            .map(drop)
    }

    pub async fn fill(&self, sel: &str, value: &str, timeout_ms: u32) -> Result<()> {
        // Native setter so React/Vue-controlled inputs see the change.
        let body = format!(
            "el.focus(); const p = Object.getPrototypeOf(el); \
             Object.getOwnPropertyDescriptor(p, 'value').set.call(el, {v}); \
             el.dispatchEvent(new Event('input', {{bubbles:true}})); \
             el.dispatchEvent(new Event('change', {{bubbles:true}})); el.blur(); return true;",
            v = json!(value)
        );
        self.with_el(sel, timeout_ms, &body).await.map(drop)
    }

    /// Like `fill`, but aborts if the page that owns the element is no longer
    /// on `expected_url`'s origin (guards a navigation between check and type).
    pub async fn fill_checked(&self, sel: &str, value: &str, expected_url: &str, timeout_ms: u32) -> Result<()> {
        let origin = {
            let (scheme, rest) = expected_url.split_once("://").unwrap_or(("", expected_url));
            format!("{scheme}://{}", rest.split(['/', '?', '#']).next().unwrap_or(""))
        };
        let body = format!(
            "if (location.origin !== {o}) throw new Error('origin changed'); \
             el.focus(); const p = Object.getPrototypeOf(el); \
             Object.getOwnPropertyDescriptor(p, 'value').set.call(el, {v}); \
             el.dispatchEvent(new Event('input', {{bubbles:true}})); \
             el.dispatchEvent(new Event('change', {{bubbles:true}})); el.blur(); return true;",
            o = json!(origin),
            v = json!(value)
        );
        self.with_el(sel, timeout_ms, &body).await.map(drop)
    }

    pub async fn url(&self) -> Result<String> {
        let v = self.eval("location.href").await?;
        Ok(serde_json::from_str::<String>(&v)?)
    }

    pub async fn wait_url(&self, needle: &str, timeout_ms: u32) -> Result<String> {
        let mut rx = self.cdp.subscribe();
        let t = Instant::now();
        let cur = self.url().await;
        if std::env::var_os("P0_TRACE").is_some() {
            eprintln!("  wait_url: url() {:.1}ms -> {:?}", t.elapsed().as_secs_f64() * 1e3, cur.as_ref().map(|u| &u[..u.len().min(80)]));
        }
        if let Ok(u) = cur
            && u.contains(needle)
        {
            return Ok(u);
        }
        let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms as u64));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => bail!("timeout waiting for url ~ {needle}"),
                ev = rx.recv() => {
                    let ev = ev.map_err(|e| anyhow!("event stream: {e}"))?;
                    if std::env::var("P0_TRACE").as_deref() == Ok("2") {
                        eprintln!("  wait_url ev +{:.0}ms {} {}", t.elapsed().as_secs_f64() * 1e3, ev.method, ev.params.get("frame").map(|f| format!("{} parent={}", f["url"].as_str().unwrap_or("").chars().take(90).collect::<String>(), f.get("parentId").is_some())).or(ev.params.get("url").map(|u| u.to_string())).unwrap_or_default());
                    }
                    if ev.session.as_deref() != Some(&self.session) { continue }
                    let u = match ev.method.as_str() {
                        "Page.frameNavigated" if ev.params["frame"].get("parentId").is_none() =>
                            ev.params["frame"]["url"].as_str().map(str::to_owned),
                        "Page.navigatedWithinDocument" => ev.params["url"].as_str().map(str::to_owned),
                        _ => None,
                    };
                    if let Some(u) = u && u.contains(needle) { return Ok(u) }
                }
            }
        }
    }
}

fn is_nav_error(e: &anyhow::Error) -> bool {
    let s = e.to_string();
    s.contains("context was destroyed") || s.contains("Cannot find context") || s.contains("Inspected target navigated")
}

/// Dedicated profile: keeps Apple ID session warm, separate from daily Chrome
/// (Chrome refuses remote debugging on the default profile).
/// Warn when the Chrome on `port` lacks p0's anti-throttling flags (e.g. it was
/// started by hand). Reads the process table: CDP only reports the command
/// line under --enable-automation.
pub fn check_flags(port: u16) -> Option<String> {
    let out = std::process::Command::new("ps").args(["-ax", "-o", "args="]).output().ok()?;
    let needle = format!("--remote-debugging-port={port}");
    let ps = String::from_utf8_lossy(&out.stdout);
    let args = ps.lines().find(|l| l.contains(&needle) && !l.contains("--type="))?;
    let missing: Vec<_> = THROTTLE_FLAGS.iter().filter(|f| !args.contains(*f)).collect();
    (!missing.is_empty()).then(|| {
        "Chrome was started without p0's anti-throttling flags: hidden tabs get 1s timers and no rAF, \
         which slows checkouts ~10s/step. Quit Chrome and run `p0 chrome`."
            .to_owned()
    })
}

const THROTTLE_FLAGS: [&str; 3] =
    ["--disable-background-timer-throttling", "--disable-renderer-backgrounding", "--disable-backgrounding-occluded-windows"];

pub fn profile_dir() -> PathBuf {
    if let Ok(p) = std::env::var("P0_PROFILE") {
        return PathBuf::from(p);
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".p0/chrome-profile")
}

/// Launch Chrome with throttling disabled: background tabs otherwise get timers
/// clamped to 1/s, then 1/min after 5 minutes, which would kill in-page watch loops.
pub async fn launch_chrome(port: u16) -> Result<()> {
    if crate::cdp::is_up(port).await {
        return Ok(());
    }
    let bin = std::env::var("P0_CHROME")
        .unwrap_or_else(|_| "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into());
    std::process::Command::new(bin)
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!("--user-data-dir={}", profile_dir().display()))
        .args([
            "--no-first-run",
            "--no-default-browser-check",
            THROTTLE_FLAGS[0],
            THROTTLE_FLAGS[1],
            THROTTLE_FLAGS[2],
            "--disable-features=IntensiveWakeUpThrottling,CalculateNativeWinOcclusion",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("spawn Chrome")?;
    for _ in 0..100 {
        if crate::cdp::is_up(port).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("Chrome did not open :{port}")
}
