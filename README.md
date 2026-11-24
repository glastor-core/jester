# zerho

**Zero-Node, 100% Rust frontend dev server and production bundler.**

`zerho` ships as a single **16.60 MB** native Rust binary (`unsafe_code = "forbid"`) with declarative `zerho.toml` configuration, first-class support for React/Preact/Solid + TypeScript + CSS, embedded ACID `redb` + HTTP remote team caching, Scope Hoisting + `oxc_minifier`, React Fast Refresh, Brotli `.br`, Library Mode, pure-Rust SVG/WebP optimization, `zerho check`, and a WebAssembly Component Model (`zerho:plugin@0.1.0`) plugin contract.

## Real `--release` Benchmarks (v0.7.0)

> **Reference Environment:** `Windows 11 x86_64 (NT 10.0.26300.0)` · `AMD Zen 3 (16 logical threads)` · `rustc 1.96.1` · `zerho 0.7.0 --release` (`16.60 MB`). Full methodology & JSON snapshot in [`benchmarks/BENCHMARKS.md`](benchmarks/BENCHMARKS.md) and [`benchmarks/results.json`](benchmarks/results.json).

| Project | Cold Oxc Transform | Warm `redb` Lookup | Cold Build *(empty cache)* | Warm Build Min *(redb hot)* | Entry Size *(Raw / Gzip / Brotli)* |
| :--- | :---: | :---: | :---: | :---: | :---: |
| `examples/basic-ts` (3 mods) | **0.553 ms** | **0.0017 ms** *(1.7 µs)* | **43.04 ms** | **19.32 ms** | 804 B / 543 B / **458 B** |
| `examples/react-tsx` (5 mods + split) | **0.548 ms** | **0.0020 ms** *(2.0 µs)* | **60.27 ms** | **29.18 ms** | 2.28 KB / 1.09 KB / **1.00 KB** |
| `synthetic-500-modules` (500 TS mods) | **1.266 ms** | **0.0435 ms** | **1,254 ms** | **763.53 ms** *(655 mods/s)* | 17.37 KB / 2.73 KB / **1.53 KB** |

Verify on your own machine in one command:

```bash
cargo build --release -p zerho-cli && ./target/release/zerho bench --synthetic 500 --iterations 5 --json
```

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
