default:
    @just --list

# Build + install the p0 CLI into ~/.cargo/bin
install:
    cargo install --path crates/p0-cli --locked

# Build the host CLI (target/release/p0)
host:
    cargo build --release

# Build a program to a wasm component: just prog iphone-duo
prog name:
    cd programs/{{name}} && cargo build --release --target wasm32-wasip2

# Build + run a program: just run iphone-duo --set dry-run
run name *args: host (prog name)
    ./target/release/p0 run {{justfile_directory()}}/programs/{{name}}/target/wasm32-wasip2/release/$(echo {{name}} | tr - _).wasm {{args}}

# Path of a program's built wasm (for p0 vault ...)
wasm name:
    @echo {{justfile_directory()}}/programs/{{name}}/target/wasm32-wasip2/release/$(echo {{name}} | tr - _).wasm

# Scaffold a new program from programs/hello: just new my-program
new name:
    test ! -e programs/{{name}}
    mkdir -p programs/{{name}}/src
    sed 's/^name = "hello"/name = "{{name}}"/' programs/hello/Cargo.toml > programs/{{name}}/Cargo.toml
    sed 's/name: "hello"/name: "{{name}}"/' programs/hello/src/lib.rs > programs/{{name}}/src/lib.rs
    @echo "created programs/{{name}} — edit src/lib.rs, then: just run {{name}}"

chrome: host
    ./target/release/p0 chrome
