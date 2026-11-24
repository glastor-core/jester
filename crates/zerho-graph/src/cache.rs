use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use redb::{Database, TableDefinition};
use zerho_core::{ContentHash, ZerhoError, ZerhoResult};

const CACHE_MAGIC: &[u8; 4] = b"ZHC1";
const DB_FILE_NAME: &str = "zerho-cache.redb";
const ARTIFACTS_TABLE: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("artifacts_v2");
const REMOTE_TIMEOUT: Duration = Duration::from_secs(2);

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
    pub remote_hits: u64,
}

#[derive(Clone)]
struct RemoteCacheClient {
    host: String,
    port: u16,
    base_path: String,
    token: Option<String>,
}

impl fmt::Debug for RemoteCacheClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteCacheClient")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("base_path", &self.base_path)
            .field("has_token", &self.token.is_some())
            .finish()
    }
}

impl RemoteCacheClient {
    fn parse(raw_url: &str, raw_token: Option<&str>) -> Option<Self> {
        let stripped = raw_url.trim().strip_prefix("http://")?;
        let (authority, path_part) = match stripped.find('/') {
            Some(idx) => (&stripped[..idx], &stripped[idx..]),
            None => (stripped, ""),
        };
        if authority.is_empty() {
            return None;
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !h.is_empty() => (h.to_owned(), p.parse::<u16>().ok()?),
            _ => (authority.to_owned(), 80),
        };
        let base_path = path_part.trim_end_matches('/').to_owned();
        let token = raw_token.and_then(resolve_token);

        Some(Self {
            host,
            port,
            base_path,
            token,
        })
    }

    fn connect(&self) -> Option<TcpStream> {
        let addr = format!("{}:{}", self.host, self.port)
            .to_socket_addrs()
            .ok()?
            .next()?;
        let stream = TcpStream::connect_timeout(&addr, REMOTE_TIMEOUT).ok()?;
        let _ = stream.set_read_timeout(Some(REMOTE_TIMEOUT));
        let _ = stream.set_write_timeout(Some(REMOTE_TIMEOUT));
        Some(stream)
    }

    fn object_path(&self, key: ContentHash) -> String {
        if self.base_path.is_empty() {
            format!("/{key}.zcache")
        } else {
            format!("{}/{key}.zcache", self.base_path)
        }
    }

    fn get(&self, key: ContentHash) -> Option<Vec<u8>> {
        let mut stream = self.connect()?;
        let path = self.object_path(key);
        let mut req = format!(
            "GET {path} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n",
            self.host, self.port
        );
        if let Some(tok) = &self.token {
            req.push_str("Authorization: Bearer ");
            req.push_str(tok);
            req.push_str("\r\n");
        }
        req.push_str("\r\n");

        stream.write_all(req.as_bytes()).ok()?;
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).ok()?;
        parse_http_ok_body(&resp)
    }

    fn put(&self, key: ContentHash, payload: &[u8]) {
        let Some(mut stream) = self.connect() else {
            return;
        };
        let path = self.object_path(key);
        let mut req = format!(
            "PUT {path} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.host,
            self.port,
            payload.len()
        );
        if let Some(tok) = &self.token {
            req.push_str("Authorization: Bearer ");
            req.push_str(tok);
            req.push_str("\r\n");
        }
        req.push_str("\r\n");

        if stream.write_all(req.as_bytes()).is_ok() && stream.write_all(payload).is_ok() {
            let mut sink = [0u8; 256];
            let _ = stream.read(&mut sink);
        }
    }
}

fn resolve_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(var_name) = trimmed.strip_prefix("env:") {
        std::env::var(var_name.trim()).ok().filter(|v| !v.is_empty())
    } else {
        Some(trimmed.to_owned())
    }
}

fn parse_http_ok_body(raw: &[u8]) -> Option<Vec<u8>> {
    let header_end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let headers = std::str::from_utf8(&raw[..header_end]).ok()?;
    let status_line = headers.lines().next()?;
    let status_code = status_line.split_whitespace().nth(1)?;
    if status_code != "200" {
        return None;
    }
    let body = &raw[header_end + 4..];
    let is_chunked = headers.lines().any(|line| {
        line.to_ascii_lowercase().starts_with("transfer-encoding:")
            && line.to_ascii_lowercase().contains("chunked")
    });
    if is_chunked {
        decode_chunked_body(body)
    } else {
        Some(body.to_vec())
    }
}

fn decode_chunked_body(mut input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let crlf = input.windows(2).position(|w| w == b"\r\n")?;
        let size_str = std::str::from_utf8(&input[..crlf]).ok()?.trim();
        let size_hex = size_str.split(';').next()?.trim();
        let size = usize::from_str_radix(size_hex, 16).ok()?;
        input = &input[crlf + 2..];
        if size == 0 {
            return Some(out);
        }
        if input.len() < size + 2 {
            return None;
        }
        out.extend_from_slice(&input[..size]);
        input = &input[size + 2..];
    }
}

/// Persistent, content-addressed cache backed by an embedded ACID `redb` B-tree database
/// (`blake3` keys) with optional HTTP remote team cache fallback.
pub struct DiskCache {
    dir: PathBuf,
    enabled: bool,
    db: OnceLock<Database>,
    remote: Option<RemoteCacheClient>,
    hits: AtomicU64,
    misses: AtomicU64,
    remote_hits: AtomicU64,
}

impl fmt::Debug for DiskCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiskCache")
            .field("dir", &self.dir)
            .field("enabled", &self.enabled)
            .field("remote", &self.remote)
            .field("hits", &self.hits.load(Ordering::Relaxed))
            .field("misses", &self.misses.load(Ordering::Relaxed))
            .field("remote_hits", &self.remote_hits.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl DiskCache {
    pub fn new(dir: impl Into<PathBuf>, enabled: bool) -> Self {
        Self::with_remote(dir, enabled, None, None)
    }

    /// Creates a `DiskCache` with an optional HTTP/S3-compatible remote team cache endpoint.
    pub fn with_remote(
        dir: impl Into<PathBuf>,
        enabled: bool,
        remote_url: Option<&str>,
        remote_token: Option<&str>,
    ) -> Self {
        let remote = remote_url.and_then(|url| RemoteCacheClient::parse(url, remote_token));
        Self {
            dir: dir.into(),
            enabled,
            db: OnceLock::new(),
            remote,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            remote_hits: AtomicU64::new(0),
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

        if let Some(artifact) = self.get_local(key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Some(artifact);
        }

        if let Some(remote) = &self.remote
            && let Some(bytes) = remote.get(key)
            && let Some(artifact) = decode_artifact(&bytes)
        {
            let _ = self.put_local_raw(key, &bytes);
            self.hits.fetch_add(1, Ordering::Relaxed);
            self.remote_hits.fetch_add(1, Ordering::Relaxed);
            return Some(artifact);
        }

        None
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
        let encoded = encode_artifact(artifact)?;
        self.put_local_raw(key, &encoded)?;
        if let Some(remote) = &self.remote {
            remote.put(key, &encoded);
        }
        Ok(())
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            remote_hits: self.remote_hits.load(Ordering::Relaxed),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Returns the path to the embedded `redb` database file.
    pub fn db_path(&self) -> PathBuf {
        self.dir.join(DB_FILE_NAME)
    }

    fn database(&self) -> ZerhoResult<&Database> {
        if let Some(db) = self.db.get() {
            return Ok(db);
        }
        fs::create_dir_all(&self.dir).map_err(|err| ZerhoError::Io {
            path: Some(self.dir.clone()),
            reason: err.to_string(),
        })?;
        let db_path = self.db_path();
        let db = Database::create(&db_path).map_err(|err| ZerhoError::Io {
            path: Some(db_path),
            reason: format!("failed to open redb cache: {err}"),
        })?;
        let _ = self.db.set(db);
        self.db.get().ok_or_else(|| ZerhoError::Io {
            path: Some(self.db_path()),
            reason: "redb cache initialization race".to_owned(),
        })
    }

    fn get_local(&self, key: ContentHash) -> Option<CachedArtifact> {
        let db = self.database().ok()?;
        let read_txn = db.begin_read().ok()?;
        let table = read_txn.open_table(ARTIFACTS_TABLE).ok()?;
        let guard = table.get(key.as_bytes().as_slice()).ok()??;
        decode_artifact(guard.value())
    }

    fn put_local_raw(&self, key: ContentHash, encoded: &[u8]) -> ZerhoResult<()> {
        let db = self.database()?;
        let write_txn = db.begin_write().map_err(|err| ZerhoError::Io {
            path: Some(self.db_path()),
            reason: format!("redb begin_write failed: {err}"),
        })?;
        {
            let mut table = write_txn.open_table(ARTIFACTS_TABLE).map_err(|err| ZerhoError::Io {
                path: Some(self.db_path()),
                reason: format!("redb open_table failed: {err}"),
            })?;
            table
                .insert(key.as_bytes().as_slice(), encoded)
                .map_err(|err| ZerhoError::Io {
                    path: Some(self.db_path()),
                    reason: format!("redb insert failed: {err}"),
                })?;
        }
        write_txn.commit().map_err(|err| ZerhoError::Io {
            path: Some(self.db_path()),
            reason: format!("redb commit failed: {err}"),
        })?;
        Ok(())
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

    Some(CachedArtifact {
        code,
        imports,
        output_hash,
    })
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(*cursor..*cursor + 4)?.try_into().ok()?;
    *cursor += 4;
    Some(u32::from_le_bytes(slice))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[test]
    fn disk_cache_persists_across_instances() {
        let temp_dir = std::env::temp_dir().join(format!(
            "zerho-redb-cache-test-{}",
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
            assert!(writer.db_path().is_file());
            assert_eq!(
                writer.stats(),
                CacheStats {
                    hits: 0,
                    misses: 1,
                    remote_hits: 0
                }
            );
        }

        // Simulate cold server restart with a fresh DiskCache instance.
        let reader = DiskCache::new(&temp_dir, true);
        let loaded = reader
            .get(key)
            .expect("cold restart should hit persistent redb cache");
        assert_eq!(loaded, artifact);
        assert_eq!(
            reader.stats(),
            CacheStats {
                hits: 1,
                misses: 0,
                remote_hits: 0
            }
        );

        drop(reader);
        let _ = fs::remove_dir_all(&temp_dir);
    }

    fn handle_mock_cache_request(
        mut stream: TcpStream,
        store: &Arc<Mutex<HashMap<String, Vec<u8>>>>,
    ) {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let mut header_end = None;
        let mut content_len = 0usize;
        while let Ok(n) = stream.read(&mut tmp) {
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if header_end.is_none()
                && let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
            {
                header_end = Some(pos + 4);
                let headers = String::from_utf8_lossy(&buf[..pos]);
                for line in headers.lines() {
                    if let Some(val) = line.strip_prefix("Content-Length:") {
                        content_len = val.trim().parse().unwrap_or(0);
                    }
                }
            }
            if let Some(h_end) = header_end
                && buf.len() >= h_end + content_len
            {
                break;
            }
        }
        let Some(h_end) = header_end else {
            return;
        };
        let header_str = String::from_utf8_lossy(&buf[..h_end]);
        let first_line = header_str.lines().next().unwrap_or("");
        let mut parts = first_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("").to_owned();

        if !header_str.contains("Authorization: Bearer team-secret") {
            let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
            return;
        }

        match method {
            "PUT" => {
                let body = buf[h_end..h_end + content_len].to_vec();
                if let Ok(mut map) = store.lock() {
                    map.insert(path, body);
                }
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            }
            "GET" => {
                let found = store.lock().ok().and_then(|m| m.get(&path).cloned());
                if let Some(body) = found {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                } else {
                    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                }
            }
            _ => {}
        }
    }

    #[test]
    fn remote_team_cache_syncs_and_backfills_local_redb() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock remote cache");
        let addr = listener.local_addr().expect("local addr");
        let store: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let store_clone = Arc::clone(&store);

        let handle = thread::spawn(move || {
            for _ in 0..2 {
                let Ok((stream, _)) = listener.accept() else {
                    break;
                };
                handle_mock_cache_request(stream, &store_clone);
            }
        });

        let dir_a = std::env::temp_dir().join("zerho-remote-cache-a");
        let dir_b = std::env::temp_dir().join("zerho-remote-cache-b");
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);

        let remote_url = format!("http://{addr}/cache");
        let key = DiskCache::compute_key("export const x = 42;", "ts", "vanilla", true, false);
        let artifact = CachedArtifact {
            code: "export const x = 42;\n".to_owned(),
            imports: vec![],
            output_hash: ContentHash::from_bytes(b"export const x = 42;\n"),
        };

        // Developer A populates local + remote cache.
        {
            let dev_a = DiskCache::with_remote(&dir_a, true, Some(&remote_url), Some("team-secret"));
            dev_a.put(key, &artifact).expect("put should succeed");
        }

        // Developer B starts with an empty local cache and fetches from remote team cache.
        {
            let dev_b = DiskCache::with_remote(&dir_b, true, Some(&remote_url), Some("team-secret"));
            let fetched = dev_b.get(key).expect("should hit remote team cache");
            assert_eq!(fetched, artifact);
            assert_eq!(dev_b.stats().remote_hits, 1);
            assert_eq!(dev_b.stats().hits, 1);

            // Second lookup should hit local redb without hitting the remote server again.
            let local_hit = dev_b.get(key).expect("should hit backfilled local redb");
            assert_eq!(local_hit, artifact);
            assert_eq!(dev_b.stats().remote_hits, 1);
            assert_eq!(dev_b.stats().hits, 2);
        }

        let _ = handle.join();
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }
}
