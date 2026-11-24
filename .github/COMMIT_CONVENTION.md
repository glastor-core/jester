# Convención de Mensajes de Commit (Conventional Commits) — GLASTOR® zerho

Para escalar el desarrollo y garantizar una **Software Supply Chain** transparente en `https://github.com/glastor-core/jester`, este repositorio implementa una convención estricta basada en el estándar [Conventional Commits 1.0.0](https://www.conventionalcommits.org/).

Esta práctica impulsa la automatización de versiones con SemVer, la generación de changelogs (`CHANGELOG.md`), la validación AST de **GLASTOR® Specter** y los releases multiplataforma.

---

## Estructura del Mensaje

```text
<tipo>(<scope>): <asunto en minúsculas y modo imperativo>

[cuerpo descriptivo opcional]

[pie de página / referencias opcional]
```

### 1. Tipos de Commit Permitidos

| Tipo | Propósito | Impacto en Release |
| :--- | :--- | :---: |
| `feat` | Nueva capacidad del compilador, bundler, dev server, CLI o plugin WASM | Minor (`v0.X.0`) |
| `fix` | Corrección de bug en resolución, HMR, bundling, SSR o seguridad | Patch (`v0.0.X`) |
| `perf` | Optimización en `oxc`, `rayon`, caché `redb`, Scope Hoisting o Brotli | Patch (`v0.0.X`) |
| `refactor` | Reestructuración interna sin alterar el comportamiento público | Ninguno |
| `docs` | Actualización de `README.md`, `CHANGELOG.md`, `ROADMAP.md` o `website/` | Ninguno |
| `test` | Nuevas pruebas unitarias o de integración en el workspace Rust | Ninguno |
| `ci` | Ajustes en GitHub Actions (`ci.yml`, `audit.yml`, `release.yml`) o Dependabot | Ninguno |
| `chore` | Tareas de mantenimiento, dependencias o metadatos | Ninguno |

### 2. Scopes Oficiales de `zerho`

- `core`: Tipos base (`ModuleId`, `ContentHash`, `Framework`, errores) en `crates/zerho-core/`.
- `config`: Parser declarativo `zerho.toml`, validación y JSON Schema en `crates/zerho-config/`.
- `resolver`: Resolución ESM/CJS y `tsconfig.json` `paths` en `crates/zerho-resolver/`.
- `graph`: Grafo incremental, ciclos DFS, caché `redb` y caché remoto HTTP en `crates/zerho-graph/`.
- `plugin`: Trait `Plugin`, host `wasmtime` con ABI UTF-8 y contrato WIT en `crates/zerho-plugin/`.
- `transform`: Compilador `oxc`, minifier, Scope Hoisting, Barrel/CJS, Web Workers y CSS/SVG en `crates/zerho-transform/`.
- `server`: Dev server `axum`, WebSocket HMR, `/@zerho/bundle`, `/__inspect` y `PreviewServer` en `crates/zerho-server/`.
- `bundler`: Bundler paralelo `rayon`, chunks compartidos, MPA, SSR Edge, WebP, SRI/CSP y `stats.html` en `crates/zerho-bundler/`.
- `cli`: Binario `zerho` (`dev`, `build`, `migrate`, `check`, `preview`, `bench`, `init`) en `crates/zerho-cli/`.
- `website`: Portal y documentación Astro 5 (`https://zerho.xyz`) en `website/`.

---

## Ejemplos Válidos

```text
feat(bundler): emit DOM-free dist/server.mjs bundle for Edge SSR targets
fix(resolver): resolve nested tsconfig paths with wildcard suffixes
perf(graph): batch redb write transactions during parallel module ingestion
feat(plugin): pass UTF-8 strings across wasmtime linear memory for transform hooks
docs(website): sync v0.9.0 comparison table and Vite migration guide
```

---

© 2026 GLASTOR Core Organization — Security & Infrastructure Team.
