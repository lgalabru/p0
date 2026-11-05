//! Watch Apple Downtown Brooklyn for iPhone Duo 256GB Star White (unlocked)
//! in-store pickup; the instant it's available, add to bag and check out.
//!
//! p0 run iphone_duo.wasm                     # live (yolo: places the order)
//! p0 run iphone_duo.wasm --set dry-run       # stop before "Place Order"
//! p0 run iphone_duo.wasm --set dry-run --set force   # pretend it's in stock now
//! p0 run iphone_duo.wasm --set rehearse=MW493AM/A    # full pickup checkout with a $19 cable, stops at review

use p0_sdk::prelude::*;
use serde::Deserialize;
use serde_json::json;

const PART: &str = "MK1E4LL/A"; // iPhone Duo 256GB Star White, unlocked (US)
const STORE: &str = "R715"; // Apple Downtown Brooklyn
const PRODUCT_PATH: &str = "/shop/buy-iphone/iphone-duo/7.6-inch-display-256gb-star-white-unlocked";
const BAG_API_KEY: &str = "SJHJUH4YFCTTPD4F4";
const MAX_TOTAL_USD: f64 = 2_300.0; // $1,999 + NY tax ≈ $2,176; refuse anything above

const WATCH_JS: &str = include_str!("watch.js");

/// Everything a full unattended checkout needs. `p0 vault init iphone_duo.wasm`
/// prompts for these once; `p0 run` unlocks them with one Touch ID.
/// Guest checkout is the primary path: no Apple ID session to expire, no 2FA.
const MANIFEST: Manifest = Manifest {
    name: "iphone-duo",
    origins: &["https://www.apple.com", "https://secure*.store.apple.com"],
    fields: &[
        Field::text("contact.first_name", "First name"),
        Field::text("contact.last_name", "Last name"),
        Field::text("contact.email", "Email"),
        Field::text("contact.phone", "Phone (10 digits)"),
        Field::secret("card.number", "Card number"),
        Field::secret("card.exp", "Card expiry (MM/YY)"),
        Field::secret("card.cvv", "Card CVV"),
        Field::text("billing.street", "Billing street address"),
        Field::text("billing.street2", "Billing apt/suite").optional(),
        Field::text("billing.city", "Billing city"),
        Field::text("billing.state", "Billing state (e.g. NY)"),
        Field::text("billing.zip", "Billing ZIP"),
        Field::text("apple_id.email", "Apple ID (fallback sign-in)").optional(),
        Field::secret("apple_id.password", "Apple ID password").optional(),
    ],
};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct Hit {
    polls: u64,
    ms: u64,
    atb_ms: Option<u64>,
    items: Option<u64>,
    quote: Option<String>,
    already: Option<bool>,
}

fn main() -> Result<()> {
    let dry = flag("dry-run");
    let force = flag("force");
    if force && !dry {
        bail!("--set force requires --set dry-run");
    }
    let interval: u32 = arg_or("interval", "2500").parse().map_err(|_| "bad interval")?;

    // Rehearsal: run the real pickup checkout with an in-stock item (always dry).
    if let Some(part) = arg("rehearse") {
        let tab = Tab::open(&format!("https://www.apple.com/shop/product/{part}"))?;
        // The button can render before the page hydrates (click is a no-op), so
        // confirm through the bag API and retry the click until the item lands.
        // Clicking may navigate (killing the eval), so drive it from here.
        let status = format!("fetch('/shop/bag/status?apikey={BAG_API_KEY}',{{cache:'no-store'}}).then(r=>r.json()).then(j=>j.items)");
        let mut items = 0u32;
        for i in 0..20 {
            if i % 6 == 0 {
                let _ = tab.eval("(b => b && b.click())([...document.querySelectorAll('button')].find(b => b.innerText.trim() === 'Add to Bag' && b.offsetParent && !b.disabled))");
            }
            sleep_ms(500);
            items = tab.eval_json(&status).unwrap_or(0);
            if items > 0 {
                break;
            }
        }
        if items == 0 {
            bail!("rehearsal: add to bag did not land");
        }
        info!("bag items: {items}");
        info!("rehearsing checkout with {part}");
        return checkout(&tab, true);
    }

    let tab = Tab::open(&format!("https://www.apple.com{PRODUCT_PATH}"))?;
    info!("watching {PART} @ {STORE} every ~{interval}ms (dry-run={dry})");

    let cfg = json!({
        "part": PART, "store": store(), "cppart": "UNLOCKED/US", "productPath": PRODUCT_PATH,
        "bagApiKey": BAG_API_KEY, "interval": interval, "maxInterval": 15_000, "pauseMs": 15_000, "blockPauseMs": 120_000, "conc": 1, "heartbeatMs": 60_000, "force": force, "skipIfInBag": true,
    });
    let hit: Hit = tab.eval_json(&WATCH_JS.replace("__CFG__", &cfg.to_string()))?;
    info!("in bag: {hit:?}");
    notify("p0: iPhone Duo in bag", "Pickup at Downtown Brooklyn — checking out");

    checkout(&tab, dry)?;
    Ok(())
}

/// Steps mapped from a live guest walk-through (2026-11-04, secure6.store.apple.com).
/// Every step waits on something unique to the *next* step before acting, so the
/// shared `#rs-checkout-continue-button-bottom` id is never double-clicked.
fn checkout(tab: &Tab, dry: bool) -> Result<()> {
    let t0 = now_ms();
    let lap = |what: &str| info!("checkout +{}ms {what}", now_ms() - t0);
    const CONTINUE: &str = "#rs-checkout-continue-button-bottom";
    const CONTACT: &str = "checkout.pickupContact.selfPickupContact.selfContact.address.";
    const CARD: &str = "checkout.billing.billingOptions.selectedBillingOptions.creditCard.cardInputs.cardInput-0.";
    const BILL: &str = "checkout.billing.billingOptions.selectedBillingOptions.creditCard.billingAddress.address.";
    let id = |s: &str| format!("[id=\"{s}\"]");

    tab.goto("https://www.apple.com/shop/bag")?;
    tab.click("button[data-autom=checkout]")?;
    let url = tab.wait_url("store.apple.com/shop/", 20_000)?;
    if url.contains("/signIn") {
        tab.click("text=Continue as Guest")?; // no Apple ID session to expire, no 2FA
    }
    tab.wait_url("_s=Fulfillment", 20_000)?;
    lap("fulfillment");

    // Pickup at R715.
    tab.click("text=I\u{2019}ll pick it up")?;
    let store_sel = format!("input[name=store-locator-result][value={}]", store());
    tab.click(&store_sel)?;
    let ok: bool = tab.eval_json(&format!("(e => !!e && e.checked && !e.disabled)(document.querySelector({}))", p0_sdk::js_str(&store_sel)))?;
    if !ok {
        bail!("store {} not selectable for pickup", store());
    }
    tab.click(CONTINUE)?;
    tab.wait_url("_s=PickupContact", 20_000)?;
    lap("pickup contact");

    let first = vault_get("contact.first_name")?;
    let last = vault_get("contact.last_name")?;
    tab.fill(&id(&format!("{CONTACT}firstName")), &first)?;
    tab.fill(&id(&format!("{CONTACT}lastName")), &last)?;
    tab.fill(&id(&format!("{CONTACT}emailAddress")), &vault_get("contact.email")?)?;
    tab.fill(&id(&format!("{CONTACT}fullDaytimePhone")), &vault_get("contact.phone")?)?;
    tab.click(CONTINUE)?;
    tab.wait_url("_s=Billing", 20_000)?;
    lap("billing");

    tab.click(&id("checkout.billing.billingoptions.credit"))?;
    tab.fill_secret(&id(&format!("{CARD}cardNumber")), "card.number")?;
    tab.fill_secret(&id(&format!("{CARD}expiration")), "card.exp")?;
    tab.fill_secret(&id(&format!("{CARD}securityCode")), "card.cvv")?;
    tab.fill(&id(&format!("{BILL}firstName")), &first)?;
    tab.fill(&id(&format!("{BILL}lastName")), &last)?;
    tab.fill(&id(&format!("{BILL}street")), &vault_get("billing.street")?)?;
    let street2 = vault_get("billing.street2")?;
    if !street2.is_empty() {
        tab.fill(&id(&format!("{BILL}street2")), &street2)?;
    }
    tab.fill(&id(&format!("{BILL}zipLookup.postalCode")), &vault_get("billing.zip")?)?;
    // ZIP lookup populates a city/state <select>; pick the vault's city.
    let city_sel = id(&format!("{BILL}zipLookup.zipLookupCityState"));
    let picked: String = tab.eval_json(&SELECT_CITY_JS
        .replace("__SEL__", &p0_sdk::js_str(&city_sel))
        .replace("__CITY__", &p0_sdk::js_str(&vault_get("billing.city")?))
        .replace("__STATE__", &p0_sdk::js_str(&vault_get("billing.state")?)))?;
    info!("billing city/state: {picked}");
    if arg("stop").as_deref() == Some("billing") {
        lap("stop=billing: not submitting payment");
        return Ok(());
    }
    tab.click(CONTINUE)?;
    tab.wait_url("_s=Review", 30_000)?;
    lap("review");

    // Price cap: read the order total from the review page.
    let total: f64 = tab.eval_json(TOTAL_JS)?;
    info!("order total ${total:.2} (cap ${MAX_TOTAL_USD:.0})");
    if !(total > 0.0 && total <= MAX_TOTAL_USD) {
        notify("p0: total over cap", &format!("${total:.2} > ${MAX_TOTAL_USD:.0}; not ordering"));
        bail!("total ${total:.2} outside (0, {MAX_TOTAL_USD}]");
    }
    if dry {
        lap("DRY RUN: stopping before Place Order");
        notify("p0 dry-run reached review", &format!("${total:.2} — not ordered"));
        return Ok(());
    }

    let clicked: String = tab.eval_json(PLACE_ORDER_JS)?;
    lap(&format!("clicked {clicked:?}"));
    let done = tab.wait_url("thankyou", 60_000).or_else(|_| tab.wait_url("_s=Thank", 1))?;
    lap("ORDER PLACED");
    notify("p0: iPhone Duo ORDERED", &format!("${total:.2} — pickup at Downtown Brooklyn\n{done}"));
    Ok(())
}

const SELECT_CITY_JS: &str = r#"new Promise((res, rej) => {
  const sel = __SEL__, city = __CITY__.toLowerCase(), st = __STATE__.toLowerCase();
  const t0 = Date.now();
  const tick = () => {
    const el = document.querySelector(sel);
    if (!el) return res("no city select (single match)");
    // Options can have empty values; skip placeholders and "Other".
    const opts = [...el.options].filter(o => o.text.trim() && !/^(other|select.*)$/i.test(o.text.trim()));
    if (!opts.length) return Date.now() - t0 > 25000 ? rej(new Error("zip lookup gave no cities")) : setTimeout(tick, 50);
    const o = opts.find(o => o.text.toLowerCase().includes(city) && o.text.toLowerCase().includes(st)) || opts.find(o => o.text.toLowerCase().includes(city)) || opts[0];
    el.selectedIndex = o.index;
    el.dispatchEvent(new Event("change", { bubbles: true }));
    res(o.text);
  };
  tick();
})"#;

/// Last "Total ... $X" on the page is the order total.
const TOTAL_JS: &str = r#"(() => {
  const m = [...document.body.innerText.matchAll(/Total[^$\n]*\$\s*([\d,]+\.\d{2})/gi)];
  return m.length ? parseFloat(m[m.length - 1][1].replace(/,/g, "")) : -1;
})()"#;

const PLACE_ORDER_JS: &str = r#"new Promise((res, rej) => {
  const t0 = Date.now();
  const tick = () => {
    const b = [...document.querySelectorAll("button")].find(b => /place (your )?order/i.test(b.innerText) && !b.disabled && b.offsetParent);
    if (b) { b.click(); return res(b.innerText.trim()); }
    Date.now() - t0 > 15000 ? rej(new Error("no Place Order button")) : setTimeout(tick, 50);
  };
  tick();
})"#;

fn store() -> String {
    arg_or("store", STORE)
}

p0_sdk::program!(main, MANIFEST);
