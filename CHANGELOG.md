# Changelog

All notable changes to **zerho** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.1.0] - 2026-11-27

### Added
- **Vite-Compatible `?raw` & `?url` Import Suffixes (`zerho-resolver`, `zerho-server`, `zerho-bundler`):** Supports `import txt from './file.sql?raw'` (UTF-8 string export) and `import url from './asset.svg?url'` (hashed asset URL respecting `[build].base`) in both `zerho dev` and `zerho build`.
- **Real Chromium CDP E2E Gate in CI (`scripts/verify-chromium-e2e.mjs`, `.github/workflows/ci.yml`):** Automated headless Chrome verification in `e2e-real-npm` validating both `zerho preview` and `zerho dev` on real `react` + `react-dom` + `zustand` + `lucide-react` with 0 uncaught exceptions and real DOM click state updates.
- **Public Base Path (`[build].base`) & Clean Output (`[build].empty_out_dir = true`):** Configurable subpath/relative/CDN base URL (`"/"`, `"/demo/"`, `"./"`, `"https://..."`) applied consistently across `index.html`, dynamic `import()` chunks, shared chunks, Web Workers, CSS `url(...)`, and `.wasm` imports, plus automatic pruning of stale hashed artifacts in `dist/` before each build (with safety guard `out_dir != root`).
- **`package.json` `"sideEffects": false` Tree-Shaking & Barrel Optimization (`crates/zerho-transform/src/barrel.rs`, `crates/zerho-transform/src/cjs.rs`, `crates/zerho-bundler/src/assemble.rs`):** Detects `"sideEffects": false` / `"sideEffects": []` to bypass massive barrel files like `lucide-react` (`2,158 → 28` modules, `1,847 KB → 227 KB` / `64.9 KB` Brotli on a real React 19 + Zustand + Lucide app), prunes dead `process.env.NODE_ENV` CJS `require()` branches (`development.js`), and eliminates dead top-level helper code when zero exports survive tree-shaking.
- **Upgraded Zero-Node `zerho test` (`crates/zerho-cli/src/test_runner.rs`):** Added local module `import` resolution (including `tsconfig.json` paths via `oxc_resolver`), object literals (`{ a: 1 }`), property/index access (`obj.prop`, `arr[0]`), deep `toEqual` comparison, and user-defined functions / arrow functions with lexical closure capture.
- **Precompiled Official WASM Plugins in GitHub Releases (`.github/workflows/release.yml`):** Automatically compiles and attaches `zerho-plugin-{vue-sfc,svelte,solid}.wasm` + `.sha256` digests on every tagged release.
- **Real NPM Ecosystem E2E CI (`e2e-real-npm` job, `crates/zerho-cli/tests/e2e_real_apps.rs`):** Automated end-to-end test installing real `react`, `react-dom`, `zustand`, and `lucide-react` packages from npm and validating `DevServer`, `Bundler` (`base`, `empty_out_dir`, Brotli, SHA-384 SRI, `dist/csp.json`), and `PreviewServer`.
- **Interactive Telemetry Dashboard Showcase (`examples/react-dashboard/`, `https://zerho.xyz/demo/`):** Multi-view Linear-inspired dashboard (`Overview`, `Build Pipeline` with cache filters & crate ledger, `Security & SRI` with SHA-384 & CSP previews) published live at `/demo/` on `zerho.xyz`.

### Changed
- **Modular `zerho-bundler` Architecture:** Refactored `crates/zerho-bundler/src/lib.rs` into dedicated submodules (`assemble.rs`, `html.rs`, `targets.rs`) with zero breaking API changes and `unsafe_code = "forbid"`.

### Fixed
- **Real-Browser React 19 + Zustand + Lucide-React Runtime Compatibility (Chromium CDP Verified):**
  - Lowered `export * from`, `export { a as b } from`, and empty `export {}` in `lower_esm_for_bundle` (`crates/zerho-transform/src/lib.rs`).
  - Injected `process.env.NODE_ENV` (`"production"` in `zerho build` enabling ~460 KB React dev DCE via `oxc_minifier`, `"development"` in `zerho dev`).
  - Added CJS↔ESM default-to-named interop in `__zerho_require` and protected `__zerho_cjs_entry` from `prune_unused_exports` when imported with named ESM syntax (`crates/zerho-bundler/src/assemble.rs`).
  - Unified CommonJS `require()` resolution onto the bundler's shared singleton `__zerho_registry` (`crates/zerho-bundler/src/lib.rs`), preventing duplicate `react` instances across `react-dom`, `zustand`, and `lucide-react`.
  - Upgraded `zerho dev` (`crates/zerho-server/src/lib.rs`, `prebundle_cjs_for_dev` in `crates/zerho-transform/src/cjs.rs`) with singleton external `/@zerho/deps/*` CJS imports, static ESM named exports (`export const createRoot = ...`), Windows `\\?\` canonicalization in `path_to_web_url`, and dev-mode `"sideEffects": false` Barrel Optimization (eliminating 2,130+ HTTP requests on `lucide-react`).
- **Dev Server CSS `@import` Inlining (`crates/zerho-server/src/lib.rs`):** Added `expand_dev_css_imports` in `serve_css_module` and `transform_with_cache` so local `@import "./tokens.css";` rules inline identically in `zerho dev` and `zerho build`.
- **Astro `/demo/` Directory Route (`website/src/pages/demo.astro`):** Replaced `website/public/demo/index.html` with a native Astro route (`is:inline` preserving SHA-384 SRI) so `/demo` and `/demo/` resolve in both `astro dev` and static builds.

## [1.0.0] - 2026-11-24

### Added
- **Public Neutral CI Benchmarks (`.github/workflows/benchmarks.yml`):** Automated `zerho` vs `Vite` comparison on 500 & 1,000 TypeScript modules on `ubuntu-latest`, publishing reproducible JSON artifacts and Markdown summary tables.
- **1-Line Cross-Platform Installers & Packaging (`scripts/install.sh`, `scripts/install.ps1`, `packaging/homebrew/zerho.rb`, `packaging/scoop/zerho.json`):** Verified SHA-256 one-line installers for Linux x64, macOS ARM64/x64, and Windows x64, plus Homebrew tap formula, Scoop manifest, and `x86_64-apple-darwin` added to `release.yml`.
- **Real-App E2E Integration Suite (`crates/zerho-cli/tests/e2e_real_apps.rs`):** Live TCP/HTTP/WebSocket E2E tests exercising `DevServer`, `Bundler`, and `PreviewServer` against React 19 TSX (Fast Refresh, `/@zerho/bundle`, `/__inspect` Treemap, WebSocket HMR `101`, SHA-384 SRI, Brotli, path traversal), Preact TSX, and strict TypeScript + Edge SSR (`dist/server.mjs`).
- **`zerho cache-server` (`crates/zerho-server/src/cache_server.rs`):** Standalone pure-Rust HTTP CAS remote team cache daemon (`--host`, `--port`, `--dir`, `--token` / `ZERHO_CACHE_TOKEN`) with `/health`, `/stats`, strict key validation, and atomic writes compatible with `[cache].remote_url`.
- **Official WASM Framework Plugins (`examples/plugins/{vue-sfc,svelte,solid}/`):** Standalone Rust → `wasm32-unknown-unknown` compiler plugins implementing `zerho:plugin@0.1.0` for Vue SFC (`.vue`), Svelte (`.svelte`), and SolidJS without Node.js.
- **Native Zero-Node Test Runner (`zerho test`, `crates/zerho-cli/src/test_runner.rs`):** Built-in `oxc`-powered TS/JS test runner supporting `describe`, `test`, `it`, and `expect(...)` matchers (`.toBe`, `.toEqual`, `.toBeTruthy`, `.toBeFalsy`, `.toContain`, `.toBeGreaterThan`, `.toBeLessThan`, `.not.*`) with zero Node.js or Vitest.

## [0.9.0] - 2026-11-24

### Added
- **Web Workers & `.wasm` Imports (`crates/zerho-transform/src/workers.rs`, `zerho-bundler`):** Detects `new Worker(new URL('./worker.ts', import.meta.url))` via `oxc` AST, emits dedicated `assets/worker-<hash>.js` chunks, and generates `WebAssembly.instantiateStreaming(fetch(...))` default loaders for imported `.wasm` modules.
- **Full String-Passing WASM Plugin ABI + Example Plugin (`crates/zerho-plugin/src/wasm.rs`, `examples/wasm-plugin/`):** Passes/returns UTF-8 strings across `wasmtime` linear memory (`zerho_alloc`, `zerho_resolve_id`, `zerho_load`, `zerho_transform`) inside a fuel-capped sandbox, accompanied by a standalone Rust → `wasm32-unknown-unknown` plugin example.
- **SSR Build Target (`zerho build --ssr` / `build.ssr = true`):** Emits DOM-free Edge/Server ESM bundles (`dist/server.mjs`) with `import.meta.env.SSR = true` targeting Cloudflare Workers, Deno, Node.js, and Bun.
- **Live Bundle Treemap in `/__inspect` (`crates/zerho-server/src/inspect.html`):** Exposes per-module byte sizes in `/__inspect/api` and renders an interactive filterable Treemap directly inside the dev server DevTools UI.
- **GLASTOR Core DevSecOps CI/CD Suite (`.github/workflows/audit.yml`, `ci.yml`, `release.yml`, `dependabot.yml`):** Coupled SHA-pinned `glastor-core/specter`, `byte-gauge` (with CycloneDX SBOM on releases), `pii-veil`, and `nemesis`, plus Astro website and `wasm32` example build jobs.
- **Developer Experience (`cspell.json`, `scripts/spellcheck.ps1`):** Added bilingual (`en,es`) + Rust/zerho CSpell configuration and PowerShell runner.

### Changed
- **Landing & Docs Portal (`website/`, `https://zerho.xyz`):** Refactored UX/UI (4 interactive capability pillars, unified KPI bar, 3-column benchmarks with CSS comparison bars, 1-click multi-OS install switcher, terminal proof, side-by-side Vite migration diff, sticky 9-crate ledger, complete favicon suite + `isologo.webp`, responsive navbar breakpoints, and `translate="no"` protection for Material Symbols & code).
- **Technical Documentation Audit (`README.md`, `website/src/pages/docs.astro`):** Synced all features, CLI flags (`--ssr`, `--synthetic`, `--json`, `migrate`), `[build.entries]` script paths, and framework support notes 1:1 with the Rust implementation.

### Security
- **Repository & `.env` Hygiene (`.gitignore`, `.github/`):** Blocked `.env` / `**/.env.*` globally (allowing `.env.example`), replaced tracked `examples/basic-ts/.env` with `examples/basic-ts/.env.example`, and aligned all `.github/` governance files, templates, `CITATION.cff`, and `Cargo.toml` metadata to `https://github.com/glastor-core/jester` (`main`).

## [0.8.0] - 2026-11-24

### Added
- **Deep `node_modules` CJS ↔ ESM Pre-Bundler (`crates/zerho-transform/src/cjs.rs`):** AST `require("...")` collector and recursive multi-file CJS graph pre-bundler (`prebundle_cjs_graph_to_esm`) wired into both `zerho-server` (`/@zerho/deps`) and `zerho-bundler` with `redb` caching.
- **CSS `@import` & `url(...)` Asset Rewriting (`crates/zerho-transform/src/css_assets.rs`):** Recursively inlines local `@import "./tokens.css"` rules and rewrites local `url(...)` fonts (`.woff2`) and images (`.svg`/`.png`/`.jpg`/`.webp`) to `blake3`-hashed `/assets/*` URLs with automatic SVG/WebP optimization.
- **Multi-Page Apps (`[build.entries]`):** Supports multiple HTML/script entry points in `zerho.toml`, automatically deduplicating shared modules into `assets/shared-<hash>.js` with `<link rel="modulepreload">` injection per page.
- **Vite Auto-Migrator (`zerho migrate [DIR] [--dry-run] [--force]`):** Non-destructively inspects `package.json`, `vite.config.{ts,mts,js,mjs}`, `index.html`, and `tsconfig.json` to generate `zerho.toml` + `zerho.schema.json`.

## [0.7.0] - 2026-11-23

### Added
- **Native React Fast Refresh:** Enabled `oxc::transformer::ReactRefreshOptions` for React `.tsx`/`.jsx` in dev mode (`$RefreshReg$` / `$RefreshSig$`) with automatic HMR boundary wrapping in `zerho-server`.
- **Library Mode (`[build.lib]`) & `zerho build --watch`:** Emit reusable `.mjs` (ESM) and `.cjs` (CommonJS) bundles with Brotli/Gzip/SRI, plus incremental file-watching rebuilds powered by `notify` and `redb`.
- **Pure-Rust Image Optimization (`[assets].optimize_images`):** Automatic PNG/JPEG → WebP conversion via `image` crate (zero `sharp`, zero C bindings) when WebP produces a smaller output.
- **Verified `--release` Benchmarks (v0.7.0, 16.60 MB binary, AMD Zen 3 / 16 threads / Windows 11 / `rustc 1.96.1`):** `0.0017 ms` (1.7 µs) warm `redb` cache hits, `0.548 ms` cold TSX transform, `19.32 ms` warm / `43.04 ms` cold build (`basic-ts`, 3 mods), and `763.53 ms` warm / `1,254 ms` cold on a 500-module synthetic TypeScript graph (`benchmarks/results.json`).

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
