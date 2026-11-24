use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::TcpListener;
use zerho_core::{ZerhoError, ZerhoResult};

/// Configuration for the standalone pure-Rust HTTP remote team cache daemon (`zerho cache-server`).
#[derive(Debug, Clone)]
pub struct CacheServerConfig {
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub token: Option<String>,
}

impl Default for CacheServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 9000,
            data_dir: PathBuf::from(".zerho-cache-server"),
            token: None,
        }
    }
}

#[derive(Debug)]
struct CacheServerState {
    data_dir: PathBuf,
    token: Option<String>,
    hits: AtomicU64,
    misses: AtomicU64,
    writes: AtomicU64,
}

/// Pure-Rust HTTP/CAS remote team cache daemon compatible with `[cache].remote_url`.
#[derive(Debug)]
pub struct CacheServer {
    config: CacheServerConfig,
    state: Arc<CacheServerState>,
}

impl CacheServer {
    pub fn new(config: CacheServerConfig) -> Self {
        let state = Arc::new(CacheServerState {
            data_dir: config.data_dir.clone(),
            token: config.token.clone().filter(|t| !t.trim().is_empty()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            writes: AtomicU64::new(0),
        });
        Self { config, state }
    }

    /// Builds the Axum router serving `/health`, `/stats`, `/cache/{key}`, and `/{key}`.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/health", get(handle_health))
            .route("/stats", get(handle_stats))
            .route(
                "/cache/{key}",
                get(handle_get_object).put(handle_put_object),
            )
            .route("/{key}", get(handle_get_object).put(handle_put_object))
            .with_state(Arc::clone(&self.state))
    }

    /// Starts the HTTP remote team cache daemon until `Ctrl-C`.
    pub async fn serve(&self) -> ZerhoResult<()> {
        fs::create_dir_all(&self.config.data_dir).map_err(|err| {
            ZerhoError::Server(format!(
                "failed to create cache data dir `{}`: {err}",
                self.config.data_dir.display()
            ))
        })?;

        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|err| ZerhoError::Server(format!("invalid bind address: {err}")))?;

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|err| ZerhoError::Server(format!("failed to bind {addr}: {err}")))?;

        println!(
            "zerho cache-server running (100% Safe Rust · HTTP CAS)\n  \
             endpoint: http://{addr}/cache\n  \
             data_dir: {}\n  \
             auth:     {}\n  \
             health:   http://{addr}/health\n",
            self.config.data_dir.display(),
            if self.state.token.is_some() {
                "Bearer token enabled"
            } else {
                "open (set --token or ZERHO_CACHE_TOKEN for production)"
            }
        );

        axum::serve(listener, self.router())
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|err| ZerhoError::Server(err.to_string()))
    }
}

fn check_auth(headers: &HeaderMap, expected: Option<&str>) -> bool {
    let Some(secret) = expected else {
        return true;
    };
    let Some(raw) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    raw.strip_prefix("Bearer ").is_some_and(|tok| tok == secret)
}

fn validate_cache_key(key: &str) -> bool {
    let Some(hex) = key.strip_suffix(".zcache") else {
        return false;
    };
    (16..=128).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

async fn handle_health() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        "{\"status\":\"ok\",\"service\":\"zerho-cache-server\"}",
    )
}

async fn handle_stats(headers: HeaderMap, State(state): State<Arc<CacheServerState>>) -> Response {
    if !check_auth(&headers, state.token.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let body = format!(
        "{{\"hits\":{},\"misses\":{},\"writes\":{}}}",
        state.hits.load(Ordering::Relaxed),
        state.misses.load(Ordering::Relaxed),
        state.writes.load(Ordering::Relaxed),
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        body,
    )
        .into_response()
}

async fn handle_get_object(
    headers: HeaderMap,
    AxumPath(key): AxumPath<String>,
    State(state): State<Arc<CacheServerState>>,
) -> Response {
    if !check_auth(&headers, state.token.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    if !validate_cache_key(&key) {
        return (StatusCode::BAD_REQUEST, "invalid cache key").into_response();
    }
    let path = state.data_dir.join(&key);
    if let Ok(bytes) = fs::read(&path) {
        state.hits.fetch_add(1, Ordering::Relaxed);
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response()
    } else {
        state.misses.fetch_add(1, Ordering::Relaxed);
        (StatusCode::NOT_FOUND, "not found").into_response()
    }
}

async fn handle_put_object(
    headers: HeaderMap,
    AxumPath(key): AxumPath<String>,
    State(state): State<Arc<CacheServerState>>,
    body: Bytes,
) -> Response {
    if !check_auth(&headers, state.token.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    if !validate_cache_key(&key) {
        return (StatusCode::BAD_REQUEST, "invalid cache key").into_response();
    }
    if fs::create_dir_all(&state.data_dir).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable").into_response();
    }
    let dest = state.data_dir.join(&key);
    let tmp = state.data_dir.join(format!("{key}.tmp"));
    if fs::write(&tmp, &body).is_err() || fs::rename(&tmp, &dest).is_err() {
        let _ = fs::remove_file(&tmp);
        return (StatusCode::INTERNAL_SERVER_ERROR, "write failed").into_response();
    }
    state.writes.fetch_add(1, Ordering::Relaxed);
    (StatusCode::CREATED, "stored").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerho_core::ContentHash;
    use zerho_graph::{CachedArtifact, DiskCache};

    #[tokio::test]
    async fn cache_server_serves_and_persists_remote_team_cache_artifacts() {
        let root =
            std::env::temp_dir().join(format!("zerho-cache-server-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let server_dir = root.join("server");
        let producer_dir = root.join("producer");
        let consumer_dir = root.join("consumer");

        let server = CacheServer::new(CacheServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 0,
            data_dir: server_dir,
            token: Some("team-secret-token".to_owned()),
        });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, server.router()).await;
        });

        let remote_url = format!("http://{addr}/cache");
        let key = ContentHash::from_bytes(b"shared-team-module-v1");
        let artifact = CachedArtifact {
            code: "export const team = 42;\n".to_owned(),
            imports: vec!["./dep.ts".to_owned()],
            output_hash: ContentHash::from_bytes(b"export const team = 42;\n"),
        };

        // Run blocking DiskCache calls on spawn_blocking so tokio runtime keeps serving requests.
        let url_a = remote_url.clone();
        let art_clone = artifact.clone();
        tokio::task::spawn_blocking(move || {
            let cache_a =
                DiskCache::with_remote(producer_dir, true, Some(&url_a), Some("team-secret-token"));
            cache_a.put(key, &art_clone).expect("cache put");
        })
        .await
        .expect("producer put");

        let loaded = tokio::task::spawn_blocking(move || {
            let cache_b = DiskCache::with_remote(
                consumer_dir,
                true,
                Some(&remote_url),
                Some("team-secret-token"),
            );
            cache_b.get(key)
        })
        .await
        .expect("consumer get");

        assert_eq!(loaded, Some(artifact));
        let _ = fs::remove_dir_all(&root);
    }
}
