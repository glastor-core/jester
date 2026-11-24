# zerho Roadmap

> **Current verified baseline:** `v0.8.0` — 100% Safe Rust (`unsafe_code = "forbid"`), **56/56 tests passing**, **100/100 Vektor repo hygiene**, deep CJS↔ESM pre-bundler, CSS `@import`/`url(...)` asset rewriting, Multi-Page Apps (`[build.entries]`), and `zerho migrate` from Vite.

---

## Completed Foundation (`v0.1.0` – `v0.7.0`)

- [x] **Core Zero-Node Rust Architecture:** 9-crate workspace, `oxc`, `lightningcss`, `oxc_resolver`, `axum`/`tokio`/`notify`.
- [x] **Production Compiler:** Scope Hoisting (`oxc::semantic`), `oxc_minifier` + `oxc_mangler`, two-pass symbol tree-shaking, Source Maps v3, Shared Chunks + `<link rel="modulepreload">`, Library Mode (`[build.lib]`), `zerho build --watch`.
- [x] **Caching & Parallelism:** Embedded ACID `redb` cache (`zerho-cache.redb`), HTTP remote team cache (`cache.remote_url`/`remote_token`), `rayon` wave-parallel bundler.
- [x] **Security, Budgets & Optimization:** SHA-384 SRI, strict `dist/csp.json`, Secret Leak Guard, Gzip `[build.budgets]`, pure-Rust Brotli `.br` + `PreviewServer` negotiation, pure-Rust SVG & PNG/JPG → WebP optimizers, hardened SHA-pinned GitHub Actions (100/100).
- [x] **DX & Editor Support:** Granular `import.meta.hot`, native `oxc` React Fast Refresh, Shadow DOM Error Overlay, Dev Partial Bundling (`/@zerho/bundle`), `/__inspect`, native `tsconfig.json` `paths`, Barrel File Optimizer, `zerho check`, `dist/stats.html` Bundle Analyzer, `zerho init` (`zerho.schema.json`, `tsconfig.json`, `src/zerho-env.d.ts`), declarative `[plugins].wasm` (`wasmtime v29`), and reproducible `zerho bench` (`--json`, `--synthetic <N>`).

---

## `v0.8.0` — Real-World npm Ecosystem & Large App Parity *(Completed)*

- [x] **Deep `node_modules` CJS ↔ ESM Pre-Bundler:** Handle multi-file CommonJS packages (`react-dom/client`, `zustand`, `lucide-react`, `date-fns`) with nested `require()` resolution cached in `redb`.
- [x] **CSS `@import` & `url(...)` Asset Rewriting:** Resolve `@import "./tokens.css"`, `.woff2` fonts, and CSS background images with `blake3` hashing and automatic SVG/WebP optimization.
- [x] **Multi-Page Apps (MPA):** Support multiple HTML/script entry points in `zerho.toml` (`[build.entries]`) with automatic shared chunk deduplication.
- [x] **Vite Auto-Migrator (`zerho migrate`):** Inspect `vite.config.*`, `package.json`, and `tsconfig.json` to generate a ready-to-run `zerho.toml` in seconds.

---

## `v0.9.0` — Modern Web Platform & Full WASM Component Model

- [ ] **Web Workers & `.wasm` Imports:** Detect `new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' })` and `import init from './mod.wasm'` and emit dedicated hashed chunks automatically.
- [ ] **Full WASM Component Model (`wasmtime` `component-model`):** Pass/return strings across `resolve-id`, `load`, and `transform` hooks for `zerho:plugin@0.1.0`, plus a complete Rust → `.wasm` example plugin in `examples/wasm-plugin/`.
- [ ] **SSR Build Target (`zerho build --ssr`):** Emit DOM-free server bundles targeting Edge runtimes (Cloudflare Workers, Deno, Node, Bun).
- [ ] **Live Treemap in `/__inspect`:** Embed the interactive bundle treemap directly inside the dev server `/__inspect` DevTools UI.

---

## `v1.0.0` — Public Launch, Distribution & Neutral Benchmarks

- [ ] **Public Neutral CI Benchmarks (`benchmarks.yml`):** Automated `zerho` vs `Vite` comparison (500 & 1,000 modules) on GitHub `ubuntu-latest` runners publishing reproducible JSON snapshots.
- [ ] **1-Line Cross-Platform Installers:** Signed GitHub Releases (`.sha256`) for Windows, macOS (ARM64/x64), and Linux + `cargo install zerho`, Homebrew tap, and Winget/Scoop manifests.
- [ ] **Real-App E2E Suite:** Automated integration tests exercising `zerho dev` and `zerho preview` against real React 19, Preact, and strict TypeScript apps.
- [ ] **Public Docs & Landing Deploy:** Publish `website/` on Cloudflare Pages with a *"Migrate from Vite to zerho in 2 minutes"* guide.

---

## Post-`v1.0` — Horizon

- [ ] **`zerho cache-server`:** Lightweight pure-Rust HTTP/S3 remote team cache daemon ready for Docker/Kubernetes (`cache.remote_url`).
- [ ] **Official WASM Framework Plugins:** Vue SFC, Svelte, and SolidJS compiled transforms via `zerho:plugin@0.1.0` without Node.js.
- [ ] **Integrated Native Test Runner (`zerho test`):** Ultra-fast built-in test runner without Vitest or Node.
