//! Template program: open a page, wait for an element, read it, notify.
//! `just new <name>` copies this crate.

use p0_sdk::prelude::*;

const MANIFEST: Manifest = Manifest {
    name: "hello",
    // Origins where vault secrets may be typed. Empty: this program has none.
    origins: &[],
    fields: &[Field::text("greeting.name", "Your name").optional()],
};

fn main() -> Result<()> {
    let who = vault_get("greeting.name").unwrap_or_default();
    let tab = Tab::open("https://example.com")?;
    tab.wait_for("p", 10_000)?;
    let title: String = tab.eval_json("document.title")?;
    info!("hello {who}: page says {title:?}");
    if !flag("quiet") {
        notify("p0 hello", &title);
    }
    Ok(())
}

p0_sdk::program!(main, MANIFEST);
