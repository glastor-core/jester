use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use zerho_core::{ContentHash, ZerhoError, ZerhoResult};

const CACHE_MAGIC: &[u8; 4] = b"ZHC1";

/// Cached result of transforming a single source module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedArtifact {
    pub code: String,
    pub imports: Vec<String>,
    pub output_hash: ContentHash,
}

/// Live hit/miss counters exposed to `/__inspect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
}

/// Persistent, content-addressed disk cache backed by `blake3` keys.
#[derive(Debug)]
pub struct DiskCache {
    dir: PathBuf,
    enabled: bool,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl DiskCache {
    pub fn new(dir: impl Into<PathBuf>, enabled: bool) -> Self {
        Self {
            dir: dir.into(),
            enabled,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Computes a deterministic cache key from all transform inputs.
    pub fn compute_key(
        source: &str,
        kind_tag: &str,
        framework_tag: &str,
        is_dev: bool,
        minify_css: bool,
    ) -> ContentHash {
        let mut seed = String::with_capacity(source.len() + 64);
        seed.push_str("zerho-cache-v1|");
        seed.push_str(kind_tag);
        seed.push('|');
        seed.push_str(framework_tag);
        seed.push('|');
        seed.push(if is_dev { '1' } else { '0' });
        seed.push(if minify_css { '1' } else { '0' });
        seed.push('|');
        seed.push_str(source);
        ContentHash::from_bytes(seed.as_bytes())
    }

    pub fn get(&self, key: ContentHash) -> Option<CachedArtifact> {
        if !self.enabled {
            return None;
        }
        let path = self.entry_path(key);
        let bytes = fs::read(&path).ok()?;
        let artifact = decode_artifact(&bytes)?;
        self.hits.fetch_add(1, Ordering::Relaxed);
        Some(artifact)
    }

    pub fn record_miss(&self) {
        if self.enabled {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn put(&self, key: ContentHash, artifact: &CachedArtifact) -> ZerhoResult<()> {
        if !self.enabled {
            return Ok(());
        }
        let path = self.entry_path(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| ZerhoError::Io {
                path: Some(parent.to_path_buf()),
                reason: err.to_string(),
            })?;
        }

        let encoded = encode_artifact(artifact)?;
        let tmp_path = path.with_extension("zcache.tmp");
        fs::write(&tmp_path, encoded).map_err(|err| ZerhoError::Io {
            path: Some(tmp_path.clone()),
            reason: err.to_string(),
        })?;
        // On Windows `rename` can fail if destination already exists; remove best-effort first.
        let _ = fs::remove_file(&path);
        fs::rename(&tmp_path, &path).map_err(|err| ZerhoError::Io {
            path: Some(path),
            reason: err.to_string(),
        })?;
        Ok(())
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn entry_path(&self, key: ContentHash) -> PathBuf {
        let hex = key.to_string();
        self.dir.join("v1").join(&hex[..2]).join(format!("{hex}.zcache"))
    }
}

fn encode_artifact(artifact: &CachedArtifact) -> ZerhoResult<Vec<u8>> {
    let mut buf = Vec::with_capacity(48 + artifact.code.len());
    buf.extend_from_slice(CACHE_MAGIC);
    buf.extend_from_slice(artifact.output_hash.as_bytes());

    let import_count = u32::try_from(artifact.imports.len())
        .map_err(|_| ZerhoError::Config("too many imports in module for cache".to_owned()))?;
    buf.extend_from_slice(&import_count.to_le_bytes());

    for imp in &artifact.imports {
        let len = u32::try_from(imp.len())
            .map_err(|_| ZerhoError::Config("import specifier too large for cache".to_owned()))?;
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(imp.as_bytes());
    }

    let code_len = u32::try_from(artifact.code.len())
        .map_err(|_| ZerhoError::Config("module output too large for cache".to_owned()))?;
    buf.extend_from_slice(&code_len.to_le_bytes());
    buf.extend_from_slice(artifact.code.as_bytes());
    Ok(buf)
}

fn decode_artifact(bytes: &[u8]) -> Option<CachedArtifact> {
    if bytes.len() < 4 + 32 + 4 || &bytes[..4] != CACHE_MAGIC {
        return None;
    }
    let mut cursor = 4;

    let raw_hash: [u8; 32] = bytes[cursor..cursor + 32].try_into().ok()?;
    let output_hash = ContentHash::from_raw(raw_hash);
    cursor += 32;

    let import_count = read_u32(bytes, &mut cursor)? as usize;
    let mut imports = Vec::with_capacity(import_count);
    for _ in 0..import_count {
        let len = read_u32(bytes, &mut cursor)? as usize;
        let slice = bytes.get(cursor..cursor + len)?;
        cursor += len;
        imports.push(std::str::from_utf8(slice).ok()?.to_owned());
    }

    let code_len = read_u32(bytes, &mut cursor)? as usize;
    let code_slice = bytes.get(cursor..cursor + code_len)?;
    let code = std::str::from_utf8(code_slice).ok()?.to_owned();

    Some(CachedArtifact { code, imports, output_hash })
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(*cursor..*cursor + 4)?.try_into().ok()?;
    *cursor += 4;
    Some(u32::from_le_bytes(slice))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_cache_persists_across_instances() {
        let temp_dir = std::env::temp_dir().join(format!(
            "zerho-cache-test-{}",
            ContentHash::from_bytes(b"disk-cache-unit-test").short_hex()
        ));
        let _ = fs::remove_dir_all(&temp_dir);

        let key = DiskCache::compute_key("export const a: number = 1;", "ts", "react", true, false);
        let artifact = CachedArtifact {
            code: "export const a = 1;\n".to_owned(),
            imports: vec!["./dep".to_owned(), "react".to_owned()],
            output_hash: ContentHash::from_bytes(b"export const a = 1;\n"),
        };

        {
            let writer = DiskCache::new(&temp_dir, true);
            assert!(writer.get(key).is_none());
            writer.record_miss();
            writer.put(key, &artifact).expect("cache write should succeed");
            assert_eq!(writer.stats(), CacheStats { hits: 0, misses: 1 });
        }

        // Simulate cold server restart with a fresh DiskCache instance.
        let reader = DiskCache::new(&temp_dir, true);
        let loaded = reader.get(key).expect("cold restart should hit persistent disk cache");
        assert_eq!(loaded, artifact);
        assert_eq!(reader.stats(), CacheStats { hits: 1, misses: 0 });

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
