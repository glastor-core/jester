## Descripción del Cambio

<!-- Explica de forma concisa qué problema resuelve este Pull Request o qué funcionalidad añade/optimiza en GLASTOR® zerho (glastor-core/jester). -->

## Tipo de Cambio

- [ ] Bug fix (corrección en resolver, transform, HMR, bundler, SSR o seguridad)
- [ ] Nueva característica (nueva capacidad del compilador, bundler, CLI, plugin WASM o DevTools)
- [ ] Breaking change (cambio que altera `zerho.toml`, el CLI, traits públicos o `zerho:plugin@0.1.0`)
- [ ] Optimización de rendimiento (mejora en `oxc`, `rayon`, caché `redb`, Scope Hoisting o Brotli)
- [ ] Documentación / Website (actualización de `README.md`, `CHANGELOG.md`, `ROADMAP.md` o `website/`)
- [ ] Mantenimiento / CI / Tooling (refactorización, Clippy, workflows de GitHub Actions)

## Issues Relacionados

<!-- Vincula los issues afectados, por ejemplo: Fixes #123, Closes #456 -->

- Fixes #

## Pruebas Realizadas

<!-- Marca las verificaciones ejecutadas localmente antes de enviar el PR: -->

- [ ] Formateo estricto: `cargo fmt --all -- --check`
- [ ] Linter Clippy pedantic sin advertencias: `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Suite de pruebas de Rust completa: `cargo test --workspace`
- [ ] Smoke test de build / SSR / bench: `cargo run --release -p zerho-cli -- check --config examples/basic-ts/zerho.toml`
- [ ] Build del sitio web Astro (si se modificó `website/`): `npm --prefix website run build`

## Checklist de Protección de Rama (`main`)

- [ ] Mi rama parte de la versión más reciente de `main`.
- [ ] Se mantiene `unsafe_code = "forbid"` en los 9 crates del workspace.
- [ ] Ningún test o ejemplo incluye secretos o credenciales reales.
- [ ] Los mensajes de commit cumplen con [COMMIT_CONVENTION.md](./COMMIT_CONVENTION.md).
- [ ] He actualizado `CHANGELOG.md` si corresponde.

---

© 2026 GLASTOR Core Organization — Security & Infrastructure Team.
