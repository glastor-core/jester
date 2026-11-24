# Security Policy — GLASTOR Core pii-veil

At **GLASTOR Core** (`glastor-core/pii-veil`), data loss prevention (DLP), cryptographic integrity of reversible vaults (`Vault`), and supply-chain security are paramount priorities.

The core engine (`crates/veil-core`) and CLI/MCP/Proxy binary (`crates/veil-cli`) are implemented in **100% safe Rust** (no `unsafe` blocks), eliminating memory corruption vulnerabilities and catastrophic regular expression backtracking (ReDoS) through linear-time deterministic finite automata (`regex` and `aho-corasick`).

---

## Supported Versions

We provide active security updates and patches for stable releases of `pii-veil` across all distribution channels (`crates.io`, `PyPI`, `npm`, and `Go`):

| Version | Status | Notes |
| :--- | :--- | :--- |
| `0.1.x` | Supported | Active maintenance & immediate security patches |
| `< 0.1` | Unsupported | Pre-release development versions |

---

## Reporting a Vulnerability

If you discover a security vulnerability or bypass affecting `pii-veil` (e.g. DLP redaction evasion in the HTTP/SSE proxy or MCP server, cryptographic weakness in authenticated `.vault` encryption with `--vault-pass`, token leakage in `StreamingRestorer`, or memory flaws at the C-ABI boundary in `crates/veil-ffi`), please **do not open a public issue**. Instead, follow our coordinated vulnerability disclosure process:

### Option 1: GitHub Private Vulnerability Reporting (Recommended)

Submit a confidential report directly via GitHub Security Advisories:
👉 [Report a private vulnerability in pii-veil](https://github.com/glastor-core/pii-veil/security/advisories/new)

### Option 2: Direct Email Disclosure

Send technical details directly to our security and engineering desk:
📧 **[founder@glastor.es](mailto:founder@glastor.es)**

### Details to Include

To accelerate triage and resolution, please provide:
- Clear description of the vulnerability, DLP bypass vector, or cryptographic concern.
- Minimal reproducible proof of concept (PoC) using **synthetic mock data** (never submit real PII or live secrets).
- Affected components (`veil-core`, `veil mcp`, `veil proxy`, `veil-ffi`, `veil-py`, `veil-node`, or `sdks/go`).
- Suggested remediation or patch (if available).

---

## Response SLA & Commitment

- **Initial Acknowledgment:** <= 24 business hours.
- **Triage & Severity Assessment (CVSS):** <= 48 business hours.
- **Security Patch Release:** Immediate priority via GitHub Security Advisory, synchronized across `crates.io`, `PyPI`, and `npm`.

---

© 2026 GLASTOR Core Organization — Security & Infrastructure Team.
