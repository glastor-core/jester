use std::fmt;
use std::path::{Path, PathBuf};

/// Stable numeric identifier for a module inside the incremental graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(pub u64);

impl ModuleId {
    /// Derives a deterministic module identifier from its canonical specifier using `blake3`.
    pub fn from_specifier(specifier: &str) -> Self {
        let digest = blake3::hash(specifier.as_bytes());
        let bytes: [u8; 8] = digest.as_bytes()[..8]
            .try_into()
            .expect("blake3 digest always has 32 bytes");
        Self(u64::from_le_bytes(bytes))
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mod:{:016x}", self.0)
    }
}

/// Cryptographic-speed content hash powered by `blake3` for incremental caching and asset naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub fn from_raw(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn short_hex(self) -> String {
        format!(
            "{:02x}{:02x}{:02x}{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Classification of source modules handled natively by `zerho`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleKind {
    Js,
    Ts,
    Jsx,
    Tsx,
    Css,
    Json,
    Asset,
    Virtual,
}

impl ModuleKind {
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("js" | "mjs" | "cjs") => Self::Js,
            Some("ts" | "mts" | "cts") => Self::Ts,
            Some("jsx") => Self::Jsx,
            Some("tsx") => Self::Tsx,
            Some("css") => Self::Css,
            Some("json") => Self::Json,
            _ => Self::Asset,
        }
    }

    pub fn is_script(self) -> bool {
        matches!(self, Self::Js | Self::Ts | Self::Jsx | Self::Tsx)
    }
}

/// First-class frameworks supported natively in Rust without Node.js.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Framework {
    #[default]
    React,
    Preact,
    Solid,
    Vanilla,
}

impl Framework {
    pub fn parse(value: &str) -> Result<Self, ZerhoError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "react" => Ok(Self::React),
            "preact" => Ok(Self::Preact),
            "solid" => Ok(Self::Solid),
            "vanilla" => Ok(Self::Vanilla),
            other => Err(ZerhoError::Config(format!(
                "unsupported framework `{other}` (expected: react, preact, solid, vanilla)"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::React => "react",
            Self::Preact => "preact",
            Self::Solid => "solid",
            Self::Vanilla => "vanilla",
        }
    }
}

impl fmt::Display for Framework {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Structured error type shared across all `zerho` crates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZerhoError {
    Config(String),
    Resolve { specifier: String, importer: Option<PathBuf>, reason: String },
    Transform { path: PathBuf, reason: String },
    Io { path: Option<PathBuf>, reason: String },
    Plugin { plugin: String, reason: String },
    Server(String),
}

impl fmt::Display for ZerhoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(msg) => write!(f, "config error: {msg}"),
            Self::Resolve { specifier, importer, reason } => match importer {
                Some(imp) => write!(
                    f,
                    "failed to resolve `{specifier}` from `{}`: {reason}",
                    imp.display()
                ),
                None => write!(f, "failed to resolve `{specifier}`: {reason}"),
            },
            Self::Transform { path, reason } => {
                write!(f, "transform error in `{}`: {reason}", path.display())
            }
            Self::Io { path: Some(path), reason } => {
                write!(f, "I/O error for `{}`: {reason}", path.display())
            }
            Self::Io { path: None, reason } => write!(f, "I/O error: {reason}"),
            Self::Plugin { plugin, reason } => write!(f, "plugin `{plugin}` error: {reason}"),
            Self::Server(msg) => write!(f, "dev server error: {msg}"),
        }
    }
}

impl std::error::Error for ZerhoError {}

pub type ZerhoResult<T> = Result<T, ZerhoError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_kind_detects_extensions() {
        assert_eq!(ModuleKind::from_path(Path::new("App.tsx")), ModuleKind::Tsx);
        assert_eq!(ModuleKind::from_path(Path::new("styles.css")), ModuleKind::Css);
        assert_eq!(ModuleKind::from_path(Path::new("logo.svg")), ModuleKind::Asset);
    }

    #[test]
    fn content_hash_uses_blake3_deterministically() {
        let a = ContentHash::from_bytes(b"export const x = 1;");
        let b = ContentHash::from_bytes(b"export const x = 1;");
        let c = ContentHash::from_bytes(b"export const x = 2;");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.short_hex().len(), 8);
        assert_eq!(a.to_string().len(), 64);
    }
}
