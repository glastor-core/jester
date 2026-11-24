# zerho

**Zero-Node, 100% Rust frontend dev server and production bundler.**

`zerho` ships as a single **16.57 MB** native Rust binary (`unsafe_code = "forbid"`) with declarative `zerho.toml` configuration, first-class support for React/Preact/Solid + TypeScript + CSS, embedded ACID `redb` + HTTP remote team caching, Scope Hoisting + `oxc_minifier`, React Fast Refresh, Brotli `.br`, Library Mode, pure-Rust SVG/WebP optimization, `zerho check`, and a WebAssembly Component Model (`zerho:plugin@0.1.0`) plugin contract.

## Real `--release` Benchmarks (v0.7.0, Windows, 20 iterations)

| Project | Cold Oxc Transform | Warm `redb` Cache Hit | Full Production Build (Scope Hoisting + Minifier + Brotli + SRI) |
| :--- | :---: | :---: | :---: |
| `examples/basic-ts` (3 modules) | **0.73 ms** | **0.002 ms** (2 µs) | **17.06 ms** min (19.96 ms avg) |
| `examples/react-tsx` (5 modules + split + sourcemaps) | **0.63 ms** | **0.002 ms** (2 µs) | **29.76 ms** min (35.16 ms avg) |

## Workspace Crates

| Crate | Responsibility |
| :--- | :--- |
| `zerho-core` | Core identifiers (`ModuleId`, `blake3` `ContentHash`), `ModuleKind`, `Framework`, and typed error hierarchy |
| `zerho-config` | Declarative `zerho.toml` parser, validator, budgets/security/lib/remote-cache config, and JSON Schema generator |
| `zerho-resolver` | Native ESM/CJS & `tsconfig.json` `paths` resolution powered by `oxc_resolver`, `package.json`, and virtual modules |
| `zerho-graph` | Incremental module graph, DFS cycle detector, embedded ACID `redb` cache, and HTTP remote team cache |
| `zerho-plugin` | Native `Plugin` trait (`DefinePlugin`), fuel-metered `wasmtime` v29 host, and `zerho:plugin@0.1.0` WIT contract |
| `zerho-transform` | `oxc` TS/JSX/React Fast Refresh compiler, `oxc_minifier`/`oxc_mangler`, Scope Hoisting, Barrel Optimizer, SVG optimizer, `lightningcss` |
| `zerho-server` | Zero-Node `axum` + `tokio` + `notify` dev server, WebSocket HMR, `/@zerho/bundle`, `/__inspect` UI, and Brotli `PreviewServer` |
| `zerho-bundler` | `rayon` parallel bundler, incremental `redb` cache, Shared Chunks + `modulepreload`, Library Mode, WebP optimizer, `zerho check`, `dist/stats.html` |
| `zerho-cli` | Single binary CLI (`dev`, `build [--analyze] [--watch]`, `check [--strict]`, `preview`, `bench`, `init`, `schema`, `wit`) |

## Quick Start

```bash
# Scaffold a new Zero-Node React + TSX project in milliseconds
zerho init my-app --template react-tsx
cd my-app
zerho dev

# Static full-graph audit (broken imports, cycles, missing .env, secrets)
zerho check

# Scope-hoisted, minified, Brotli-compressed build + interactive dist/stats.html
zerho build --analyze

# Watch mode for incremental redb-cached builds
zerho build --watch

# Run workspace checks & tests
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
