---
name: p0
description: |
  Create, configure and run p0 programs: Rust compiled to wasm components that watch a website and act end to end (in-stock alerts, add to cart, checkout, booking) by driving a warm Chrome over CDP, with a per-program Keychain vault unlocked by Touch ID once per run.
  TRIGGERS: "p0", "write a p0 program", "automate buying X", "watch for stock and buy", "snipe a restock", "auto checkout", "p0 vault", "p0 run", "add a field to the vault", "map the checkout flow"
  Use when creating a program under programs/, adding vault fields or origins, mapping a site's flow into selectors and endpoints, or running and debugging a program.
---

# p0

p0 runs **programs**: Rust crates compiled to `wasm32-wasip2` components that
implement `wit/p0.wit`. The host (`p0` CLI) gives them a warm, real Chrome
(`~/.p0/chrome-profile`, CDP on `:9222`) and a per-program vault.

Repository: `~/Coding/p0`. Read `README.md` for the why. Read
`docs/development.md` for the internals.

## The loop

```sh
just new my-program                       # copy programs/hello → programs/my-program
$EDITOR programs/my-program/src/lib.rs    # manifest + main
just prog my-program                      # build the wasm component
p0 vault init $(just wasm my-program)     # only if the manifest has required fields
just run my-program --set dry-run         # always dry-run first
```

`just run` rebuilds the host and the program, then runs `p0 run <wasm> [--set k[=v]]...`.

## Anatomy of a program

```rust
use p0_sdk::prelude::*;

const MANIFEST: Manifest = Manifest {
    name: "my-program",                          // vault key: Keychain "p0" / "vault:my-program"
    origins: &["https://shop.example.com", "https://secure*.example.com"], // where secrets may be typed
    fields: &[
        Field::text("contact.email", "Email"),
        Field::secret("card.number", "Card number"),
        Field::text("contact.company", "Company").optional(),
    ],
};

fn main() -> Result<()> {
    let dry = flag("dry-run");
    let tab = Tab::open("https://shop.example.com/item/123")?;
    // ... watch (see below), then act:
    tab.click("text=Add to Cart")?;
    tab.fill("#email", &vault_get("contact.email")?)?;
    tab.fill_secret("#card", "card.number")?;     // host types it; guest never sees it
    if dry { info!("dry-run: stopping before pay"); return Ok(()); }
    tab.click("button[type=submit]")?;
    tab.wait_url("/confirmation", 60_000)?;
    notify("bought", "order placed");
    Ok(())
}

p0_sdk::program!(main, MANIFEST);
```

Crate rules: `crate-type = ["cdylib"]`, an empty `[workspace]` table (programs
are outside the host workspace), and `p0-sdk = { path = "../../crates/p0-sdk" }`.
`just new` sets all of this up.

## SDK reference (`p0_sdk::prelude::*`)

| Call | Behaviour |
| --- | --- |
| `Tab::open(url)` | New tab; returns at DOMContentLoaded |
| `tab.goto(url)` | Navigate; returns at DOMContentLoaded |
| `tab.click(sel)` / `click_within(sel, ms)` | Waits for `sel` with an in-page MutationObserver, then `el.click()`, in one round-trip. Survives navigations. Default wait is 15 s |
| `tab.fill(sel, value)` | Waits, then sets the value through the native setter and fires `input`/`change` (works with React) |
| `tab.fill_secret(sel, key)` | Same, with a vault secret typed by the host. Refused unless the page origin matches `MANIFEST.origins` |
| `tab.wait_for(sel, ms)` / `exists(sel)` | Waits for, or checks, an element |
| `tab.wait_url(needle, ms)` / `url()` | Waits until the URL contains `needle` (event-driven) |
| `tab.eval(js)` / `eval_json::<T>(js)` | Evaluates JS in the page; promises are awaited; the result comes back as JSON |
| `vault_get(key)` | Non-secret vault field. Secrets return an error by design |
| `arg(k)` / `arg_or(k, d)` / `flag(k)` | `--set k=v`; `flag` is true for `--set k`, `=1`, `=true` or `=yes` |
| `info!(..)` / `log(..)` / `notify(title, body)` | Writes to the host log, or sends a macOS notification (plus ntfy.sh when `P0_NTFY_TOPIC` is set) |
| `now_ms()` / `sleep_ms(ms)` | Clock |
| `bail!(..)` | Returns `Err(String)` |

Selectors are CSS, or `text=Exact Visible Text` to match a visible
button, link, label or submit input by its exact text. Prefer stable hooks such
as `[data-autom=…]`, `[name=…]` or `#id` over generated class names.

## Speed rules (apply them by default)

1. **Put the hot loop in the page.** Never poll from Rust with
   `sleep_ms` + `eval`. Write a JS file, `include_str!` it, and run it with one
   `eval_json`. The JS loops, detects and **acts** (for example, adds to cart in
   the same turn), then resolves once. See `programs/iphone-duo/src/watch.js`.
2. **Poll the site's JSON endpoints, not pages.** Find them by opening the page
   with `npx agent-browser connect 9222` and running `network requests` after
   clicking through. Call them with relative `fetch` from a page on that
   origin, so cookies and anti-bot tokens come along for free.
3. **Skip redirects you don't need.** `fetch(u, {redirect: "manual"})` when the
   side effect happens on the first response (this saved 1.7 s on Apple's
   add-to-bag).
4. **Report from the page with `console.log("[p0]", …)`.** It streams to the
   host log without a round-trip.
5. **Respect rate limits with AIMD pacing.** On a 503, slow down and pause. On
   a 541 or 403 (bot block), back off hard (minutes). Pushing through a block
   extends it to the whole session. Copy the pacing in `watch.js`.

## Mapping a new site's flow

1. Run `p0 chrome`, then `npx -y agent-browser connect 9222`, then open
   `npx -y agent-browser tab new <url>`. Use **your own tab**; the user may be
   browsing in this Chrome.
2. Walk the flow with `snapshot -i`, `click @eN`, `network requests`, and
   `eval` to dump forms and `[data-autom]` hooks.
3. Record for each step: the URL pattern, the selector to act on, and the
   selector or URL that proves the step finished.
4. Map checkout with a **cheap in-stock item** and stop at the review page.
   **Never click the final pay/place-order button while mapping.**
5. Turn each step into `click`/`fill`/`fill_secret` + `wait_for`/`wait_url`.
6. Clean up anything you added (cart items).

## Vault

- `p0 vault init <wasm>` prompts for every manifest field. Secrets are hidden as
  you type, and pressing Enter keeps the current value.
- `p0 vault import <wasm> < file.json` replaces the vault with a JSON object of
  strings. Unknown keys are rejected.
- `p0 vault show <wasm>` (Touch ID, secrets masked) and `p0 vault rm <wasm>`.
- `p0 run` checks whether the vault exists **before** prompting. With no vault,
  it fails fast if any field is required, and runs with an empty vault if all
  fields are optional. Otherwise it prompts for Touch ID once, then checks that
  every required field is present.
- Adding a field changes the manifest. Rebuild and rerun `p0 vault init`;
  existing values are kept.

Never ask the user to paste secrets into chat, and never write them to
files. Tell them to run `p0 vault init` themselves.

## Running and debugging

- `P0_TRACE=1 p0 run …` logs every browser call with its latency.
  `P0_TRACE=2` also logs every CDP event that `wait_url` sees, including frame
  URLs. Use it to tell whether the site or p0 is slow.
- `p0 run … --vault-file dummy.json` reads a **dummy** vault from a file (no
  Touch ID). It's for rehearsals only; never put real secrets on disk.
- Rehearse money flows with an in-stock cheap item and a stop point (see
  `iphone-duo`: `--set rehearse=<part> --set stop=billing`). Empty the cart
  afterwards.
- A `WARNING: Chrome was started without p0's anti-throttling flags` message
  means hidden tabs get 1 s timers and no `requestAnimationFrame`. Quit Chrome
  and run `p0 chrome`. Programs open their own window to soften this.

- Log lines look like `[  12.345s name] msg`. Lines starting `page:` come from
  in-page `console.log("[p0]", …)`.
- Successful runs close their tabs. Failed runs leave them open so a human
  can take over.
- `Chrome did not open :9222` means `p0 chrome` failed or another process holds the port.
- `refusing to type X into <origin>` means the origin is missing from `MANIFEST.origins`.
- `timeout waiting for <sel>`: check the selector against the live page with
  agent-browser. Sites change; a `text=` selector is often sturdier.
- 541 or 403 from a site's API means the session is flagged. Stop and wait
  (more than 15 minutes for Apple). Do not retry in a loop.

## Safety

- Default to `--set dry-run` for any program that spends money, and run it at
  least once before going live. Programs should stop before the final pay
  click when `flag("dry-run")` is set.
- Put a hard price cap in the program and check the total on the review page
  before paying.
- Do not build rate-limit or bot-detection evasion, such as IP rotation,
  fingerprint spoofing or CAPTCHA solving.

See `references/iphone-duo.md` for a fully worked program.
