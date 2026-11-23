use std::fmt;
use std::path::{Path, PathBuf};
use oxc_resolver::{ResolveOptions, Resolver as InnerOxcResolver};
use zerho_core::{ModuleKind, ZerhoError, ZerhoResult};

/// Output of module resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModule {
    pub path: PathBuf,
    pub specifier: String,
    pub kind: ModuleKind,
    pub is_virtual: bool,
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
        let options = ResolveOptions {
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
            main_fields: vec![
                "browser".to_owned(),
                "module".to_owned(),
                "main".to_owned(),
            ],
            ..ResolveOptions::default()
        };

        Self { root, inner: InnerOxcResolver::new(options) }
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
            });
        }

        let base_dir = match importer.and_then(Path::parent) {
            Some(parent) => parent,
            None => self.root.as_path(),
        };

        let normalized = specifier.strip_prefix('/').map_or(specifier, |stripped| {
            // Treat URL-root imports like `/src/main.tsx` as relative to project root.
            stripped
        });
        let lookup_dir = if specifier.starts_with('/') {
            self.root.as_path()
        } else {
            base_dir
        };

        // Ensure relative lookup when stripping leading slash for project-root paths.
        let query = if specifier.starts_with('/') && !normalized.starts_with('.') {
            format!("./{normalized}")
        } else {
            normalized.to_owned()
        };

        let resolution = self.inner.resolve(lookup_dir, &query).map_err(|err| {
            ZerhoError::Resolve {
                specifier: specifier.to_owned(),
                importer: importer.map(Path::to_path_buf),
                reason: err.to_string(),
            }
        })?;

        let path = resolution.full_path();
        let kind = ModuleKind::from_path(&path);
        let canonical = path.to_string_lossy().into_owned();

        Ok(ResolvedModule {
            path,
            specifier: canonical,
            kind,
            is_virtual: false,
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
}
