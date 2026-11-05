//! p0 guest SDK.
//!
//! ```ignore
//! use p0_sdk::prelude::*;
//! fn main() -> Result<()> {
//!     let tab = Tab::open("https://example.com")?;
//!     tab.click("a")?;
//!     Ok(())
//! }
//! const M: Manifest = Manifest { name: "demo", origins: &[], fields: &[] };
//! p0_sdk::program!(main, M);
//! ```
//!
//! Speed model: each SDK call is one host call → one CDP round-trip (~1ms
//! locally). For anything in a tight loop, push the loop into the page with
//! [`Tab::eval`] / [`Tab::eval_json`] and return once.

#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({ path: "../../wit", world: "program", pub_export_macro: true, export_macro_name: "__export" });
}

use bindings::p0::host::{browser, sys, vault};

pub type Error = String;
pub type Result<T, E = Error> = std::result::Result<T, E>;

pub mod prelude {
    pub use crate::{Field, Manifest, Result, Tab, arg, arg_or, flag, log, notify, now_ms, sleep_ms, vault_get};
    pub use crate::{bail, info};
}

/// Default timeout for element waits.
pub const WAIT: u32 = 15_000;

#[derive(Clone, Copy, Debug)]
pub struct Tab(u32);

impl Tab {
    pub fn open(url: &str) -> Result<Self> {
        browser::tab_open(url).map(Tab)
    }
    pub fn goto(&self, url: &str) -> Result<()> {
        browser::goto(self.0, url)
    }
    /// Evaluate JS; returns the JSON-encoded result (promises awaited).
    pub fn eval(&self, js: &str) -> Result<String> {
        browser::eval(self.0, js)
    }
    pub fn eval_json<T: serde::de::DeserializeOwned>(&self, js: &str) -> Result<T> {
        let s = self.eval(js)?;
        serde_json::from_str(&s).map_err(|e| format!("decode eval result: {e}: {s}"))
    }
    pub fn click(&self, sel: &str) -> Result<()> {
        browser::click(self.0, sel, WAIT)
    }
    pub fn click_within(&self, sel: &str, ms: u32) -> Result<()> {
        browser::click(self.0, sel, ms)
    }
    pub fn fill(&self, sel: &str, value: &str) -> Result<()> {
        browser::fill(self.0, sel, value, WAIT)
    }
    /// Type vault secret `key` into `sel`. The value never enters the guest.
    pub fn fill_secret(&self, sel: &str, key: &str) -> Result<()> {
        browser::fill_secret(self.0, sel, key, WAIT)
    }
    pub fn wait_for(&self, sel: &str, ms: u32) -> Result<()> {
        browser::wait_for(self.0, sel, ms)
    }
    pub fn exists(&self, sel: &str) -> Result<bool> {
        self.eval_json(&format!("!!document.querySelector({})", js_str(sel)))
    }
    pub fn wait_url(&self, needle: &str, ms: u32) -> Result<String> {
        browser::wait_url(self.0, needle, ms)
    }
    pub fn url(&self) -> Result<String> {
        browser::url(self.0)
    }
}

/// JSON-quote a string for splicing into JS.
pub fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

pub fn log(msg: impl AsRef<str>) {
    sys::log(msg.as_ref())
}
pub fn notify(title: &str, body: &str) {
    sys::notify(title, body)
}
/// Non-secret vault field (name, phone, ...). Secrets: [`Tab::fill_secret`].
pub fn vault_get(key: &str) -> Result<String> {
    vault::get(key)
}

/// What a program needs: vault fields (prompted by `p0 vault init`) and the
/// origins where secrets may be typed.
#[derive(serde::Serialize)]
pub struct Manifest {
    pub name: &'static str,
    pub origins: &'static [&'static str],
    pub fields: &'static [Field],
}

#[derive(serde::Serialize)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub secret: bool,
    pub optional: bool,
}

impl Field {
    pub const fn text(key: &'static str, label: &'static str) -> Self {
        Self { key, label, secret: false, optional: false }
    }
    pub const fn secret(key: &'static str, label: &'static str) -> Self {
        Self { key, label, secret: true, optional: false }
    }
    pub const fn optional(self) -> Self {
        Self { optional: true, ..self }
    }
}
pub fn arg(name: &str) -> Option<String> {
    sys::arg(name)
}
pub fn arg_or(name: &str, default: &str) -> String {
    arg(name).unwrap_or_else(|| default.to_owned())
}
pub fn flag(name: &str) -> bool {
    matches!(arg(name).as_deref(), Some("1" | "true" | "yes"))
}
pub fn now_ms() -> u64 {
    sys::now_ms()
}
pub fn sleep_ms(ms: u32) {
    sys::sleep_ms(ms)
}

#[doc(hidden)]
pub fn __to_json(m: &Manifest) -> String {
    serde_json::to_string(m).unwrap()
}

#[macro_export]
macro_rules! info { ($($t:tt)*) => { $crate::log(format!($($t)*)) } }

#[macro_export]
macro_rules! bail { ($($t:tt)*) => { return Err(format!($($t)*)) } }

/// Declare the program: `p0_sdk::program!(main, MANIFEST);`
#[macro_export]
macro_rules! program {
    ($f:path, $m:expr) => {
        struct __P0;
        impl $crate::bindings::Guest for __P0 {
            fn manifest() -> ::std::string::String {
                $crate::__to_json(&$m)
            }
            fn run() -> ::std::result::Result<(), ::std::string::String> {
                $f()
            }
        }
        $crate::bindings::__export!(__P0 with_types_in $crate::bindings);
    };
}
