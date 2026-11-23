# zerho

**Zero-Node, 100% Rust frontend dev server and production bundler.**

`zerho` ships as a single native binary with declarative `zerho.toml` configuration, first-class support for React/Preact/Solid + TypeScript + CSS, persistent `blake3` disk caching, an embedded `/__inspect` DevTools UI, and a WebAssembly Component Model (`zerho:plugin@0.1.0`) plugin contract.

## Workspace Crates

| Crate | Responsibility |
| :--- | :--- |
| `zerho-core` | Core identifiers (`ModuleId`, `blake3` `ContentHash`), `ModuleKind`, `Framework`, and error types |
| `zerho-config` | Declarative `zerho.toml` parser, validator, serializer, and JSON Schema generator |
| `zerho-resolver` | Native module resolution powered by `oxc_resolver` (ESM/CJS, `package.json`, virtual modules) |
| `zerho-graph` | Incremental module dependency graph with reverse-edge HMR invalidation and persistent `DiskCache` |
| `zerho-plugin` | Native `Plugin` trait (`DefinePlugin`), WASM descriptor validator, and `zerho:plugin@0.1.0` WIT contract |
| `zerho-transform` | Native `oxc` TS/JSX compiler, AST import rewriter, ESM bundler lowering, and `lightningcss` pipeline |
| `zerho-server` | Zero-Node `axum` + `tokio` + `notify` dev server, WebSocket HMR, `/@zerho/deps`, `/__inspect` UI, and `PreviewServer` |
| `zerho-bundler` | Production bundler (graph walk, JS/CSS minification, `blake3` asset hashing, `index.html`, `manifest.json`) |
| `zerho-cli` | Single binary entrypoint (`dev`, `build`, `preview`, `bench`, `init`, `schema`, `wit`) |

## Quick Start

```bash
# Scaffold a new Zero-Node React + TSX project in milliseconds
zerho init my-app --template react-tsx
cd my-app
zerho dev

# Or run directly from the workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p zerho-cli -- dev --config examples/react-tsx/zerho.toml
```
