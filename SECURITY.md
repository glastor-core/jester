# Security Policy

## Supported Versions

| Version | Supported |
| :--- | :--- |
| `0.1.x` | Yes |

## Security Architecture

- **Memory safety:** `unsafe_code = "forbid"` across all `zerho-*` workspace crates.
- **Path traversal protection:** Both `DevServer` and `PreviewServer` canonicalize requested paths and enforce strict containment within the project root or `dist` directory.
- **Sandboxed WASM plugins:** WebAssembly plugins execute inside `wasmtime` with fuel metering (`consume_fuel(true)`) enabled.
- **Secret isolation:** `.env` loading only exposes variables matching `project.env_prefix` (default `ZERHO_`).

## Reporting a Vulnerability

Please report security issues privately via GitHub Security Advisories rather than opening a public issue.
