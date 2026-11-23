# Contributing to zerho

Thank you for your interest in contributing to **zerho** — the Zero-Node, 100% Rust frontend dev server and production bundler.

## Development Requirements

- Rust **1.85+** (Edition 2024)
- No Node.js required

## Quality Gates

Before opening a pull request, ensure all checks pass locally:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Architecture Conventions

- `unsafe_code = "forbid"` is enforced workspace-wide.
- Keep crate boundaries clean (`zerho-core` has minimal dependencies; transform logic lives in `zerho-transform`; server/HMR lives in `zerho-server`; production bundling lives in `zerho-bundler`).
- Follow Conventional Commits (`feat:`, `fix:`, `perf:`, `docs:`, `refactor:`, `test:`).
