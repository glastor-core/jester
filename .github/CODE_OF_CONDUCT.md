# Governance Protocol & Code of Conduct

## 1. DX Standards & Engineering Excellence

`GLASTOR® zerho` (maintained by **GLASTOR Core** at `https://github.com/glastor-core/jester`) is a Zero-Node, 100% Safe Rust frontend dev server and production bundler built on `oxc`, `lightningcss`, `redb`, `blake3`, and `wasmtime`. Our goal is to deliver native-speed frontend tooling with an exceptional Developer Experience (DX) and strict memory safety (`unsafe_code = "forbid"` across all 9 workspace crates). This protocol ensures our collaboration remains as predictable, rigorous, and disciplined as our technology stack.

## 2. Pillars of Sustainable Software Engineering

To uphold the integrity of our codebase, architecture, and community, all participants adhere to the following standards:

### High-Impact Collaboration

- **Technical Empathy & Peer Review:** We conduct code reviews and architecture discussions with rigorous technical standards and genuine professional courtesy. We prioritize memory safety, build determinism, and correctness over author ego.
- **Transparent Accountability:** We acknowledge compiler edge cases, resolver bugs, bundler regressions, or documentation gaps immediately, transforming them into opportunities for regression tests and improvements.
- **Data-Driven Feedback:** All performance or architecture proposals must be grounded in reproducible benchmarks (`zerho bench --json`), AST/graph regression tests (`cargo test --workspace`), or web platform specifications (ESM, Web Workers, CSS Modules, WIT Component Model), avoiding subjective bias.

### Technical & Social Boundaries (Zero Tolerance)

- **Harassment & Toxicity:** Any behavior or communication that compromises psychological safety or mutual respect will be addressed and mitigated immediately.
- **Disruption & Trolling:** Bad-faith argumentation, disruption of issue trackers, or trolling in PR discussions degrades project velocity and will not be tolerated.
- **Privacy & Secret Hygiene:** Never post real personal data, live API keys, or production credentials in issues, PRs, or example fixtures. Exposing real personal data results in immediate revocation of repository access.

## 3. Governance & Maintainer Responsibilities

The maintainers of **GLASTOR® zerho** (`@glastor-core` and `@glastor-dev`) serve as the architecture and moderation council. Maintainers have the authority and responsibility to:

- Provide impartial mediation during technical disagreements and architectural reviews.
- Reject contributions that fail to meet quality gates (`unsafe_code = "forbid"`, `clippy::pedantic -D warnings`, `cargo fmt --all -- --check`, 100% test pass) or conduct standards.
- Enforce appropriate corrective measures aligned with the long-term health and stability of the repository.

## 4. Scope

This code of conduct applies within all project spaces (`https://github.com/glastor-core/jester`, `https://zerho.xyz`) and in any public representation of the project (conferences, technical forums, crates.io, and official communications).

## 5. Incident Response Protocol (IRP)

If you witness or experience unacceptable behavior:
1. Privately contact our team at: **[founder@glastor.es](mailto:founder@glastor.es)**.
2. All reports are evaluated confidentially within 48 business hours.
3. Resolutions will be applied with discretion to protect reporting parties.

---

© 2026 GLASTOR Core Organization — Security & Infrastructure Team.
