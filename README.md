# zerho

**Zero-Node, 100% Safe Rust frontend dev server and production bundler.**

[![Portal](https://img.shields.io/badge/Portal-zerho.xyz-f97316?style=for-the-badge)](https://zerho.xyz)
[![Safe Rust](https://img.shields.io/badge/Rust-unsafe__code%20%3D%20forbid-0f172a?style=for-the-badge)](https://github.com/glastor-core/jester)
[![Version](https://img.shields.io/badge/Version-v0.9.0-10b981?style=for-the-badge)](CHANGELOG.md)

`zerho` (`https://github.com/glastor-core/jester`) ships as a single **16.60 MB** native Rust binary (`unsafe_code = "forbid"`, `clippy::pedantic -D warnings`, **60/60** passing tests) with declarative `zerho.toml` configuration. Built on `oxc`, `lightningcss`, `oxc_resolver`, `redb`, `blake3`, `rayon`, `axum`, and `wasmtime v29`, it powers React, Preact, TypeScript, and modern web apps without installing Node.js.

## What `zerho v0.9.0` Actually Ships

- **Compiler & Assets (`zerho-transform`):** Sub-millisecond `oxc` TS/JSX transforms, native React Fast Refresh (`$RefreshReg$`), `oxc_minifier` + `oxc_mangler` + DCE, lexical Scope Hoisting, Barrel File Optimizer, deep multi-file CommonJS ↔ ESM pre-bundler (`require()` graphs), `lightningcss` CSS Modules + `@import`/`url(...)` rewriting, Web Worker `new URL(..., import.meta.url)` rewriting, and pure-Rust SVG/WebP optimization.
- **Dev Server & DevTools (`zerho-server`):** Zero-Node `axum` + `tokio` + `notify` dev server, granular WebSocket HMR (`import.meta.hot`), Shadow DOM Error Overlay, zero-waterfall Dev Partial Bundling (`/@zerho/bundle`), HTTP reverse proxy (`[server.proxy]`), built-in `/__inspect` UI with **Live Bundle Treemap**, and Brotli-negotiating `PreviewServer`.
- **Production, MPA, Library & Edge SSR (`zerho-bundler`):** `rayon` wave-parallel bundling, incremental ACID `redb` + HTTP remote team cache (`cache.remote_url`), Shared Chunks + `<link rel="modulepreload">`, Multi-Page Apps (`[build.entries]`), Library Mode (`[build.lib]` `.mjs`/`.cjs`), Web Worker chunks + `.wasm` `WebAssembly.instantiateStreaming` wrappers, DOM-free Edge SSR (`zerho build --ssr` → `dist/server.mjs`), Brotli `.br`, SHA-384 SRI, strict `dist/csp.json`, Gzip budgets, client Secret Leak Guard, `zerho check`, and `dist/stats.html`.
- **WASM Plugin Sandbox (`zerho-plugin`):** Fuel-metered `wasmtime v29` host with UTF-8 linear-memory string passing (`zerho_alloc`, `zerho_resolve_id`, `zerho_load`, `zerho_transform`) for `zerho:plugin@0.1.0`, plus a complete Rust → `wasm32-unknown-unknown` example in [`examples/wasm-plugin/`](examples/wasm-plugin/README.md).
- **Zero-Friction Migration (`zerho migrate`):** Inspects `vite.config.*`, `package.json`, `index.html`, and `tsconfig.json` to generate a ready-to-run `zerho.toml`.

> **Current Scope (Honest Boundaries):** Ideal today for React, Preact, TypeScript SPAs, MPAs, libraries, Web Workers/`.wasm`, and Edge SSR without Node.js. If your project relies on Vue SFCs, Svelte, Nuxt, SvelteKit, or legacy Rollup/Vite JS plugins, stay on Vite for now.

## Real `--release` Benchmarks

> **Reference Environment:** `Windows 11 x86_64 (NT 10.0.26300.0)` · `AMD Zen 3 (16 logical threads)` · `rustc 1.96.1` · `zerho --release` (`16.60 MB`, snapshot captured on `v0.7.0` and reproducible on `v0.9.0`). Full methodology & JSON snapshot in [`benchmarks/BENCHMARKS.md`](benchmarks/BENCHMARKS.md) and [`benchmarks/results.json`](benchmarks/results.json).

| Project | Cold Oxc Transform | Warm `redb` Lookup | Cold Build *(empty cache)* | Warm Build Min *(redb hot)* | Entry Size *(Raw / Gzip / Brotli)* |
| :--- | :---: | :---: | :---: | :---: | :---: |
| `examples/basic-ts` (3 mods) | **0.553 ms** | **0.0017 ms** *(1.7 µs)* | **43.04 ms** | **19.32 ms** | 804 B / 543 B / **458 B** |
| `examples/react-tsx` (5 mods + split) | **0.548 ms** | **0.0020 ms** *(2.0 µs)* | **60.27 ms** | **29.18 ms** | 2.28 KB / 1.09 KB / **1.00 KB** |
| `synthetic-500-modules` (500 TS mods) | **1.266 ms** | **0.0435 ms** | **1,254 ms** | **763.53 ms** *(655 mods/s)* | 17.37 KB / 2.73 KB / **1.53 KB** |

Verify on your own machine in one command:

```bash
cargo build --release -p zerho-cli && ./target/release/zerho bench --synthetic 500 --iterations 5 --json
```

## Workspace Crates (9 Crates · 60/60 Tests)

| Crate | Responsibility |
| :--- | :--- |
| `zerho-core` | Core identifiers (`ModuleId`, `blake3` `ContentHash`), `ModuleKind`, `Framework`, and typed error hierarchy |
| `zerho-config` | Declarative `zerho.toml` parser, validator, MPA/SSR/budgets/security/lib/remote-cache config, and JSON Schema generator |
| `zerho-resolver` | Native ESM/CJS & `tsconfig.json` `paths` resolution powered by `oxc_resolver`, `package.json`, and virtual modules |
| `zerho-graph` | Incremental module graph, DFS cycle detector, embedded ACID `redb` cache, and HTTP remote team cache |
| `zerho-plugin` | Native `Plugin` trait (`DefinePlugin`), fuel-metered `wasmtime` v29 host with UTF-8 string hooks, and `zerho:plugin@0.1.0` WIT contract |
| `zerho-transform` | `oxc` TS/JSX/React Fast Refresh compiler, `oxc_minifier`/`oxc_mangler`, Scope Hoisting, Barrel & CJS pre-bundler, Web Workers, CSS `@import`/`url()`, SVG optimizer |
| `zerho-server` | Zero-Node `axum` + `tokio` + `notify` dev server, WebSocket HMR, `/@zerho/bundle`, `/__inspect` Live Treemap, and Brotli `PreviewServer` |
| `zerho-bundler` | `rayon` parallel bundler, incremental `redb` cache, Shared Chunks + `modulepreload`, MPA, Library Mode, Web Workers/`.wasm`, Edge SSR (`--ssr`), WebP, `zerho check`, `dist/stats.html` |
| `zerho-cli` | Single binary CLI (`init`, `migrate`, `dev`, `build [--analyze] [--watch] [--ssr]`, `check [--strict]`, `preview`, `bench [--synthetic N] [--json]`, `schema`, `wit`) |

## Quick Start

```bash
# Scaffold a new Zero-Node React + TSX project in milliseconds
zerho init my-app --template react-tsx
cd my-app
zerho dev

# Or migrate an existing Vite project automatically
zerho migrate

# Static full-graph audit (broken imports, cycles, missing .env, secrets)
zerho check

# Scope-hoisted, minified, Brotli-compressed build + interactive dist/stats.html
zerho build --analyze

# Emit a DOM-free Edge SSR bundle (dist/server.mjs) for Cloudflare Workers / Deno / Node / Bun
zerho build --ssr

# Watch mode for incremental redb-cached builds
zerho build --watch

# Run workspace checks & tests
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

---

Part of the **GLASTOR® Rust Toolchain** — [zerho.xyz](https://zerho.xyz) · [glastor.es](https://glastor.es) · [pii-veil](https://pii-veil.glastor.es) · [specter](https://specter.glastor.es)
