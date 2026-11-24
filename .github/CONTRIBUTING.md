# Contributing to GLASTOR Core pii-veil

Thank you for your interest in contributing to **GLASTOR Core pii-veil**! We build ultrafast, deterministic, and reversible data loss prevention (DLP) engines, MCP servers, and privacy proxies in Rust, WebAssembly, Go, Python, and Node.js.

This document provides clear engineering guidelines to ensure smooth, high-signal collaboration.

---

## Code of Conduct & Governance

- All contributors are expected to uphold our [Code of Conduct](CODE_OF_CONDUCT.md). Please report any violations confidentially to [`founder@glastor.es`](mailto:founder@glastor.es).
- For security vulnerabilities or DLP redaction bypasses, **never open a public issue**—follow our [Security Policy](SECURITY.md).
- Review our [Support Guidelines](SUPPORT.md) for architecture discussions and community forums.

---

## Finding Work, Reporting Bugs & Proposing Changes

- **First-Time Contributors:** Look for issues labeled `good first issue` or `help wanted`. Comment on the issue to claim it before starting work.
- **Reporting Bugs:** Open an issue with exact reproduction steps, operating system, `veil --version`, and expected vs. actual behavior. **Never include real PII, credentials, or production secrets in issues**—always use synthetic mock data.
- **Architecture RFCs & New Detectors:** Before starting large refactors, adding regional ID checksum algorithms, or altering public APIs, open a **GitHub Discussion** first to align on design and scope with maintainers.

---

## Development Workflow

We follow a structured Git branching model and enforce linear history via GitHub Pull Requests:

```text
main (production-ready, releases)
  └── feat/your-feature-name
  └── fix/issue-description
  └── perf/optimization-name
```

### 1. Prerequisites
- **Rust:** Stable 1.80+ (`rustup`, `cargo`, `rustfmt`, `clippy`).
- **Go** *(optional, for `sdks/go`)*: 1.22+.
- **Node.js** *(optional, for `web/` and `crates/veil-node`)*: 20 LTS+.
- **Python** *(optional, for `crates/veil-py`)*: 3.9+ with `maturin`.

### 2. Fork, Clone & Branch

```bash
git clone https://github.com/<your-username>/pii-veil.git
cd pii-veil
git remote add upstream https://github.com/glastor-core/pii-veil.git
git checkout -b feat/my-detector-enhancement
```

### 3. Local Verification & Quality Gates

All contributions must pass local quality gates prior to opening a PR:

```bash
# Build the complete Rust workspace
cargo build --workspace --all-targets

# Run the complete test suite (including multi-thousand iteration fuzz smoke test)
cargo test --workspace

# Verify strict formatting
cargo fmt --all -- --check

# Enforce zero Clippy warnings
cargo clippy --workspace --all-targets -- -D warnings

# Run Go SDK tests (if touching sdks/go)
cd sdks/go && go test -v -race ./...
```

---

## Commit Message Conventions

We enforce [Conventional Commits](COMMIT_CONVENTION.md) to ensure automated changelog generation and SemVer tracking:

- `feat(detector)`: Add new entity detector or checksum validator
- `fix(vault)`: Fix token reconstitution or vault boundary issue
- `perf(core)`: Optimize Aho-Corasick automaton or SIMD scanning
- `docs(web)`: Documentation, architecture whitepapers, or landing page updates
- `test(fuzz)`: Fuzzing targets or edge-case regression suites
- `refactor(cli)`: Internal restructuring without API change

---

## Pull Request Submission Checklist

Before requesting review from maintainers ([CODEOWNERS](CODEOWNERS)):

- [ ] Branch is rebased against the latest `upstream/main`.
- [ ] No real PII, tokens, or live credentials exist in any test fixture or commit.
- [ ] `cargo fmt --all -- --check` passes cleanly.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` produces zero warnings.
- [ ] All tests pass: `cargo test --workspace`.
- [ ] Specter contract verification: public API signatures are backward-compatible.
- [ ] Documentation updated if public structs, flags, or configuration options changed.

---

© 2026 GLASTOR Core Organization — Security & Infrastructure Team.
