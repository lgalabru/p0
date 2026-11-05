# Development

## Toolchain

```sh
rustup target add wasm32-wasip2   # programs
xcode-select --install            # swiftc + codesign for the Keychain helper
brew install just
```

## Layout

| Path | What |
| --- | --- |
| `wit/p0.wit` | The contract. `browser`, `sys` and `vault` are imported by programs; `manifest` and `run` are exported. |
| `crates/p0-core/src/cdp.rs` | Minimal CDP client: one browser websocket, flattened target sessions, id-correlated calls, broadcast events. |
| `crates/p0-core/src/browser.rs` | Tab ops. Every wait-and-act happens in one `Runtime.evaluate` (in-page `MutationObserver`), retried across navigations. `text=` selectors match visible buttons and links by exact text. |
| `crates/p0-core/src/host.rs` | wasmtime component host. Programs run on a blocking thread; host calls `block_on` the tokio runtime. |
| `crates/p0-core/src/vault.rs` | Per-program vault, origin allow-list, embedded Swift helper. |
| `crates/p0-core/src/macos/helper.swift` | Keychain read/write and the `LAContext` Touch ID prompt. Compiled and ad-hoc signed by `build.rs`, then installed to `~/.p0/bin/p0-keychain` only if its bytes match the embedded copy. |
| `crates/p0-sdk` | Guest SDK: `Tab`, `Manifest`/`Field`, `program!` macro. |

## Run lifecycle

1. `p0 run x.wasm` loads the component (cached compile) and calls `manifest()`.
2. If the manifest declares fields, p0 prompts for Touch ID once and reads
   `vault:<name>`. It fails fast if a required field is missing. Chrome
   launches and connects **in parallel** with the prompt.
3. `run()` executes. Secret fields are reachable only through
   `browser.fill-secret`, which checks the page origin against
   `manifest.origins`, both on the host and again in the page.
4. In-page `console.log("[p0]", …)` lines stream to the host log.

## Chrome

`p0 chrome` starts Chrome on `:9222` with `~/.p0/chrome-profile`. A dedicated
profile is required because Chrome refuses remote debugging on the default
profile. It also keeps sessions warm. Background throttling is disabled
(`--disable-background-timer-throttling`, `IntensiveWakeUpThrottling` off);
otherwise a watch loop in a background tab drops to one tick per minute.

Keep this window for p0. Browsing in it is fine, but don't touch tabs a
program is driving.

## Anti-bot notes (apple.com)

- Chrome for Testing in headless mode gets `541` on `/shop/fulfillment-messages`; real Chrome gets `200`.
- About 1.4 req/s from one session gets `503` within about 25 s. Retrying
  through it escalated to `541` for the whole session, page loads included, for
  more than 15 minutes.
- `watch.js` therefore paces itself with AIMD (additive increase,
  multiplicative decrease). It slows by ×1.5 and pauses 15 s on a 503, slows by
  ×2 and pauses 2 min on a 541, and speeds up by 5% after every 20 clean polls.

## Tests

```sh
cargo test --release            # host (origin matching, …)
just run iphone-duo --set dry-run --set force   # end-to-end against the real site, stops before paying
```
