use oxc_resolver::{
    ResolveOptions, Resolver as InnerOxcResolver, TsconfigDiscovery, TsconfigOptions,
    TsconfigReferences,
};
use std::fmt;
use std::path::{Path, PathBuf};
use zerho_core::{ModuleKind, ZerhoError, ZerhoResult};

/// Optional Vite-compatible import query suffix (`?raw`, `?url`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssetQuery {
    #[default]
    None,
    /// Import file contents as a UTF-8 string (`import txt from './file.md?raw'`).
    Raw,
    /// Emit file as a hashed static asset and export its public URL (`import u from './logo.svg?url'`).
    Url,
}

/// Splits a specifier into `(clean_path, AssetQuery)` for Vite-style `?raw` and `?url` imports.
pub fn split_specifier_query(specifier: &str) -> (&str, AssetQuery) {
    let Some((base, qs)) = specifier.split_once('?') else {
        return (specifier, AssetQuery::None);
    };
    let mut query = AssetQuery::None;
    for part in qs.split('&') {
        match part {
            "raw" => query = AssetQuery::Raw,
            "url" => query = AssetQuery::Url,
            _ => {}
        }
    }
    (base, query)
}

/// Output of module resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModule {
    pub path: PathBuf,
    pub specifier: String,
    pub kind: ModuleKind,
    pub is_virtual: bool,
    pub query: AssetQuery,
}

/// Resolution trait shared by the dev server and production bundler.
pub trait Resolver: Send + Sync {
    fn resolve(&self, specifier: &str, importer: Option<&Path>) -> ZerhoResult<ResolvedModule>;
}

/// Native Rust resolver powered by `oxc_resolver` (Node ESM/CJS, `package.json` `exports`/`imports`,
/// TypeScript/JSX extensions, and virtual modules).
pub struct OxcResolver {
    root: PathBuf,
    inner: InnerOxcResolver,
}

impl fmt::Debug for OxcResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OxcResolver")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl OxcResolver {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root_tsconfig = root.join("tsconfig.json");
        let tsconfig = if root_tsconfig.is_file() {
            Some(TsconfigDiscovery::Manual(TsconfigOptions {
                config_file: root_tsconfig,
                references: TsconfigReferences::Auto,
            }))
        } else {
            Some(TsconfigDiscovery::Auto)
        };

        let options = ResolveOptions {
            tsconfig,
            extensions: vec![
                ".tsx".to_owned(),
                ".ts".to_owned(),
                ".jsx".to_owned(),
                ".js".to_owned(),
                ".mjs".to_owned(),
                ".css".to_owned(),
                ".json".to_owned(),
            ],
            condition_names: vec![
                "browser".to_owned(),
                "import".to_owned(),
                "module".to_owned(),
                "default".to_owned(),
            ],
            main_fields: vec!["browser".to_owned(), "module".to_owned(), "main".to_owned()],
            ..ResolveOptions::default()
        };

        Self {
            root,
            inner: InnerOxcResolver::new(options),
        }
    }
}

impl Resolver for OxcResolver {
    fn resolve(&self, specifier: &str, importer: Option<&Path>) -> ZerhoResult<ResolvedModule> {
        if specifier.starts_with('\0')
            || specifier.starts_with("virtual:")
            || specifier.starts_with("/@zerho/")
        {
            return Ok(ResolvedModule {
                path: PathBuf::from(specifier),
                specifier: specifier.to_owned(),
                kind: ModuleKind::Virtual,
                is_virtual: true,
                query: AssetQuery::None,
            });
        }

        let (clean_specifier, asset_query) = split_specifier_query(specifier);

        let base_dir = match importer.and_then(Path::parent) {
            Some(parent) => parent,
            None => self.root.as_path(),
        };

        let normalized = clean_specifier
            .strip_prefix('/')
            .map_or(clean_specifier, |stripped| {
                // Treat URL-root imports like `/src/main.tsx` as relative to project root.
                stripped
            });
        let lookup_dir = if clean_specifier.starts_with('/') {
            self.root.as_path()
        } else {
            base_dir
        };

        // Ensure relative lookup when stripping leading slash for project-root paths.
        let query = if clean_specifier.starts_with('/') && !normalized.starts_with('.') {
            format!("./{normalized}")
        } else {
            normalized.to_owned()
        };

        let resolution =
            self.inner
                .resolve(lookup_dir, &query)
                .map_err(|err| ZerhoError::Resolve {
                    specifier: specifier.to_owned(),
                    importer: importer.map(Path::to_path_buf),
                    reason: err.to_string(),
                })?;

        let path = resolution.full_path();
        let kind = match asset_query {
            AssetQuery::Url => ModuleKind::Asset,
            AssetQuery::Raw | AssetQuery::None => ModuleKind::from_path(&path),
        };
        let canonical = path.to_string_lossy().into_owned();

        Ok(ResolvedModule {
            path,
            specifier: canonical,
            kind,
            is_virtual: false,
            query: asset_query,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_virtual_modules() {
        let resolver = OxcResolver::new(".");
        let module = resolver
            .resolve("virtual:zerho/hmr-client", None)
            .expect("virtual module should resolve");
        assert!(module.is_virtual);
        assert_eq!(module.kind, ModuleKind::Virtual);
    }

    #[test]
    fn resolves_zerho_internal_client_path() {
        let resolver = OxcResolver::new(".");
        let module = resolver
            .resolve("/@zerho/client", None)
            .expect("internal client should resolve");
        assert!(module.is_virtual);
    }

    #[test]
    fn resolves_tsconfig_paths_natively_without_plugins() {
        let dir = std::env::temp_dir().join("zerho-resolver-tsconfig-paths-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/utils")).expect("create src/utils");
        std::fs::write(
            dir.join("tsconfig.json"),
            r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@/*": ["src/*"]
    }
  }
}"#,
        )
        .expect("write tsconfig");
        std::fs::write(
            dir.join("src/utils/math.ts"),
            "export const add = (a: number, b: number) => a + b;\n",
        )
        .expect("write math.ts");
        std::fs::write(
            dir.join("src/main.ts"),
            "import { add } from '@/utils/math';\n",
        )
        .expect("write main.ts");

        let resolver = OxcResolver::new(&dir);
        let module = resolver
            .resolve("@/utils/math", Some(&dir.join("src/main.ts")))
            .expect("tsconfig @/* alias should resolve natively");
        assert!(
            module
                .path
                .ends_with(Path::new("src").join("utils").join("math.ts"))
        );
        assert_eq!(module.kind, ModuleKind::Ts);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolves_vite_raw_and_url_query_suffixes() {
        let dir = std::env::temp_dir().join("zerho-resolver-raw-url-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("create src");
        std::fs::write(dir.join("src/note.md"), "# Hello zerho\n").expect("write note.md");

        let resolver = OxcResolver::new(&dir);
        let raw_mod = resolver
            .resolve("./note.md?raw", Some(&dir.join("src/main.ts")))
            .expect("?raw should resolve");
        assert_eq!(raw_mod.query, AssetQuery::Raw);
        assert!(raw_mod.path.ends_with("note.md"));

        let url_mod = resolver
            .resolve("./note.md?url", Some(&dir.join("src/main.ts")))
            .expect("?url should resolve");
        assert_eq!(url_mod.query, AssetQuery::Url);
        assert_eq!(url_mod.kind, ModuleKind::Asset);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
