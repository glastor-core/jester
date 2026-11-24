# Changelog

All notable changes to **zerho** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.0] - 2026-11-23

### Added
- **Native `tsconfig.json` `paths` Resolution:** Enabled `TsconfigDiscovery` + `TsconfigReferences::Auto` directly in `OxcResolver`, resolving `@/*` aliases in dev, build, and check without plugins.
- **Barrel File Optimizer:** AST-based `parse_pure_barrel` & `rewrite_barrel_imports` bypass pure re-export barrels (`export { X } from './leaf'`) before graph expansion so unused transitive modules are never loaded or parsed.
- **`zerho check` Static Graph Auditor:** Parallel (`rayon`) auditor detecting broken static/dynamic imports, circular dependency chains (`ModuleGraph::find_cycles`), undeclared `import.meta.env.<PREFIX>*` references, and leaked client secrets (`--strict` mode).
- **Native Bundle Analyzer (`dist/stats.html`):** Zero-dependency interactive treemap & chunk table (`zerho build --analyze` or `build.analyze = true`) displaying Raw/Gzip/Brotli sizes, SRI status, Scope Hoisting mode, exports shaken, and bypassed barrels.

## [0.5.0] - 2026-11-23

### Added
- **AST Minifier + Mangler + Constant Folding / DCE:** Enabled `oxc` `minifier` & `mangler` features in `finalize_js_bundle_with_map` with `Codegen::with_scoping` while preserving Source Maps v3.
- **Lexical Scope Hoisting (`build.scope_hoisting = true`):** Flattens acyclic ESM module graphs into a single strict IIFE without `__zerho_registry` / `__zerho_require` wrappers using `oxc::semantic` symbol resolution, with automatic fallback to registry mode for CJS/cycles/namespace imports.

## [0.4.0] - 2026-11-23

### Added
- **Incremental Build Cache in `Bundler`:** Wired `DiskCache` (`redb` + remote team cache) into `Bundler::load_and_transform_module` keyed by source + `.env` hash + framework.
- **Pre-Compressed Brotli (`.br`) & `PreviewServer` Negotiation:** Pure-Rust `brotli` emission (`build.brotli = true`), Raw/Gzip/Brotli CLI size reporting, and automatic `Content-Encoding: br` serving in `PreviewServer`.
- **Shared Chunks + `<link rel="modulepreload">`:** Extracts modules shared across ≥2 dynamic `import()` chunks into `assets/shared-<hash>.js` and injects `<link rel="modulepreload">` (with SRI) into `dist/index.html` and `manifest.json`.

## [0.3.0] - 2026-11-23

### Added
- **Embedded ACID `redb` Cache + HTTP Remote Team Cache:** Migrated `DiskCache` to single-file `zerho-cache.redb` with optional `cache.remote_url` and `cache.remote_token` (`env:VAR` support) and automatic local backfill.
- **Parallel Bundler (`rayon`):** Wave-by-wave BFS parallel loading/transforming + deterministic post-order DFS emission + parallel tree-shaking and ESM lowering.

## [0.2.0] - 2026-11-22

### Added
- **Native Production Security:** SHA-384 Subresource Integrity (`integrity="sha384-..."`), strict `dist/csp.json`, and client bundle Secret Leak Guard.
- **Real Gzip Reporting & `[build.budgets]`:** `flate2` gzip measurement and hard budget enforcement (`max_js_kb`, `max_css_kb`, `max_chunk_kb`).
- **Pure-Rust SVG Optimizer (`[assets].optimize_svg`)** and **Dev Partial Bundling (`GET /@zerho/bundle`)**.

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
