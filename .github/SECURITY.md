# Security Policy — GLASTOR® zerho

At **GLASTOR Core** (`glastor-core/jester`), memory safety, build integrity (SRI/CSP), WASM plugin sandboxing, and supply-chain security are core priorities.

All 9 workspace crates of `zerho` are implemented in **100% Safe Rust** (`unsafe_code = "forbid"`, `clippy::pedantic -D warnings`), and third-party plugins execute inside fuel-metered `wasmtime` sandboxes rather than unrestricted Node.js processes.

---

## Supported Versions

We provide active security updates and patches for current releases of `zerho`:

| Version | Status | Notes |
| :--- | :--- | :--- |
| `0.9.x` | Supported | Active maintenance & immediate security patches |
| `< 0.9` | Unsupported | Previous pre-release milestones |

---

## Reporting a Vulnerability

If you discover a security vulnerability affecting `zerho` (e.g., path traversal in `zerho-server` / `PreviewServer`, HTTP proxy CRLF/header injection, WASM plugin sandbox escape or fuel exhaustion bypass, SRI/CSP generation flaw, or secret-scan evasion in `zerho-bundler`), please **do not open a public issue**. Instead, follow our coordinated vulnerability disclosure process:

### Option 1: GitHub Private Vulnerability Reporting (Recommended)

Submit a confidential report directly via GitHub Security Advisories:
- [Report a private vulnerability in zerho (glastor-core/jester)](https://github.com/glastor-core/jester/security/advisories/new)

### Option 2: Direct Email Disclosure

Send technical details directly to our security and engineering desk:
- **[founder@glastor.es](mailto:founder@glastor.es)**

### Details to Include

To accelerate triage and resolution, please provide:
- Clear description of the vulnerability and attack vector.
- Minimal reproducible proof of concept (PoC) using **synthetic test fixtures** (never submit real credentials or live secrets).
- Affected crate(s) (`zerho-server`, `zerho-bundler`, `zerho-plugin`, `zerho-resolver`, `zerho-transform`, `zerho-config`, `zerho-cli`).
- Suggested remediation or patch (if available).

---

## Response SLA & Commitment

- **Initial Acknowledgment:** <= 24 business hours.
- **Triage & Severity Assessment (CVSS):** <= 48 business hours.
- **Security Patch Release:** Immediate priority via GitHub Security Advisory and tagged GitHub Release with SHA-256 checksums and CycloneDX SBOM.

---

© 2026 GLASTOR Core Organization — Security & Infrastructure Team.
