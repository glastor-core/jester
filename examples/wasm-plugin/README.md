# zerho Example Rust → WebAssembly Plugin

A standalone example plugin compiled from Rust to `wasm32-unknown-unknown` implementing the sandboxed `zerho` string-transform ABI (`zerho_abi_version`, `zerho_alloc`, `zerho_transform`).

## Build

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

## Enable in `zerho.toml`

```toml
[plugins]
wasm = ["examples/wasm-plugin/target/wasm32-unknown-unknown/release/zerho_example_wasm_plugin.wasm"]
fuel_limit = 1000000
```
