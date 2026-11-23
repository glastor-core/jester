# Changelog

All notable changes to **zerho** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-11-22

### Added
- **Zero-Node 100% Rust Architecture:** 9-crate Cargo workspace (`zerho-core`, `zerho-config`, `zerho-resolver`, `zerho-graph`, `zerho-plugin`, `zerho-transform`, `zerho-server`, `zerho-bundler`, `zerho-cli`) enforcing `unsafe_code = "forbid"` and `clippy::pedantic`.
- **Declarative Configuration (`zerho.toml`):** Fast TOML loader, validator, JSON Schema generator (`zerho schema`), `.env` / `import.meta.env` support, `[server.proxy]`, and `public/` static assets.
- **Native Compiler Pipeline:** TypeScript + React/Preact/Solid JSX compilation via `oxc`, CSS nesting & minification via `lightningcss`, and Node ESM/CJS resolution via `oxc_resolver`.
- **Dev Server & DX:** `axum` + `tokio` + `notify` dev server, granular `import.meta.hot` HMR with state preservation, Shadow DOM `<zerho-error-overlay>`, `/@zerho/deps` automatic CJS→ESM pre-bundling, and embedded `/__inspect` DevTools UI.
- **Persistent Disk Cache:** Content-addressed `blake3` cache (`.zerho/cache`) with sub-millisecond warm hits across cold restarts.
- **Production Bundler (`zerho build`) & Preview (`zerho preview`):** Two-pass AST symbol-level tree-shaking, dynamic `import()` code splitting (`assets/chunk-<hash>.js`), Source Maps v3 (`linked`/`inline`/`hidden`), `blake3` asset hashing, `public/` copying, and `manifest.json`.
- **WASM Plugin System:** Sandboxed `wasmtime` execution engine + `zerho:plugin@0.1.0` WIT contract (`zerho wit`) and built-in `DefinePlugin`.
- **Reproducible Benchmarks (`zerho bench`):** Built-in transform, disk cache, and production build benchmarking.
