## 📋 Descripción del Cambio

<!-- Explica de forma concisa qué problema resuelve este Pull Request o qué funcionalidad añade/optimiza en GLASTOR® pii-veil. -->

## 🎯 Tipo de Cambio

- [ ] 🐛 Bug fix (corrección de falso positivo/negativo, streaming SQL/CSV o proxy SSE)
- [ ] ✨ Nueva característica (nuevo detector PII/secreto, herramienta MCP, subcomando CLI o binding)
- [ ] 💥 Breaking change (cambio que altera interfaces públicas de `VeilEngine`, `Vault`, `veil mcp` o `pii_veil.h`)
- [ ] ⚡ Optimización de rendimiento (mejora de latencia en `RegexSet`, `AhoCorasick` o `StreamingRestorer`)
- [ ] 📝 Documentación (actualización de `README.md`, `CHANGELOG.md`, ejemplos o doctests)
- [ ] 🔧 Mantenimiento / CI / Tooling (refactorización, Clippy, workflows de GitHub Actions)

## 🔗 Issues Relacionados

<!-- Vincula los issues afectados, por ejemplo: Fixes #123, Closes #456 -->

- Fixes #

## 🧪 Pruebas Realizadas

<!-- Marca las verificaciones ejecutadas localmente antes de enviar el PR: -->

- [ ] Formateo estricto: `cargo fmt --all -- --check`
- [ ] Linter Clippy sin advertencias: `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Suite de pruebas de Rust completa: `cargo test --workspace`
- [ ] Suite de pruebas del SDK de Go + puente MCP: `cd sdks/go && go test -v -race ./...`
- [ ] Benchmark y demo verificados: `cargo run --release -p pii-veil-cli -- bench`

## 🛡 Checklist de Protección de Rama (`main`)

- [ ] Mi rama parte de la versión más reciente de `main`.
- [ ] Ningún test o ejemplo incluye datos personales (PII) o credenciales reales.
- [ ] Si se modificó `crates/veil-ffi`, todo bloque `unsafe` valida punteros nulos y documenta `# Safety`.
- [ ] Los mensajes de commit cumplen con [COMMIT_CONVENTION.md](./COMMIT_CONVENTION.md).
- [ ] He actualizado el archivo `CHANGELOG.md` si corresponde.

---

© 2026 GLASTOR Core Organization — Security & Infrastructure Team.
