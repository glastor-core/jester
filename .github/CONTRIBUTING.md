# Contributing to GLASTOR® zerho

Thank you for your interest in contributing to **GLASTOR® zerho** (`https://github.com/glastor-core/jester`)! We build a Zero-Node, 100% Safe Rust frontend dev server and production bundler powered by `oxc`, `lightningcss`, `redb`, `blake3`, and `wasmtime`.

This document provides clear engineering guidelines to ensure smooth, high-signal collaboration.

---

## Code of Conduct & Governance

- All contributors are expected to uphold our [Code of Conduct](CODE_OF_CONDUCT.md). Please report any violations confidentially to [`founder@glastor.es`](mailto:founder@glastor.es).
- For security vulnerabilities (path traversal, WASM sandbox escapes, SRI/CSP bypasses, secret-scan evasion), **never open a public issue**—follow our [Security Policy](SECURITY.md).
- Review our [Support Guidelines](SUPPORT.md) for architecture discussions and community forums.

---

## Finding Work, Reporting Bugs & Proposing Changes

- **First-Time Contributors:** Look for issues labeled `good first issue` or `help wanted`. Comment on the issue to claim it before starting work.
- **Reporting Bugs:** Open an issue with minimal reproduction steps, operating system, `zerho --version`, and expected vs. actual behavior. **Never include real credentials or production secrets in issues.**
- **Architecture RFCs:** Before starting large refactors or altering public traits/WIT contracts (`zerho:plugin@0.1.0`), open a **GitHub Discussion** first to align on design and scope.

---

## Development Workflow

We follow a structured Git branching model and enforce linear history via GitHub Pull Requests against `main`:

```text
main (production-ready, releases)
  └── feat/your-feature-name
  └── fix/issue-description
  └── perf/optimization-name
```

### 1. Prerequisites
- **Rust:** Stable **1.95+** (Edition 2024, `rustup`, `cargo`, `rustfmt`, `clippy`).
- **WASM target** *(optional, for `examples/wasm-plugin`)*: `rustup target add wasm32-unknown-unknown`.
- **Node.js** *(optional, only for building the Astro landing site in `website/`)*: 22 LTS+.

### 2. Fork, Clone & Branch

```bash
git clone https://github.com/<your-username>/jester.git
cd jester
git remote add upstream https://github.com/glastor-core/jester.git
git checkout -b feat/my-bundler-enhancement
```

### 3. Local Verification & Quality Gates

All contributions must pass local quality gates prior to opening a PR:

```bash
# Verify strict formatting
cargo fmt --all -- --check

# Enforce zero Clippy pedantic warnings (unsafe_code = "forbid")
cargo clippy --workspace --all-targets -- -D warnings

# Run the complete workspace test suite (60+ unit & integration tests)
cargo test --workspace

# Build release binary & run reproducible benchmark smoke test
cargo build --release -p zerho-cli
./target/release/zerho bench --synthetic 20 --json

# Build Astro website (if touching website/)
npm --prefix website run build
```

---

## Commit Message Conventions

We enforce [Conventional Commits](COMMIT_CONVENTION.md) for automated changelog generation, SemVer tracking, and **GLASTOR® Specter** verification:

- `feat(bundler)`: New bundler capability (SSR, MPA, Web Workers, `.wasm`)
- `fix(server)`: Fix HMR invalidation, proxy, or `/__inspect` issue
- `perf(transform)`: Optimize `oxc` transforms, Scope Hoisting, or CJS pre-bundling
- `docs(website)`: Documentation or `zerho.xyz` landing page updates
- `ci`: GitHub Actions or Dependabot updates

---

## Pull Request Submission Checklist

Before requesting review from maintainers ([CODEOWNERS](CODEOWNERS)):

- [ ] Branch is rebased against the latest `upstream/main`.
- [ ] Zero `unsafe` blocks in any of the 9 workspace crates (`unsafe_code = "forbid"`).
- [ ] `cargo fmt --all -- --check` passes cleanly.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` produces zero warnings.
- [ ] All tests pass: `cargo test --workspace`.
- [ ] Specter contract verification: public API signatures are backward-compatible.
- [ ] Documentation / `CHANGELOG.md` updated if CLI flags or `zerho.toml` options changed.

---

© 2026 GLASTOR Core Organization — Security & Infrastructure Team.
