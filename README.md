<div align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-github-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/assets/banner-github-light.png">
    <img alt="p0" width="50%" src="docs/assets/banner-github-light.png">
  </picture>
</div>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/programs-rust%20%E2%86%92%20wasm-orange">
  <img alt="Wasmtime" src="https://img.shields.io/badge/runtime-wasmtime%20components-blue">
  <img alt="macOS" src="https://img.shields.io/badge/vault-Keychain%20%2B%20Touch%20ID-black">
</p>

**p0 is a priority lane for the web.** Rust programs, compiled to WebAssembly,
watch a site for the moment something happens and complete the purchase in a
real Chrome. Your details stay in the Keychain, unlocked with Touch ID.

## Getting started

```sh
just install          # build the p0 CLI into ~/.cargo/bin
p0 chrome             # launch the p0 Chrome (dedicated profile, throttling off)
```

A **program** is a wasm component with two exports. `manifest()` declares the
data the program needs and the sites it may type secrets into. `run()` does the
work. Build the bundled one, fill its vault once, then run it:

```sh
just prog iphone-duo
p0 vault init programs/iphone-duo/target/wasm32-wasip2/release/iphone_duo.wasm
just run iphone-duo --set dry-run --set force   # pretend it's in stock, stop before paying
just run iphone-duo                             # live: watch, add to bag, check out
```

Writing your own program, the SDK, and vault fields are covered in
[skills/p0/SKILL.md](skills/p0/SKILL.md). The internals are in
[docs/development.md](docs/development.md).

## Why it's fast

The bottleneck is never CPU. It's round-trips, page loads and anti-bot
friction, so every design decision removes one of them.

| | p0 does | instead of |
| --- | --- | --- |
| 🔁 **Hot loop** | polls, decides and acts **inside the page** with one `eval` | a host round-trip per poll |
| 📡 **Signal** | calls the site's own JSON endpoints | reloading and scraping pages |
| ⚡ **Act** | waits with a `MutationObserver` and clicks in the same evaluation | polling for selectors |
| 🧭 **Navigation** | continues at `DOMContentLoaded`, and waits survive navigations | waiting for `load` |
| 🔥 **Browser** | uses real Chrome with a warm profile and timer throttling off | headless Chrome, which Apple's bot protection blocks (541) |
| 🪶 **Host** | keeps one CDP websocket and caches compiled wasm (start ≈ 10 ms) | a protocol framework and a cold start |

Measured against apple.com: an availability check takes about 300 ms, and
add-to-bag plus bag verification takes **205 ms**. Following Apple's
add-to-bag redirect took 1.9 s.

## Trust model

| | |
| --- | --- |
| 🔐 **Vault** | One Keychain item per program (`p0` / `vault:<name>`), stored only on this Mac and readable only while it is unlocked |
| 👆 **Touch ID** | Prompted once when `p0 run` starts. The vault stays in host memory for the run and is wiped on exit |
| 🙈 **Secrets** | Never enter the wasm guest. `fill_secret` has the host type them directly into the page |
| 🌐 **Origins** | Secrets are typed only on origins the manifest lists, and p0 re-checks the origin at the moment it types |

## Where this is going

| | | |
| --- | --- | --- |
| 🍎 **iphone-duo** | Pickup at Apple Downtown Brooklyn, guest checkout | Watch + add to bag + checkout to Billing verified live; Review/Place Order pending a real-card dry-run |
| 🖥️ **Menubar app** | Running programs, logs and a kill switch over the same core | Next |
| 🧰 **Recorder** | Walk a flow once with [agent-browser](https://github.com/vercel-labs/agent-browser) and generate a program | Later |

## Repository

```
crates/     🦀 p0-core (CDP client, wasm host, vault), p0-cli (`p0`), p0-sdk (guest SDK)
wit/        📜 p0.wit — the host ⇄ program contract
programs/   🤖 programs, one crate each (wasm32-wasip2)
skills/     🧠 agent skill: create, configure and run programs
docs/       📐 development guide, assets
```
