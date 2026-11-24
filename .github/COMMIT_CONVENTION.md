# 🏛 Convención de Mensajes de Commit (Conventional Commits) — GLASTOR® pii-veil

Para escalar el desarrollo y garantizar una **Software Supply Chain** transparente, este repositorio implementa una convención estricta basada en el estándar [Conventional Commits 1.0.0](https://www.conventionalcommits.org/).

Esta práctica impulsa la automatización de versiones con SemVer, la generación de changelogs (`CHANGELOG.md`) y la publicación coordinada en `crates.io`, `PyPI`, `npm` y el módulo de `Go`.

---

## ⚙ Estructura del Mensaje

```text
<tipo>(<scope>): <asunto en minúsculas y modo imperativo>

[cuerpo descriptivo opcional]

[pie de página / referencias opcional]
```

### 1. Tipos de Commit Permitidos

| Tipo | Propósito | Impacto en Release |
| :--- | :--- | :---: |
| `feat` | Nuevo detector PII/secreto, subcomando CLI, herramienta MCP o binding | Minor (`v0.X.0`) |
| `fix` | Corrección de falso positivo/negativo, bug en streaming o proxy SSE | Patch (`v0.0.X`) |
| `perf` | Optimización de pre-filtrado `RegexSet`, `AhoCorasick` o latencia de Vault | Patch (`v0.0.X`) |
| `refactor` | Reestructuración interna sin alterar el comportamiento público | Ninguno |
| `docs` | Actualización de `README.md`, `CHANGELOG.md`, ejemplos o comentarios doc | Ninguno |
| `test` | Nuevas pruebas unitarias o de integración en Rust, Go, Python o Node | Ninguno |
| `ci` | Ajustes en GitHub Actions (`ci.yml`, `release.yml`) o Dependabot | Ninguno |
| `chore` | Tareas de mantenimiento, dependencias o metadatos de empaquetado | Ninguno |

### 2. Scopes Oficiales de pii-veil

- `core`: Motor principal `VeilEngine`, sanitización JSON y orquestación (`crates/veil-core/src/lib.rs`).
- `detector`: Detectores regex, pre-filtro `RegexSet` y validadores Luhn/Mod-97/Mod-23/Mod-11 (`crates/veil-core/src/detector.rs`).
- `strategy`: Generadores `Placeholder`, `Synthetic` determinista y `Mask` (`crates/veil-core/src/strategy.rs`).
- `vault`: Bóveda bidireccional, `StreamingRestorer` para SSE y cifrado autenticado (`crates/veil-core/src/vault.rs`).
- `stream`: Procesador en memoria constante $O(1)$ para `Lines`, `Ndjson`, `Sql` y `Csv` (`crates/veil-core/src/stream.rs`).
- `policy`: Motor declarativo `VeilPolicy` y reglas personalizadas (`crates/veil-core/src/policy.rs`).
- `cli`: Binario `veil` y subcomandos de terminal (`crates/veil-cli/src/main.rs`).
- `mcp`: Servidor Model Context Protocol sobre JSON-RPC 2.0 STDIO (`crates/veil-cli/src/mcp.rs`).
- `proxy`: Proxy reverso HTTP/SSE local (`crates/veil-cli/src/proxy.rs`).
- `ffi`: Capa C-ABI y cabecera `pii_veil.h` (`crates/veil-ffi/`).
- `py`: Bindings de Python vía `PyO3` y `maturin` (`crates/veil-py/`).
- `node`: Bindings de Node.js/TypeScript vía `napi-rs` (`crates/veil-node/`).
- `go`: SDK nativo de Go, middleware `net/http` y `McpBridge` (`sdks/go/`).

---

## 💡 Ejemplos Válidos

```text
feat(detector): add Brazilian CPF Mod-11 mathematical checksum validator
fix(proxy): preserve empty event separator lines during SSE JSON delta restoration
perf(core): pre-filter custom rules alongside built-in RegexSet automaton
feat(mcp): expose veil_scan_file tool over JSON-RPC 2.0 stdio server
docs(examples): add FastAPI SSE streaming and Go net/http middleware guides
```

---

© 2026 GLASTOR Core Organization — Security & Infrastructure Team.
