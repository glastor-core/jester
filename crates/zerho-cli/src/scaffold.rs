use std::fs;
use std::path::{Path, PathBuf};
use zerho_config::ZerhoConfig;
use zerho_core::{ZerhoError, ZerhoResult};

/// Available built-in zero-Node project templates for `zerho init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Template {
    #[default]
    BasicTs,
    ReactTsx,
}

impl Template {
    pub fn parse(raw: &str) -> ZerhoResult<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "basic-ts" | "ts" | "vanilla" => Ok(Self::BasicTs),
            "react-tsx" | "react" | "tsx" => Ok(Self::ReactTsx),
            other => Err(ZerhoError::Config(format!(
                "unknown template `{other}` (expected: basic-ts, react-tsx)"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::BasicTs => "basic-ts",
            Self::ReactTsx => "react-tsx",
        }
    }
}

/// Scaffolds a complete Zero-Node frontend project into `target_dir`.
pub fn scaffold_project(target_dir: &Path, template: Template) -> ZerhoResult<Vec<PathBuf>> {
    let config_path = target_dir.join("zerho.toml");
    if config_path.exists() {
        return Err(ZerhoError::Config(format!(
            "`{}` already exists",
            config_path.display()
        )));
    }

    let project_name = target_dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && *n != ".")
        .unwrap_or("zerho-app")
        .to_owned();

    let mut created = Vec::new();
    match template {
        Template::BasicTs => scaffold_basic_ts(target_dir, &project_name, &mut created)?,
        Template::ReactTsx => scaffold_react_tsx(target_dir, &project_name, &mut created)?,
    }
    Ok(created)
}

fn scaffold_basic_ts(root: &Path, name: &str, created: &mut Vec<PathBuf>) -> ZerhoResult<()> {
    write_file(
        root,
        "zerho.toml",
        &format!(
            "#:schema ./zerho.schema.json\n\n\
             [project]\n\
             name = \"{name}\"\n\
             root = \".\"\n\
             entry = \"src/main.ts\"\n\
             public_dir = \"public\"\n\
             env_prefix = \"ZERHO_\"\n\
             framework = \"vanilla\"\n\n\
             [server]\n\
             host = \"127.0.0.1\"\n\
             port = 5173\n\
             hmr = true\n\
             open = false\n\n\
             [build]\n\
             out_dir = \"dist\"\n\
             minify = true\n\
             sourcemap = \"linked\"\n\
             target = \"es2022\"\n\
             splitting = true\n\n\
             [css]\n\
             modules = false\n\
             minify = true\n\n\
             [cache]\n\
             enabled = true\n\
             dir = \".zerho/cache\"\n"
        ),
        created,
    )?;

    write_file(
        root,
        ".env",
        &format!("ZERHO_APP_TITLE=\"{name} (Zero-Node)\"\n"),
        created,
    )?;
    write_file(root, "public/logo.svg", LOGO_SVG, created)?;
    write_file(
        root,
        "index.html",
        &render_html(name, "/src/main.ts", "app"),
        created,
    )?;
    write_file(
        root,
        "zerho.schema.json",
        ZerhoConfig::json_schema(),
        created,
    )?;
    write_file(root, "tsconfig.json", TSCONFIG_JSON, created)?;
    write_file(root, "src/zerho-env.d.ts", ZERHO_ENV_DTS, created)?;
    write_file(root, "src/styles.css", BASE_CSS, created)?;
    write_file(root, "src/counter.ts", BASIC_COUNTER_TS, created)?;
    write_file(root, "src/main.ts", BASIC_MAIN_TS, created)?;
    Ok(())
}

fn scaffold_react_tsx(root: &Path, name: &str, created: &mut Vec<PathBuf>) -> ZerhoResult<()> {
    write_file(
        root,
        "zerho.toml",
        &format!(
            "#:schema ./zerho.schema.json\n\n\
             [project]\n\
             name = \"{name}\"\n\
             root = \".\"\n\
             entry = \"src/main.tsx\"\n\
             public_dir = \"public\"\n\
             env_prefix = \"ZERHO_\"\n\
             framework = \"react\"\n\n\
             [server]\n\
             host = \"127.0.0.1\"\n\
             port = 5173\n\
             hmr = true\n\
             open = false\n\n\
             [build]\n\
             out_dir = \"dist\"\n\
             minify = true\n\
             sourcemap = \"linked\"\n\
             target = \"es2022\"\n\
             splitting = true\n\n\
             [css]\n\
             modules = false\n\
             minify = true\n\n\
             [cache]\n\
             enabled = true\n\
             dir = \".zerho/cache\"\n"
        ),
        created,
    )?;

    write_file(
        root,
        ".env",
        &format!("ZERHO_APP_TITLE=\"{name} · React + TSX\"\n"),
        created,
    )?;
    write_file(root, "public/logo.svg", LOGO_SVG, created)?;
    write_file(
        root,
        "index.html",
        &render_html(name, "/src/main.tsx", "root"),
        created,
    )?;
    write_file(
        root,
        "zerho.schema.json",
        ZerhoConfig::json_schema(),
        created,
    )?;
    write_file(root, "tsconfig.json", TSCONFIG_JSON, created)?;
    write_file(root, "src/zerho-env.d.ts", ZERHO_ENV_DTS, created)?;
    write_file(root, "src/styles.css", BASE_CSS, created)?;
    write_file(root, "src/lazy-panel.tsx", REACT_LAZY_TSX, created)?;
    write_file(root, "src/main.tsx", REACT_MAIN_TSX, created)?;
    write_file(
        root,
        "node_modules/react/package.json",
        REACT_PKG_JSON,
        created,
    )?;
    write_file(root, "node_modules/react/index.js", REACT_INDEX_JS, created)?;
    write_file(
        root,
        "node_modules/react/jsx-runtime.js",
        REACT_JSX_RUNTIME_JS,
        created,
    )?;
    Ok(())
}

fn write_file(
    root: &Path,
    rel: &str,
    content: &str,
    created: &mut Vec<PathBuf>,
) -> ZerhoResult<()> {
    let full = root.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).map_err(|err| ZerhoError::Io {
            path: Some(parent.to_path_buf()),
            reason: err.to_string(),
        })?;
    }
    fs::write(&full, content).map_err(|err| ZerhoError::Io {
        path: Some(full.clone()),
        reason: err.to_string(),
    })?;
    created.push(full);
    Ok(())
}

fn render_html(title: &str, entry_src: &str, mount_id: &str) -> String {
    format!(
        "<!doctype html>\n\
         <html lang=\"en\">\n\
         <head>\n\
           <meta charset=\"UTF-8\" />\n\
           <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\" />\n\
           <title>{title}</title>\n\
           <link rel=\"preconnect\" href=\"https://fonts.googleapis.com\" />\n\
           <link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin />\n\
           <link href=\"https://fonts.googleapis.com/css2?family=Red+Hat+Display:wght@700;900&family=Red+Hat+Mono:wght@500&family=Red+Hat+Text:wght@400;600&display=swap\" rel=\"stylesheet\" />\n\
           <link href=\"https://fonts.googleapis.com/css2?family=Material+Symbols+Outlined\" rel=\"stylesheet\" />\n\
         </head>\n\
         <body>\n\
           <div id=\"{mount_id}\"></div>\n\
           <script type=\"module\" src=\"{entry_src}\"></script>\n\
         </body>\n\
         </html>\n"
    )
}

const TSCONFIG_JSON: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "jsx": "react-jsx",
    "strict": true,
    "noEmit": true,
    "isolatedModules": true,
    "baseUrl": ".",
    "paths": {
      "@/*": ["src/*"]
    }
  },
  "include": ["src"]
}
"#;

const ZERHO_ENV_DTS: &str = r#"/// <reference types="zerho/client" />

interface ImportMetaEnv {
  readonly MODE: string;
  readonly DEV: boolean;
  readonly PROD: boolean;
  readonly [key: string]: string | boolean | undefined;
}

interface ImportMetaHot {
  readonly data: Record<string, unknown>;
  accept(cb?: (mod: unknown) => void): void;
  dispose(cb: (data: Record<string, unknown>) => void): void;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
  readonly hot?: ImportMetaHot;
}

declare module "*.module.css" {
  const classes: { readonly [key: string]: string };
  export default classes;
}

declare module "*.css" {
  const css: string;
  export default css;
}

declare module "*.svg" {
  const src: string;
  export default src;
}

declare module "*.png" {
  const src: string;
  export default src;
}

declare module "*.jpg" {
  const src: string;
  export default src;
}

declare module "*.jpeg" {
  const src: string;
  export default src;
}

declare module "*.webp" {
  const src: string;
  export default src;
}
"#;

const LOGO_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64">
  <rect width="64" height="64" rx="14" fill="#111827"/>
  <path d="M36 10L16 36h14l-2 18 20-26H34l2-18z" fill="#f97316"/>
</svg>
"##;

const BASE_CSS: &str = r#":root {
  color-scheme: dark;
  --bg: #090d14;
  --surface: #111827;
  --border: #24324a;
  --accent: #f97316;
  --text: #f1f5f9;
  --muted: #94a3b8;
}

body {
  min-height: 100vh;
  margin: 0;
  display: grid;
  place-items: center;
  background: var(--bg);
  color: var(--text);
  font-family: "Red Hat Text", system-ui, sans-serif;

  & .card {
    width: min(540px, 92vw);
    padding: 2rem;
    border-radius: 16px;
    background: var(--surface);
    border: 1px solid var(--border);
    display: grid;
    gap: 1.25rem;
  }

  & h1 {
    font-family: "Red Hat Display", sans-serif;
    margin: 0;
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  & .mono {
    font-family: "Red Hat Mono", monospace;
    color: var(--accent);
    font-size: 0.9rem;
  }

  & button {
    cursor: pointer;
    border: none;
    border-radius: 10px;
    padding: 0.85rem 1.25rem;
    background: var(--accent);
    color: #090d14;
    font-family: "Red Hat Mono", monospace;
    font-weight: 600;
  }
}
"#;

const BASIC_COUNTER_TS: &str = r"export interface CounterState {
  count: number;
}

export function increment(state: CounterState): CounterState {
  return { count: state.count + 1 };
}
";

const BASIC_MAIN_TS: &str = r#"import "./styles.css";
import { increment, type CounterState } from "./counter";

interface ZerhoHot {
  data: { count?: number };
  accept: () => void;
  dispose: (cb: (data: { count?: number }) => void) => void;
}

const hot = (import.meta as unknown as { hot?: ZerhoHot }).hot;
const env = (import.meta as unknown as { env?: Record<string, string> }).env;

let state: CounterState = { count: hot?.data.count ?? 0 };
const title = env?.ZERHO_APP_TITLE ?? "zerho app";

const root = document.getElementById("app");
if (root) {
  root.innerHTML = `
    <main class="card">
      <h1><img src="/logo.svg" alt="zerho" width="32" height="32" /> ${title}</h1>
      <p class="mono">Compiled natively in Rust (Zero Node.js)</p>
      <button id="counter-btn">Clicks: ${state.count}</button>
    </main>
  `;

  const button = document.getElementById("counter-btn");
  button?.addEventListener("click", () => {
    state = increment(state);
    button.textContent = `Clicks: ${state.count}`;
  });
}

if (hot) {
  hot.dispose((data) => {
    data.count = state.count;
  });
  hot.accept();
}
"#;

const REACT_LAZY_TSX: &str = r#"export default function LazyPanel(): unknown {
  return (
    <section className="mono">
      Split async chunk loaded on demand by zerho!
    </section>
  );
}
"#;

const REACT_MAIN_TSX: &str = r#"import "./styles.css";
import { mount } from "react";

interface ZerhoHot {
  data: { chunkLoaded?: boolean };
  accept: () => void;
  dispose: (cb: (data: { chunkLoaded?: boolean }) => void) => void;
}

const hot = (import.meta as unknown as { hot?: ZerhoHot }).hot;
const env = (import.meta as unknown as { env?: Record<string, string> }).env;
let chunkLoaded = hot?.data.chunkLoaded ?? false;

async function renderLazyChunk() {
  const mod = await import("./lazy-panel");
  const slot = document.getElementById("lazy-slot");
  if (slot) {
    const Panel = mod.default;
    mount(<Panel />, slot);
    chunkLoaded = true;
  }
}

function App() {
  const title = env?.ZERHO_APP_TITLE ?? "zerho React + TSX";
  return (
    <main className="card">
      <h1>
        <img src="/logo.svg" alt="zerho" width="32" height="32" />
        {title}
      </h1>
      <p className="mono">Zero-Node Rust Dev Server & Bundler</p>
      <button onClick={renderLazyChunk}>Load Split Async Chunk</button>
      <div id="lazy-slot" />
    </main>
  );
}

const root = document.getElementById("root");
if (root) {
  mount(<App />, root);
  if (chunkLoaded) {
    void renderLazyChunk();
  }
}

if (hot) {
  hot.dispose((data) => {
    data.chunkLoaded = chunkLoaded;
  });
  hot.accept();
}
"#;

const REACT_PKG_JSON: &str = r#"{
  "name": "react",
  "version": "19.0.0-zerho-lite",
  "type": "module",
  "exports": {
    ".": "./index.js",
    "./jsx-runtime": "./jsx-runtime.js",
    "./jsx-dev-runtime": "./jsx-runtime.js"
  }
}
"#;

const REACT_INDEX_JS: &str = r#"export function mount(vnode, container) {
  container.innerHTML = "";
  container.appendChild(renderNode(vnode));
}

function renderNode(node) {
  if (node == null || typeof node === "boolean") return document.createTextNode("");
  if (typeof node === "string" || typeof node === "number") return document.createTextNode(String(node));
  if (Array.isArray(node)) {
    const frag = document.createDocumentFragment();
    for (const child of node) frag.appendChild(renderNode(child));
    return frag;
  }
  if (typeof node.type === "function") return renderNode(node.type(node.props || {}));
  const el = document.createElement(node.type);
  const props = node.props || {};
  for (const [key, val] of Object.entries(props)) {
    if (key === "children") continue;
    if (key === "className") el.setAttribute("class", val);
    else if (key.startsWith("on") && typeof val === "function") el.addEventListener(key.slice(2).toLowerCase(), val);
    else if (val != null) el.setAttribute(key, String(val));
  }
  if (props.children !== undefined) el.appendChild(renderNode(props.children));
  return el;
}
"#;

const REACT_JSX_RUNTIME_JS: &str = r"export function jsx(type, props) {
  return { type, props: props || {} };
}
export const jsxs = jsx;
export const jsxDEV = jsx;
export function Fragment(props) {
  return props.children;
}
";

#[cfg(test)]
mod tests {
    use super::*;
    use zerho_bundler::Bundler;
    use zerho_config::ZerhoConfig;

    #[test]
    fn scaffolds_and_builds_both_templates_end_to_end() {
        for (name, tpl) in [
            ("zerho-tpl-basic", Template::BasicTs),
            ("zerho-tpl-react", Template::ReactTsx),
        ] {
            let dir = std::env::temp_dir().join(name);
            let _ = fs::remove_dir_all(&dir);

            let files = scaffold_project(&dir, tpl).expect("scaffold should succeed");
            assert!(!files.is_empty());

            let mut cfg = ZerhoConfig::load(&dir.join("zerho.toml")).expect("config should load");
            cfg.project.root = dir.clone();
            let mut bundler = Bundler::new(cfg);
            let out = bundler
                .build()
                .expect("scaffolded project should build cleanly");
            assert!(out.out_dir.join("index.html").is_file());
            assert!(out.out_dir.join("logo.svg").is_file());
            assert!(out.out_dir.join("manifest.json").is_file());

            let _ = fs::remove_dir_all(&dir);
        }
    }
}
