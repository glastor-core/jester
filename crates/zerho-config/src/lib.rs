use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use zerho_core::{Framework, ZerhoError, ZerhoResult};

/// Root configuration loaded from `zerho.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZerhoConfig {
    pub project: ProjectConfig,
    pub server: ServerConfig,
    pub build: BuildConfig,
    pub css: CssConfig,
    pub cache: CacheConfig,
    pub security: SecurityConfig,
    pub assets: AssetsConfig,
    pub plugins: PluginsConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginsConfig {
    /// Paths to sandboxed WebAssembly plugins (`.wasm`) loaded via `wasmtime`.
    pub wasm: Vec<PathBuf>,
    /// Execution fuel budget per WASM hook call to prevent infinite loops.
    pub fuel_limit: u64,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            wasm: Vec::new(),
            fuel_limit: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityConfig {
    /// Emit `integrity="sha384-..."` (Subresource Integrity) on `<script>` and `<link>` tags.
    pub sri: bool,
    /// Emit strict `dist/csp.json` Content-Security-Policy manifest.
    pub csp: bool,
    /// Scan emitted JS bundles for accidentally leaked API keys/tokens and fail build if found.
    pub secret_scan: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            sri: true,
            csp: true,
            secret_scan: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetsConfig {
    /// Automatically optimize imported and `public/` `.svg` files in pure Rust.
    pub optimize_svg: bool,
    /// Automatically convert imported `.png`/`.jpg`/`.jpeg` assets to smaller `.webp` in pure Rust.
    pub optimize_images: bool,
}

impl Default for AssetsConfig {
    fn default() -> Self {
        Self {
            optimize_svg: true,
            optimize_images: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibConfig {
    /// Library entry point (enables Library Mode when set, emitting ESM `.mjs` and/or CJS `.cjs`).
    pub entry: Option<PathBuf>,
    /// Output base filename (defaults to `project.name`).
    pub name: Option<String>,
    /// Output formats: `"es"`, `"cjs"` (default: `["es", "cjs"]`).
    pub formats: Vec<String>,
}

impl Default for LibConfig {
    fn default() -> Self {
        Self {
            entry: None,
            name: None,
            formats: vec!["es".to_owned(), "cjs".to_owned()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BudgetsConfig {
    /// Maximum allowed total gzipped JS size in KiB.
    pub max_js_kb: Option<u64>,
    /// Maximum allowed total gzipped CSS size in KiB.
    pub max_css_kb: Option<u64>,
    /// Maximum allowed gzipped size for any single JS/CSS chunk in KiB.
    pub max_chunk_kb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    pub name: String,
    pub root: PathBuf,
    pub entry: PathBuf,
    pub public_dir: PathBuf,
    pub env_prefix: String,
    pub framework: Framework,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: "zerho-app".to_owned(),
            root: PathBuf::from("."),
            entry: PathBuf::from("src/main.tsx"),
            public_dir: PathBuf::from("public"),
            env_prefix: "ZERHO_".to_owned(),
            framework: Framework::React,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub hmr: bool,
    pub open: bool,
    /// Enable zero-waterfall in-memory partial bundling endpoint (`/@zerho/bundle`).
    pub partial_bundling: bool,
    /// Path-prefix HTTP reverse proxy map, e.g. `"/api" -> "http://127.0.0.1:8080"`.
    pub proxy: BTreeMap<String, String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 5173,
            hmr: true,
            open: false,
            partial_bundling: true,
            proxy: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMapMode {
    None,
    #[default]
    Linked,
    Inline,
    Hidden,
}

impl SourceMapMode {
    pub fn parse(value: &str) -> ZerhoResult<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "linked" => Ok(Self::Linked),
            "inline" => Ok(Self::Inline),
            "hidden" => Ok(Self::Hidden),
            other => Err(ZerhoError::Config(format!(
                "invalid sourcemap mode `{other}` (expected: none, linked, inline, hidden)"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Linked => "linked",
            Self::Inline => "inline",
            Self::Hidden => "hidden",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct BuildConfig {
    pub out_dir: PathBuf,
    /// Public base path used for emitted asset, chunk, and HTML URLs (default: `"/"`).
    pub base: String,
    /// Remove stale files from `out_dir` before writing fresh build output (default: `true`).
    pub empty_out_dir: bool,
    pub minify: bool,
    pub sourcemap: SourceMapMode,
    pub target: String,
    pub splitting: bool,
    pub brotli: bool,
    pub scope_hoisting: bool,
    pub analyze: bool,
    /// Emit a DOM-free Edge/Server ESM bundle (`dist/server.mjs`) with `import.meta.env.SSR = true`.
    pub ssr: bool,
    pub budgets: BudgetsConfig,
    pub lib: LibConfig,
    /// Optional Multi-Page App (MPA) entry map: `"index" -> "src/main.ts"`, `"admin" -> "src/admin.ts"`.
    pub entries: BTreeMap<String, PathBuf>,
}

impl BuildConfig {
    /// Returns a canonicalized public base prefix ending with `/` (e.g. `"/"`, `"/demo/"`, `"./"`).
    pub fn normalized_base(&self) -> String {
        let trimmed = self.base.trim();
        if trimmed.is_empty() || trimmed == "/" {
            return "/".to_owned();
        }
        if trimmed == "." || trimmed == "./" {
            return "./".to_owned();
        }
        let mut out = if trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
            || trimmed.starts_with("./")
            || trimmed.starts_with('/')
        {
            trimmed.to_owned()
        } else {
            format!("/{trimmed}")
        };
        if !out.ends_with('/') {
            out.push('/');
        }
        out
    }

    /// Joins `normalized_base()` with a bundle-relative path (e.g. `"assets/index-abc.js"`).
    pub fn asset_url(&self, rel_path: &str) -> String {
        let base = self.normalized_base();
        let clean = rel_path.trim_start_matches('/');
        format!("{base}{clean}")
    }
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            out_dir: PathBuf::from("dist"),
            base: "/".to_owned(),
            empty_out_dir: true,
            minify: true,
            sourcemap: SourceMapMode::Linked,
            target: "es2022".to_owned(),
            splitting: true,
            brotli: true,
            scope_hoisting: true,
            analyze: false,
            ssr: false,
            budgets: BudgetsConfig::default(),
            lib: LibConfig::default(),
            entries: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssConfig {
    pub modules: bool,
    pub minify: bool,
}

impl Default for CssConfig {
    fn default() -> Self {
        Self {
            modules: true,
            minify: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheConfig {
    pub enabled: bool,
    pub dir: PathBuf,
    /// Optional HTTP/S3-compatible remote team cache base URL (e.g. `http://127.0.0.1:9000/cache`).
    pub remote_url: Option<String>,
    /// Optional bearer token (or env var reference) for authenticating with `remote_url`.
    pub remote_token: Option<String>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: PathBuf::from(".zerho/cache"),
            remote_url: None,
            remote_token: None,
        }
    }
}

impl ZerhoConfig {
    /// Loads and validates a `zerho.toml` file from disk.
    pub fn load(path: &Path) -> ZerhoResult<Self> {
        let raw = fs::read_to_string(path).map_err(|err| ZerhoError::Io {
            path: Some(path.to_path_buf()),
            reason: err.to_string(),
        })?;
        Self::parse_toml(&raw)
    }

    /// Parses declarative `zerho.toml` content into a validated [`ZerhoConfig`].
    #[allow(clippy::too_many_lines)]
    pub fn parse_toml(input: &str) -> ZerhoResult<Self> {
        let mut config = Self::default();
        let mut section = String::new();

        for (idx, raw_line) in input.lines().enumerate() {
            let line_no = idx + 1;
            let line = strip_inline_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                line[1..line.len() - 1].trim().clone_into(&mut section);
                continue;
            }

            let Some((raw_key, value)) = line.split_once('=') else {
                return Err(ZerhoError::Config(format!(
                    "line {line_no}: expected `key = value`, got `{line}`"
                )));
            };

            let key = raw_key.trim();
            let value = value.trim();

            if section == "server.proxy" {
                let prefix = unquote_key(key);
                let target = parse_string(value, line_no)?;
                config.server.proxy.insert(prefix, target);
                continue;
            }

            if section == "build.entries" {
                let entry_name = unquote_key(key);
                let entry_path = PathBuf::from(parse_string(value, line_no)?);
                config.build.entries.insert(entry_name, entry_path);
                continue;
            }

            match (section.as_str(), key) {
                ("project", "name") => config.project.name = parse_string(value, line_no)?,
                ("project", "root") => {
                    config.project.root = PathBuf::from(parse_string(value, line_no)?);
                }
                ("project", "entry") => {
                    config.project.entry = PathBuf::from(parse_string(value, line_no)?);
                }
                ("project", "public_dir") => {
                    config.project.public_dir = PathBuf::from(parse_string(value, line_no)?);
                }
                ("project", "env_prefix") => {
                    config.project.env_prefix = parse_string(value, line_no)?;
                }
                ("project", "framework") => {
                    config.project.framework = Framework::parse(&parse_string(value, line_no)?)?;
                }
                ("server", "host") => config.server.host = parse_string(value, line_no)?,
                ("server", "port") => config.server.port = parse_u16(value, line_no)?,
                ("server", "hmr") => config.server.hmr = parse_bool(value, line_no)?,
                ("server", "open") => config.server.open = parse_bool(value, line_no)?,
                ("server", "partial_bundling") => {
                    config.server.partial_bundling = parse_bool(value, line_no)?;
                }
                ("build", "out_dir") => {
                    config.build.out_dir = PathBuf::from(parse_string(value, line_no)?);
                }
                ("build", "base") => config.build.base = parse_string(value, line_no)?,
                ("build", "empty_out_dir") => {
                    config.build.empty_out_dir = parse_bool(value, line_no)?;
                }
                ("build", "minify") => config.build.minify = parse_bool(value, line_no)?,
                ("build", "sourcemap") => {
                    config.build.sourcemap = SourceMapMode::parse(&parse_string(value, line_no)?)?;
                }
                ("build", "target") => config.build.target = parse_string(value, line_no)?,
                ("build", "splitting") => config.build.splitting = parse_bool(value, line_no)?,
                ("build", "brotli") => config.build.brotli = parse_bool(value, line_no)?,
                ("build", "scope_hoisting") => {
                    config.build.scope_hoisting = parse_bool(value, line_no)?;
                }
                ("build", "analyze") => config.build.analyze = parse_bool(value, line_no)?,
                ("build", "ssr") => config.build.ssr = parse_bool(value, line_no)?,
                ("build.budgets", "max_js_kb") => {
                    config.build.budgets.max_js_kb = Some(parse_u64(value, line_no)?);
                }
                ("build.budgets", "max_css_kb") => {
                    config.build.budgets.max_css_kb = Some(parse_u64(value, line_no)?);
                }
                ("build.budgets", "max_chunk_kb") => {
                    config.build.budgets.max_chunk_kb = Some(parse_u64(value, line_no)?);
                }
                ("build.lib", "entry") => {
                    config.build.lib.entry = Some(PathBuf::from(parse_string(value, line_no)?));
                }
                ("build.lib", "name") => {
                    config.build.lib.name = Some(parse_string(value, line_no)?);
                }
                ("build.lib", "formats") => {
                    config.build.lib.formats = parse_string_list(value, line_no)?;
                }
                ("css", "modules") => config.css.modules = parse_bool(value, line_no)?,
                ("css", "minify") => config.css.minify = parse_bool(value, line_no)?,
                ("cache", "enabled") => config.cache.enabled = parse_bool(value, line_no)?,
                ("cache", "dir") => {
                    config.cache.dir = PathBuf::from(parse_string(value, line_no)?);
                }
                ("cache", "remote_url") => {
                    config.cache.remote_url = Some(parse_string(value, line_no)?);
                }
                ("cache", "remote_token") => {
                    config.cache.remote_token = Some(parse_string(value, line_no)?);
                }
                ("security", "sri") => config.security.sri = parse_bool(value, line_no)?,
                ("security", "csp") => config.security.csp = parse_bool(value, line_no)?,
                ("security", "secret_scan") => {
                    config.security.secret_scan = parse_bool(value, line_no)?;
                }
                ("assets", "optimize_svg") => {
                    config.assets.optimize_svg = parse_bool(value, line_no)?;
                }
                ("assets", "optimize_images") => {
                    config.assets.optimize_images = parse_bool(value, line_no)?;
                }
                ("plugins", "wasm") => {
                    config.plugins.wasm = parse_string_list(value, line_no)?
                        .into_iter()
                        .map(PathBuf::from)
                        .collect();
                }
                ("plugins", "fuel_limit") => {
                    config.plugins.fuel_limit = parse_u64(value, line_no)?;
                }
                _ => {
                    return Err(ZerhoError::Config(format!(
                        "line {line_no}: unknown key `{section}.{key}`"
                    )));
                }
            }
        }

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> ZerhoResult<()> {
        if self.project.name.trim().is_empty() {
            return Err(ZerhoError::Config(
                "`project.name` cannot be empty".to_owned(),
            ));
        }
        if self.server.port == 0 {
            return Err(ZerhoError::Config(
                "`server.port` must be between 1 and 65535".to_owned(),
            ));
        }
        Ok(())
    }

    /// Serializes the configuration into a clean `zerho.toml` file.
    pub fn to_toml(&self) -> String {
        let mut out = format!(
            "[project]\n\
             name = \"{}\"\n\
             root = \"{}\"\n\
             entry = \"{}\"\n\
             public_dir = \"{}\"\n\
             env_prefix = \"{}\"\n\
             framework = \"{}\"\n\n\
             [server]\n\
             host = \"{}\"\n\
             port = {}\n\
             hmr = {}\n\
             open = {}\n\n\
             [build]\n\
             out_dir = \"{}\"\n\
             base = \"{}\"\n\
             empty_out_dir = {}\n\
             minify = {}\n\
             sourcemap = \"{}\"\n\
             target = \"{}\"\n\
             splitting = {}\n\
             brotli = {}\n\
             scope_hoisting = {}\n\n\
             [css]\n\
             modules = {}\n\
             minify = {}\n\n\
             [cache]\n\
             enabled = {}\n\
             dir = \"{}\"\n",
            self.project.name,
            self.project.root.display(),
            self.project.entry.display(),
            self.project.public_dir.display(),
            self.project.env_prefix,
            self.project.framework.as_str(),
            self.server.host,
            self.server.port,
            self.server.hmr,
            self.server.open,
            self.build.out_dir.display(),
            self.build.base,
            self.build.empty_out_dir,
            self.build.minify,
            self.build.sourcemap.as_str(),
            self.build.target,
            self.build.splitting,
            self.build.brotli,
            self.build.scope_hoisting,
            self.css.modules,
            self.css.minify,
            self.cache.enabled,
            self.cache.dir.display(),
        );
        if let Some(remote_url) = &self.cache.remote_url {
            let _ = writeln!(out, "remote_url = \"{remote_url}\"");
        }
        if let Some(remote_token) = &self.cache.remote_token {
            let _ = writeln!(out, "remote_token = \"{remote_token}\"");
        }
        if !self.server.proxy.is_empty() {
            out.push_str("\n[server.proxy]\n");
            for (prefix, target) in &self.server.proxy {
                let _ = writeln!(out, "\"{prefix}\" = \"{target}\"");
            }
        }
        out
    }

    /// Emits a JSON Schema (Draft 2020-12) for IDE autocomplete & validation of `zerho.toml`
    /// across VS Code (Even Better TOML / Taplo), Cursor, Zed, `JetBrains`, and Neovim.
    #[allow(clippy::too_many_lines)]
    pub fn json_schema() -> &'static str {
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "zerho.toml Configuration",
  "type": "object",
  "properties": {
    "project": {
      "type": "object",
      "properties": {
        "name": { "type": "string" },
        "root": { "type": "string", "default": "." },
        "entry": { "type": "string", "default": "src/main.tsx" },
        "public_dir": { "type": "string", "default": "public" },
        "env_prefix": { "type": "string", "default": "ZERHO_" },
        "framework": { "type": "string", "enum": ["react", "preact", "solid", "vanilla"], "default": "react" }
      }
    },
    "server": {
      "type": "object",
      "properties": {
        "host": { "type": "string", "default": "127.0.0.1" },
        "port": { "type": "integer", "minimum": 1, "maximum": 65535, "default": 5173 },
        "hmr": { "type": "boolean", "default": true },
        "open": { "type": "boolean", "default": false },
        "partial_bundling": { "type": "boolean", "default": true },
        "proxy": {
          "type": "object",
          "additionalProperties": { "type": "string" }
        }
      }
    },
    "build": {
      "type": "object",
      "properties": {
        "out_dir": { "type": "string", "default": "dist" },
        "base": { "type": "string", "default": "/" },
        "empty_out_dir": { "type": "boolean", "default": true },
        "minify": { "type": "boolean", "default": true },
        "sourcemap": { "type": "string", "enum": ["none", "linked", "inline", "hidden"], "default": "linked" },
        "target": { "type": "string", "default": "es2022" },
        "splitting": { "type": "boolean", "default": true },
        "brotli": { "type": "boolean", "default": true },
        "scope_hoisting": { "type": "boolean", "default": true },
        "analyze": { "type": "boolean", "default": false },
        "ssr": { "type": "boolean", "default": false },
        "budgets": {
          "type": "object",
          "properties": {
            "max_js_kb": { "type": "integer", "minimum": 1 },
            "max_css_kb": { "type": "integer", "minimum": 1 },
            "max_chunk_kb": { "type": "integer", "minimum": 1 }
          }
        },
        "lib": {
          "type": "object",
          "properties": {
            "entry": { "type": "string" },
            "name": { "type": "string" },
            "formats": {
              "type": "array",
              "items": { "type": "string", "enum": ["es", "esm", "cjs"] },
              "default": ["es", "cjs"]
            }
          }
        },
        "entries": {
          "type": "object",
          "additionalProperties": { "type": "string" }
        }
      }
    },
    "css": {
      "type": "object",
      "properties": {
        "modules": { "type": "boolean", "default": true },
        "minify": { "type": "boolean", "default": true }
      }
    },
    "cache": {
      "type": "object",
      "properties": {
        "enabled": { "type": "boolean", "default": true },
        "dir": { "type": "string", "default": ".zerho/cache" },
        "remote_url": { "type": "string" },
        "remote_token": { "type": "string" }
      }
    },
    "security": {
      "type": "object",
      "properties": {
        "sri": { "type": "boolean", "default": true },
        "csp": { "type": "boolean", "default": true },
        "secret_scan": { "type": "boolean", "default": true }
      }
    },
    "assets": {
      "type": "object",
      "properties": {
        "optimize_svg": { "type": "boolean", "default": true },
        "optimize_images": { "type": "boolean", "default": true }
      }
    },
    "plugins": {
      "type": "object",
      "properties": {
        "wasm": { "type": "array", "items": { "type": "string" }, "default": [] },
        "fuel_limit": { "type": "integer", "minimum": 1, "default": 1000000 }
      }
    }
  }
}"#
    }
}

/// Loads `.env`, `.env.local`, `.env.<mode>`, and `.env.<mode>.local` from `root`,
/// returning only variables starting with `env_prefix` (default `"ZERHO_"`).
pub fn load_env_vars(root: &Path, mode: &str, env_prefix: &str) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    let files = [
        ".env".to_owned(),
        ".env.local".to_owned(),
        format!(".env.{mode}"),
        format!(".env.{mode}.local"),
    ];

    for file in files {
        let path = root.join(file);
        if let Ok(content) = fs::read_to_string(path) {
            parse_dotenv_into(&content, env_prefix, &mut vars);
        }
    }
    vars
}

/// Builds a JSON object literal for `import.meta.env` containing `MODE`, `DEV`, `PROD`, and `ZERHO_*` vars.
pub fn build_import_meta_env_json(
    mode: &str,
    is_dev: bool,
    vars: &BTreeMap<String, String>,
) -> String {
    let mut entries = vec![
        format!("\"MODE\":\"{}\"", escape_json_str(mode)),
        format!("\"DEV\":{is_dev}"),
        format!("\"PROD\":{}", !is_dev),
    ];
    for (k, v) in vars {
        entries.push(format!(
            "\"{}\":\"{}\"",
            escape_json_str(k),
            escape_json_str(v)
        ));
    }
    format!("{{{}}}", entries.join(","))
}

fn parse_dotenv_into(content: &str, prefix: &str, out: &mut BTreeMap<String, String>) {
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if !prefix.is_empty() && !key.starts_with(prefix) {
            continue;
        }
        let val = v.trim();
        let unquoted = if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
        {
            &val[1..val.len() - 1]
        } else {
            val
        };
        out.insert(key.to_owned(), unquoted.to_owned());
    }
}

fn escape_json_str(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

fn unquote_key(raw: &str) -> String {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        raw[1..raw.len() - 1].to_owned()
    } else {
        raw.to_owned()
    }
}

fn strip_inline_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn parse_string(raw: &str, line_no: usize) -> ZerhoResult<String> {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        Ok(raw[1..raw.len() - 1].to_owned())
    } else {
        Err(ZerhoError::Config(format!(
            "line {line_no}: expected quoted string, got `{raw}`"
        )))
    }
}

fn parse_string_list(raw: &str, line_no: usize) -> ZerhoResult<Vec<String>> {
    let trimmed = raw.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let inner = &trimmed[1..trimmed.len() - 1];
        let mut items = Vec::new();
        for part in inner.split(',') {
            let p = part.trim();
            if !p.is_empty() {
                items.push(parse_string(p, line_no)?);
            }
        }
        return Ok(items);
    }
    let single = parse_string(trimmed, line_no)?;
    Ok(single
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

fn parse_bool(raw: &str, line_no: usize) -> ZerhoResult<bool> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ZerhoError::Config(format!(
            "line {line_no}: expected boolean (`true` or `false`), got `{raw}`"
        ))),
    }
}

fn parse_u16(raw: &str, line_no: usize) -> ZerhoResult<u16> {
    raw.parse::<u16>().map_err(|_| {
        ZerhoError::Config(format!(
            "line {line_no}: expected integer between 0 and 65535, got `{raw}`"
        ))
    })
}

fn parse_u64(raw: &str, line_no: usize) -> ZerhoResult<u64> {
    raw.parse::<u64>().map_err(|_| {
        ZerhoError::Config(format!(
            "line {line_no}: expected positive integer, got `{raw}`"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_roundtrip_toml_including_proxy() {
        let mut original = ZerhoConfig::default();
        original
            .server
            .proxy
            .insert("/api".to_owned(), "http://127.0.0.1:8080".to_owned());
        original.cache.remote_url = Some("http://127.0.0.1:9000/cache".to_owned());
        original.cache.remote_token = Some("env:ZERHO_CACHE_TOKEN".to_owned());
        let serialized = original.to_toml();
        let parsed = ZerhoConfig::parse_toml(&serialized).expect("default config should parse");
        assert_eq!(original, parsed);
    }

    #[test]
    fn parses_dotenv_with_prefix_filtering() {
        let mut vars = BTreeMap::new();
        parse_dotenv_into(
            "# comment\nZERHO_API=\"http://localhost:8080\"\nSECRET_KEY=ignored\nZERHO_FLAG=1\n",
            "ZERHO_",
            &mut vars,
        );
        assert_eq!(
            vars.get("ZERHO_API").map(String::as_str),
            Some("http://localhost:8080")
        );
        assert_eq!(vars.get("ZERHO_FLAG").map(String::as_str), Some("1"));
        assert!(!vars.contains_key("SECRET_KEY"));

        let json = build_import_meta_env_json("development", true, &vars);
        assert!(json.contains("\"MODE\":\"development\""));
        assert!(json.contains("\"DEV\":true"));
        assert!(json.contains("\"ZERHO_API\":\"http://localhost:8080\""));
    }

    #[test]
    fn rejects_invalid_port_and_framework() {
        let bad_port = "[server]\nport = 0\n";
        assert!(ZerhoConfig::parse_toml(bad_port).is_err());

        let bad_fw = "[project]\nframework = \"unknown\"\n";
        assert!(ZerhoConfig::parse_toml(bad_fw).is_err());
    }

    #[test]
    fn emits_json_schema() {
        let schema = ZerhoConfig::json_schema();
        assert!(schema.contains("json-schema.org/draft/2020-12/schema"));
        for key in [
            "\"proxy\"",
            "\"partial_bundling\"",
            "\"brotli\"",
            "\"scope_hoisting\"",
            "\"analyze\"",
            "\"budgets\"",
            "\"lib\"",
            "\"remote_url\"",
            "\"security\"",
            "\"assets\"",
            "\"optimize_images\"",
            "\"base\"",
            "\"empty_out_dir\"",
        ] {
            assert!(schema.contains(key), "JSON schema missing {key}");
        }
    }

    #[test]
    fn normalizes_build_base_and_asset_urls() {
        let mut cfg = BuildConfig::default();
        assert_eq!(cfg.normalized_base(), "/");
        assert_eq!(cfg.asset_url("assets/app.js"), "/assets/app.js");

        cfg.base = "demo".to_owned();
        assert_eq!(cfg.normalized_base(), "/demo/");
        assert_eq!(cfg.asset_url("/assets/app.js"), "/demo/assets/app.js");

        cfg.base = "/sub/app".to_owned();
        assert_eq!(cfg.normalized_base(), "/sub/app/");

        cfg.base = "./".to_owned();
        assert_eq!(cfg.normalized_base(), "./");
        assert_eq!(cfg.asset_url("assets/app.js"), "./assets/app.js");

        cfg.base = "https://cdn.example.com/static".to_owned();
        assert_eq!(cfg.normalized_base(), "https://cdn.example.com/static/");
    }
}
