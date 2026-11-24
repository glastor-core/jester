use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{any, get};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use zerho_config::{ZerhoConfig, build_import_meta_env_json, load_env_vars};
use zerho_core::{ModuleId, ModuleKind, ZerhoError, ZerhoResult};
use zerho_graph::{CachedArtifact, DiskCache, ModuleGraph};
use zerho_plugin::{PluginRegistry, register_wasm_plugins};
use zerho_resolver::{OxcResolver, Resolver};
use zerho_transform::{
    TransformInput, TransformPipeline, is_likely_commonjs, lower_esm_for_bundle,
    prebundle_cjs_graph_to_esm, rewrite_imports, wrap_cjs_as_esm, wrap_css_as_hmr_module,
    wrap_css_with_exports_as_hmr_module,
};

const INSPECT_HTML: &str = include_str!("inspect.html");

const HMR_CLIENT_JS: &str = include_str!("hmr_client.js");

/// HMR update payload dispatched to connected browser clients over WebSocket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HmrUpdate {
    pub changed: ModuleId,
    pub affected: Vec<ModuleId>,
}

#[derive(Debug)]
struct ServerState {
    config: ZerhoConfig,
    root: PathBuf,
    public_dir: PathBuf,
    env_json: String,
    graph: RwLock<ModuleGraph>,
    cache: DiskCache,
    resolver: OxcResolver,
    pipeline: TransformPipeline,
    hmr_tx: broadcast::Sender<String>,
}

/// Zero-Node development server powered by `axum`, `tokio`, `notify`, `oxc`, and `lightningcss`.
#[derive(Debug)]
pub struct DevServer {
    state: Arc<ServerState>,
}

impl DevServer {
    pub fn new(config: ZerhoConfig) -> Self {
        let root = config
            .project
            .root
            .canonicalize()
            .unwrap_or_else(|_| config.project.root.clone());
        let public_dir = {
            let p = root.join(&config.project.public_dir);
            p.canonicalize().unwrap_or(p)
        };
        let env_vars = load_env_vars(&root, "development", &config.project.env_prefix);
        let env_json = build_import_meta_env_json("development", true, &env_vars);
        let cache_dir = if config.cache.dir.is_absolute() {
            config.cache.dir.clone()
        } else {
            root.join(&config.cache.dir)
        };
        let cache = DiskCache::with_remote(
            cache_dir,
            config.cache.enabled,
            config.cache.remote_url.as_deref(),
            config.cache.remote_token.as_deref(),
        );
        let mut registry = PluginRegistry::new();
        if let Err(err) = register_wasm_plugins(
            &mut registry,
            &root,
            &config.plugins.wasm,
            config.plugins.fuel_limit,
        ) {
            eprintln!("zerho warning: failed to load WASM plugin: {err}");
        }
        let resolver = OxcResolver::new(&root);
        let (hmr_tx, _) = broadcast::channel(64);
        let state = Arc::new(ServerState {
            config,
            root,
            public_dir,
            env_json,
            graph: RwLock::new(ModuleGraph::new()),
            cache,
            resolver,
            pipeline: TransformPipeline::new(registry),
            hmr_tx,
        });
        Self { state }
    }

    pub fn config(&self) -> &ZerhoConfig {
        &self.state.config
    }

    /// Computes the HMR invalidation set for a changed module.
    pub fn compute_hmr_update(&self, changed: ModuleId) -> HmrUpdate {
        let affected = self
            .state
            .graph
            .read()
            .map(|g| g.invalidate(changed))
            .unwrap_or_default();
        HmrUpdate { changed, affected }
    }

    /// Builds the Axum router serving HTML, on-the-fly transformed modules, `/@zerho/deps`, `/__inspect`, and HMR WS.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/", get(serve_index))
            .route("/index.html", get(serve_index))
            .route("/@zerho/client", get(serve_hmr_client))
            .route("/@zerho/hmr", get(hmr_ws_handler))
            .route("/@zerho/deps/{*pkg}", get(serve_dep))
            .route("/@zerho/bundle", get(serve_partial_bundle))
            .route("/__inspect", get(serve_inspect_ui))
            .route("/__inspect/api", get(serve_inspect_api))
            .route("/{*path}", any(serve_module))
            .with_state(Arc::clone(&self.state))
    }

    /// Starts the real HTTP + WebSocket HMR dev server and file watcher.
    pub async fn serve(&self) -> ZerhoResult<()> {
        self.state.config.validate()?;

        let addr: SocketAddr = format!(
            "{}:{}",
            self.state.config.server.host, self.state.config.server.port
        )
        .parse()
        .map_err(|err| ZerhoError::Server(format!("invalid bind address: {err}")))?;

        let _watcher = if self.state.config.server.hmr {
            Some(start_file_watcher(Arc::clone(&self.state))?)
        } else {
            None
        };

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|err| ZerhoError::Server(format!("failed to bind {addr}: {err}")))?;

        println!(
            "zerho dev server running (100% Rust / Zero-Node)\n  \
             project:   {} ({})\n  \
             local:     http://{addr}\n  \
             inspect:   http://{addr}/__inspect\n  \
             cache:     {}\n  \
             hmr:       {}\n",
            self.state.config.project.name,
            self.state.config.project.framework,
            if self.state.config.cache.enabled {
                self.state.cache.dir().display().to_string()
            } else {
                "disabled".to_owned()
            },
            if self.state.config.server.hmr {
                "enabled (ws://.../@zerho/hmr)"
            } else {
                "disabled"
            }
        );

        axum::serve(listener, self.router())
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|err| ZerhoError::Server(err.to_string()))
    }
}

/// Static production preview server (`zerho preview`) serving `build.out_dir`.
#[derive(Debug)]
pub struct PreviewServer {
    config: ZerhoConfig,
    dist_dir: PathBuf,
}

impl PreviewServer {
    pub fn new(config: ZerhoConfig) -> Self {
        let root = config
            .project
            .root
            .canonicalize()
            .unwrap_or_else(|_| config.project.root.clone());
        let dist = if config.build.out_dir.is_absolute() {
            config.build.out_dir.clone()
        } else {
            root.join(&config.build.out_dir)
        };
        let dist_dir = dist.canonicalize().unwrap_or(dist);
        Self { config, dist_dir }
    }

    /// Builds the Axum router serving built production assets with Brotli negotiation.
    pub fn router(&self) -> Router {
        let dist = Arc::new(self.dist_dir.clone());
        Router::new()
            .route("/", get(serve_preview_index))
            .route("/{*path}", get(serve_preview_asset))
            .with_state(dist)
    }

    pub async fn serve(&self) -> ZerhoResult<()> {
        self.config.validate()?;
        if !self.dist_dir.is_dir() {
            return Err(ZerhoError::Server(format!(
                "build output directory `{}` does not exist (run `zerho build` first)",
                self.dist_dir.display()
            )));
        }

        let addr: SocketAddr = format!("{}:{}", self.config.server.host, self.config.server.port)
            .parse()
            .map_err(|err| ZerhoError::Server(format!("invalid bind address: {err}")))?;

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|err| ZerhoError::Server(format!("failed to bind {addr}: {err}")))?;

        println!(
            "zerho preview server running\n  dist:   {}\n  local:  http://{addr}\n",
            self.dist_dir.display()
        );

        let app = self.router();

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|err| ZerhoError::Server(err.to_string()))
    }
}

async fn serve_preview_index(
    headers: axum::http::HeaderMap,
    State(dist): State<Arc<PathBuf>>,
) -> Response {
    let accepts_br = accepts_brotli(&headers);
    serve_static_file(&dist, &dist.join("index.html"), accepts_br).await
}

async fn serve_preview_asset(
    AxumPath(req_path): AxumPath<String>,
    headers: axum::http::HeaderMap,
    State(dist): State<Arc<PathBuf>>,
) -> Response {
    let accepts_br = accepts_brotli(&headers);
    let candidate = dist.join(req_path.trim_start_matches('/'));
    if candidate.is_file() {
        serve_static_file(&dist, &candidate, accepts_br).await
    } else {
        serve_static_file(&dist, &dist.join("index.html"), accepts_br).await
    }
}

fn accepts_brotli(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|enc| enc.split(',').any(|part| part.trim().starts_with("br")))
}

async fn serve_static_file(dist_root: &Path, target: &Path, accepts_br: bool) -> Response {
    let Ok(canonical) = target.canonicalize() else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    if !canonical.starts_with(dist_root) {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    let content_type = match canonical.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    };

    if accepts_br {
        let mut br_os = canonical.as_os_str().to_os_string();
        br_os.push(".br");
        let br_path = PathBuf::from(br_os);
        if br_path.is_file()
            && let Ok(br_bytes) = tokio::fs::read(&br_path).await
        {
            return (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
                    (header::CONTENT_ENCODING, HeaderValue::from_static("br")),
                    (header::VARY, HeaderValue::from_static("Accept-Encoding")),
                ],
                br_bytes,
            )
                .into_response();
        }
    }

    let Ok(bytes) = tokio::fs::read(&canonical).await else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static(content_type))],
        bytes,
    )
        .into_response()
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("\nzerho dev server stopped.");
}

fn start_file_watcher(state: Arc<ServerState>) -> ZerhoResult<notify::RecommendedWatcher> {
    let root = state.root.clone();
    let watch_root = state.root.clone();
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        let Ok(event) = res else { return };
        if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
            return;
        }
        for path in event.paths {
            if is_ignored_path(&path) || path.is_dir() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(&root) else {
                continue;
            };
            let web_path = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
            let id = ModuleId::from_specifier(&path.to_string_lossy());
            if let Ok(graph) = state.graph.read() {
                let _ = graph.invalidate(id);
            }
            let kind = ModuleKind::from_path(&path);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());

            // Pre-validate modified script/CSS files so syntax errors immediately pop the Error Overlay.
            if (kind.is_script() || kind == ModuleKind::Css)
                && let Ok(source) = std::fs::read_to_string(&path)
                && let Err(err) = transform_with_cache(&state, &path, kind, source)
            {
                let payload = format!(
                    "{{\"type\":\"error\",\"file\":\"{}\",\"message\":\"{}\"}}",
                    escape_json(&web_path),
                    escape_json(&err.to_string())
                );
                let _ = state.hmr_tx.send(payload);
                continue;
            }

            let event_type = if kind == ModuleKind::Css {
                "css-update"
            } else if kind.is_script() {
                "js-update"
            } else {
                "full-reload"
            };
            let payload = format!(
                "{{\"type\":\"{event_type}\",\"path\":\"{}\",\"timestamp\":{ts}}}",
                escape_json(&web_path)
            );
            let _ = state.hmr_tx.send(payload);
        }
    })
    .map_err(|err| ZerhoError::Server(format!("failed to initialize file watcher: {err}")))?;

    watcher
        .watch(&watch_root, RecursiveMode::Recursive)
        .map_err(|err| {
            ZerhoError::Server(format!("failed to watch `{}`: {err}", watch_root.display()))
        })?;

    Ok(watcher)
}

fn is_ignored_path(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c,
            Component::Normal(name)
                if name == ".git" || name == "target" || name == "dist" || name == ".zerho" || name == "node_modules"
        )
    })
}

async fn serve_index(State(state): State<Arc<ServerState>>) -> Response {
    let index_path = state.root.join("index.html");
    match tokio::fs::read_to_string(&index_path).await {
        Ok(html) => {
            let injected = if state.config.server.hmr {
                inject_hmr_client(&html)
            } else {
                html
            };
            Html(injected).into_response()
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            Html(format!(
                "<h1>zerho</h1><p>No <code>index.html</code> found in <code>{}</code>.</p>",
                state.root.display()
            )),
        )
            .into_response(),
    }
}

fn inject_hmr_client(html: &str) -> String {
    const TAG: &str = r#"<script type="module" src="/@zerho/client"></script>"#;
    if let Some(pos) = html.find("</head>") {
        let mut out = String::with_capacity(html.len() + TAG.len() + 4);
        out.push_str(&html[..pos]);
        out.push_str("  ");
        out.push_str(TAG);
        out.push('\n');
        out.push_str(&html[pos..]);
        out
    } else {
        format!("{html}\n{TAG}\n")
    }
}

async fn serve_hmr_client() -> Response {
    js_response(HMR_CLIENT_JS.to_owned(), "no-cache")
}

async fn hmr_ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<ServerState>>) -> Response {
    let rx = state.hmr_tx.subscribe();
    ws.on_upgrade(move |socket| handle_ws(socket, rx))
}

async fn handle_ws(mut socket: WebSocket, mut rx: broadcast::Receiver<String>) {
    while let Ok(msg) = rx.recv().await {
        if socket.send(Message::Text(msg.into())).await.is_err() {
            break;
        }
    }
}

async fn serve_inspect_ui() -> Response {
    Html(INSPECT_HTML).into_response()
}

async fn serve_inspect_api(State(state): State<Arc<ServerState>>) -> Response {
    let (metrics, items_json) = state.graph.read().map_or_else(
        |_| {
            (
                zerho_graph::GraphStats {
                    module_count: 0,
                    edge_count: 0,
                },
                String::from("[]"),
            )
        },
        |g| {
            let items: Vec<String> = g
                .snapshot()
                .into_iter()
                .map(|node| {
                    let rel = node
                        .path
                        .strip_prefix(&state.root)
                        .unwrap_or(&node.path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    let bytes = std::fs::metadata(&node.path).map_or(0, |m| m.len());
                    format!(
                        "{{\"path\":\"/{rel}\",\"kind\":\"{}\",\"hash\":\"{}\",\"bytes\":{bytes},\"imports\":{},\"importers\":{}}}",
                        module_kind_tag(node.kind),
                        node.hash.short_hex(),
                        node.imports.len(),
                        node.importers.len()
                    )
                })
                .collect();
            (g.stats(), format!("[{}]", items.join(",")))
        },
    );
    let cache = state.cache.stats();
    let body = format!(
        "{{\"project\":\"{}\",\"framework\":\"{}\",\"modules\":{},\"edges\":{},\"cache_hits\":{},\"cache_remote_hits\":{},\"cache_misses\":{},\"partial_bundling\":{},\"items\":{items_json}}}",
        state.config.project.name,
        state.config.project.framework,
        metrics.module_count,
        metrics.edge_count,
        cache.hits,
        cache.remote_hits,
        cache.misses,
        state.config.server.partial_bundling
    );
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        )],
        body,
    )
        .into_response()
}

/// Zero-waterfall in-memory partial bundler for dev mode (`GET /@zerho/bundle`).
/// Combines the local static script subgraph into a single HTTP response while keeping
/// `DiskCache` and CSS HMR imports active.
async fn serve_partial_bundle(State(state): State<Arc<ServerState>>) -> Response {
    if !state.config.server.partial_bundling {
        return (
            StatusCode::NOT_FOUND,
            "partial bundling disabled in zerho.toml",
        )
            .into_response();
    }

    let entry_path = state.root.join(&state.config.project.entry);
    let Ok(entry_canon) = entry_path.canonicalize() else {
        return (StatusCode::NOT_FOUND, "entry file not found").into_response();
    };

    let mut visited = std::collections::HashSet::new();
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut css_imports: Vec<String> = Vec::new();

    if let Err(err) = collect_partial_dev_modules(
        &state,
        &entry_canon,
        &mut visited,
        &mut entries,
        &mut css_imports,
    ) {
        return js_response(
            render_error_overlay_module("/@zerho/bundle", &err.to_string()),
            "no-cache",
        );
    }

    let entry_key =
        path_to_web_url(&state.root, &entry_canon).unwrap_or_else(|| "/src/main.ts".to_owned());

    let mut out = String::new();
    for css in css_imports {
        out.push_str("import \"");
        out.push_str(&css);
        out.push_str("\";\n");
    }
    out.push_str("import.meta.env = ");
    out.push_str(&state.env_json);
    out.push_str(";\nconst __zerho_registry = {\n");
    for (id, body) in entries {
        out.push_str("  \"");
        out.push_str(&id);
        out.push_str("\": function(__zerho_require, __zerho_exports) {\n");
        out.push_str(&body);
        out.push_str("\n  },\n");
    }
    out.push_str(
        "};\n\
         const __zerho_cache = {};\n\
         function __zerho_require(id) {\n\
           if (__zerho_cache[id]) return __zerho_cache[id].exports;\n\
           const f = __zerho_registry[id];\n\
           if (!f) throw new Error(\"[zerho partial] missing: \" + id);\n\
           const m = { exports: {} };\n\
           __zerho_cache[id] = m;\n\
           f(__zerho_require, m.exports);\n\
           return m.exports;\n\
         }\n\
         __zerho_require(\"",
    );
    out.push_str(&entry_key);
    out.push_str("\");\n");

    js_response(out, "no-cache")
}

fn collect_partial_dev_modules(
    state: &ServerState,
    path: &Path,
    visited: &mut std::collections::HashSet<PathBuf>,
    entries: &mut Vec<(String, String)>,
    css_imports: &mut Vec<String>,
) -> ZerhoResult<()> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canonical.clone()) {
        return Ok(());
    }

    let kind = ModuleKind::from_path(&canonical);
    if kind == ModuleKind::Css {
        if let Some(url) = path_to_web_url(&state.root, &canonical) {
            css_imports.push(url);
        }
        return Ok(());
    }
    if !kind.is_script() {
        return Ok(());
    }

    let source = std::fs::read_to_string(&canonical).map_err(|err| ZerhoError::Io {
        path: Some(canonical.clone()),
        reason: err.to_string(),
    })?;
    let artifact = transform_with_cache(state, &canonical, kind, source)?;

    for spec in &artifact.imports {
        if let Ok(resolved) = state.resolver.resolve(spec, Some(&canonical))
            && !resolved.is_virtual
        {
            collect_partial_dev_modules(state, &resolved.path, visited, entries, css_imports)?;
        }
    }

    let lowered = lower_esm_for_bundle(&artifact.code, &canonical, |spec| {
        let resolved = state.resolver.resolve(spec, Some(&canonical)).ok()?;
        if resolved.kind == ModuleKind::Css || resolved.is_virtual {
            return None;
        }
        let target = resolved.path.canonicalize().unwrap_or(resolved.path);
        path_to_web_url(&state.root, &target)
    })?;

    if let Some(id) = path_to_web_url(&state.root, &canonical) {
        entries.push((id, lowered));
    }
    Ok(())
}

/// Resolves and serves bare `node_modules` packages as browser ESM without Node.js.
async fn serve_dep(
    AxumPath(pkg): AxumPath<String>,
    State(state): State<Arc<ServerState>>,
) -> Response {
    let resolved = match state.resolver.resolve(&pkg, None) {
        Ok(res) => res,
        Err(err) => {
            return (
                StatusCode::NOT_FOUND,
                format!("// zerho dep resolve error for `{pkg}`: {err}\n"),
            )
                .into_response();
        }
    };

    let Ok(raw_source) = tokio::fs::read_to_string(&resolved.path).await else {
        return (StatusCode::NOT_FOUND, "dependency file not found").into_response();
    };

    // Automatic deep CJS -> ESM pre-bundling (resolving nested require() calls, cached in redb).
    let source = if resolved.kind == ModuleKind::Js && is_likely_commonjs(&raw_source) {
        let entry_canon = resolved
            .path
            .canonicalize()
            .unwrap_or_else(|_| resolved.path.clone());
        prebundle_cjs_graph_to_esm(&entry_canon, &raw_source, |spec, importer| {
            let dep = state.resolver.resolve(spec, Some(importer)).ok()?;
            if dep.is_virtual {
                return None;
            }
            let dep_canon = dep.path.canonicalize().unwrap_or(dep.path);
            let src = std::fs::read_to_string(&dep_canon).ok()?;
            Some((dep_canon, src))
        })
        .unwrap_or_else(|_| wrap_cjs_as_esm(&raw_source))
    } else {
        raw_source
    };

    let artifact = match transform_with_cache(&state, &resolved.path, resolved.kind, source) {
        Ok(art) => art,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("// zerho dep transform error: {err}\n"),
            )
                .into_response();
        }
    };

    let rewritten = match rewrite_imports(&artifact.code, &resolved.path, |spec| {
        if is_bare_specifier(spec) {
            Some(format!("/@zerho/deps/{spec}"))
        } else {
            state
                .resolver
                .resolve(spec, Some(&resolved.path))
                .ok()
                .and_then(|target| path_to_web_url(&state.root, &target.path))
        }
    }) {
        Ok(code) => code,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("// zerho dep rewrite error: {err}\n"),
            )
                .into_response();
        }
    };

    js_response(rewritten, "public, max-age=31536000, immutable")
}

async fn serve_module(
    AxumPath(req_path): AxumPath<String>,
    State(state): State<Arc<ServerState>>,
    request: Request,
) -> Response {
    let clean_specifier = format!("/{}", req_path.trim_start_matches('/'));

    // 1. Check `[server.proxy]` rules first.
    if let Some(target_base) = find_proxy_target(&state.config.server.proxy, &clean_specifier) {
        return forward_http_proxy(&target_base, &clean_specifier, request).await;
    }

    // 2. Serve static files from `project.public_dir` if present (e.g. `/logo.svg`, `/favicon.ico`).
    let public_candidate = state.public_dir.join(req_path.trim_start_matches('/'));
    if public_candidate.is_file() {
        return serve_static_file(&state.public_dir, &public_candidate, false).await;
    }

    let resolved = match state.resolver.resolve(&clean_specifier, None) {
        Ok(res) => res,
        Err(err) => {
            return (
                StatusCode::NOT_FOUND,
                format!("// zerho resolve error: {err}\n"),
            )
                .into_response();
        }
    };

    // Guard against path traversal outside project root.
    let canonical = match resolved.path.canonicalize() {
        Ok(p) if p.starts_with(&state.root) => p,
        _ => return (StatusCode::FORBIDDEN, "forbidden path").into_response(),
    };

    let kind = resolved.kind;
    let is_asset_import = request
        .uri()
        .query()
        .is_some_and(|q| q.split('&').any(|part| part == "import"));

    if kind == ModuleKind::Asset {
        if is_asset_import {
            return js_response(
                format!("export default \"{}\";\n", escape_json(&clean_specifier)),
                "no-cache",
            );
        }
        return serve_static_file(&state.root, &canonical, false).await;
    }

    let Ok(source) = tokio::fs::read_to_string(&canonical).await else {
        return (StatusCode::NOT_FOUND, "file not found").into_response();
    };

    // CSS Modules (`*.module.css`) export scoped class maps alongside HMR style injection.
    if kind == ModuleKind::Css
        && canonical
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.ends_with(".module.css"))
    {
        return serve_css_module(&state, canonical, &clean_specifier, source);
    }

    let artifact = match transform_with_cache(&state, &canonical, kind, source) {
        Ok(art) => art,
        Err(err) => {
            return js_response(
                render_error_overlay_module(&clean_specifier, &err.to_string()),
                "no-cache",
            );
        }
    };

    // Resolve static imports to both rewrite URLs for the browser and record real graph edges.
    let mut resolved_edges = Vec::new();
    let final_code = if kind.is_script() {
        match rewrite_dev_script(
            &state,
            &canonical,
            &clean_specifier,
            &artifact.code,
            &mut resolved_edges,
        ) {
            Ok(code) => code,
            Err(err) => {
                return js_response(
                    render_error_overlay_module(&clean_specifier, &err.to_string()),
                    "no-cache",
                );
            }
        }
    } else {
        artifact.code
    };

    let mod_id = ModuleId::from_specifier(&canonical.to_string_lossy());
    if let Ok(mut graph) = state.graph.write() {
        graph.upsert_module(
            mod_id,
            canonical,
            kind,
            artifact.output_hash,
            resolved_edges,
        );
    }

    match kind {
        ModuleKind::Js | ModuleKind::Ts | ModuleKind::Jsx | ModuleKind::Tsx | ModuleKind::Json => {
            js_response(final_code, "no-cache")
        }
        ModuleKind::Css => {
            let wrapped = wrap_css_as_hmr_module(&clean_specifier, &final_code);
            js_response(wrapped, "no-cache")
        }
        ModuleKind::Asset | ModuleKind::Virtual => (StatusCode::OK, final_code).into_response(),
    }
}

fn rewrite_dev_script(
    state: &ServerState,
    canonical: &Path,
    clean_specifier: &str,
    code: &str,
    resolved_edges: &mut Vec<ModuleId>,
) -> ZerhoResult<String> {
    let rewritten = rewrite_imports(code, canonical, |spec| {
        if is_bare_specifier(spec) {
            resolved_edges.push(ModuleId::from_specifier(&format!("dep:{spec}")));
            return Some(format!("/@zerho/deps/{spec}"));
        }
        if let Ok(target) = state.resolver.resolve(spec, Some(canonical)) {
            let target_canon = target.path.canonicalize().unwrap_or(target.path);
            resolved_edges.push(ModuleId::from_specifier(&target_canon.to_string_lossy()));
            let url = path_to_web_url(&state.root, &target_canon)?;
            return if target.kind == ModuleKind::Asset {
                Some(format!("{url}?import"))
            } else {
                Some(url)
            };
        }
        None
    })?;

    let with_env = format!("import.meta.env = {};\n{rewritten}", state.env_json);
    Ok(if state.config.server.hmr {
        inject_hmr_preamble(clean_specifier, &with_env)
    } else {
        with_env
    })
}

fn serve_css_module(
    state: &ServerState,
    canonical: PathBuf,
    clean_specifier: &str,
    source: String,
) -> Response {
    match state.pipeline.transform(&TransformInput {
        path: canonical.clone(),
        kind: ModuleKind::Css,
        source,
        framework: state.config.project.framework,
        is_dev: true,
        minify_css: false,
    }) {
        Ok(out) => {
            let mod_id = ModuleId::from_specifier(&canonical.to_string_lossy());
            if let Ok(mut graph) = state.graph.write() {
                graph.upsert_module(mod_id, canonical, ModuleKind::Css, out.hash, []);
            }
            let wrapped = wrap_css_with_exports_as_hmr_module(
                clean_specifier,
                &out.code,
                out.css_exports.as_ref(),
            );
            js_response(wrapped, "no-cache")
        }
        Err(err) => js_response(
            render_error_overlay_module(clean_specifier, &err.to_string()),
            "no-cache",
        ),
    }
}

fn transform_with_cache(
    state: &ServerState,
    path: &Path,
    kind: ModuleKind,
    source: String,
) -> ZerhoResult<CachedArtifact> {
    let key = DiskCache::compute_key(
        &source,
        module_kind_tag(kind),
        state.config.project.framework.as_str(),
        true,
        false,
    );

    if let Some(cached) = state.cache.get(key) {
        return Ok(cached);
    }

    state.cache.record_miss();
    let out = state.pipeline.transform(&TransformInput {
        path: path.to_path_buf(),
        kind,
        source,
        framework: state.config.project.framework,
        is_dev: true,
        minify_css: false,
    })?;

    let artifact = CachedArtifact {
        code: out.code,
        imports: out.imports,
        output_hash: out.hash,
    };
    let _ = state.cache.put(key, &artifact);
    Ok(artifact)
}

fn module_kind_tag(kind: ModuleKind) -> &'static str {
    match kind {
        ModuleKind::Js => "js",
        ModuleKind::Ts => "ts",
        ModuleKind::Jsx => "jsx",
        ModuleKind::Tsx => "tsx",
        ModuleKind::Css => "css",
        ModuleKind::Json => "json",
        ModuleKind::Asset => "asset",
        ModuleKind::Virtual => "virtual",
    }
}

fn is_bare_specifier(spec: &str) -> bool {
    !spec.starts_with('.')
        && !spec.starts_with('/')
        && !spec.starts_with("http://")
        && !spec.starts_with("https://")
        && !spec.starts_with("virtual:")
        && !spec.starts_with('\0')
}

fn path_to_web_url(root: &Path, target: &Path) -> Option<String> {
    let rel = target.strip_prefix(root).ok()?;
    Some(format!("/{}", rel.to_string_lossy().replace('\\', "/")))
}

fn js_response(code: String, cache_control: &'static str) -> Response {
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/javascript; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static(cache_control),
            ),
        ],
        code,
    )
        .into_response()
}

fn find_proxy_target(
    proxy: &std::collections::BTreeMap<String, String>,
    req_path: &str,
) -> Option<String> {
    proxy
        .iter()
        .filter(|(prefix, _)| req_path.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, target)| target.clone())
}

async fn forward_http_proxy(target_base: &str, req_path: &str, request: Request) -> Response {
    let host_port = target_base
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');

    let Ok(mut stream) = TcpStream::connect(host_port).await else {
        return (
            StatusCode::BAD_GATEWAY,
            format!("zerho proxy error: could not connect to `{target_base}`"),
        )
            .into_response();
    };

    let method = request.method().as_str().to_owned();
    let query = request
        .uri()
        .query()
        .map_or_else(String::new, |q| format!("?{q}"));
    if req_path.contains('\r')
        || req_path.contains('\n')
        || query.contains('\r')
        || query.contains('\n')
    {
        return (StatusCode::BAD_REQUEST, "invalid proxy request URI").into_response();
    }
    let raw_req = format!(
        "{method} {req_path}{query} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n"
    );

    if stream.write_all(raw_req.as_bytes()).await.is_err() {
        return (StatusCode::BAD_GATEWAY, "zerho proxy write failed").into_response();
    }

    let mut raw_resp = Vec::new();
    if stream.read_to_end(&mut raw_resp).await.is_err() {
        return (StatusCode::BAD_GATEWAY, "zerho proxy read failed").into_response();
    }

    let Some(split_pos) = raw_resp.windows(4).position(|w| w == b"\r\n\r\n") else {
        return (
            StatusCode::BAD_GATEWAY,
            "zerho proxy invalid upstream response",
        )
            .into_response();
    };

    let head = String::from_utf8_lossy(&raw_resp[..split_pos]);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or(StatusCode::OK);

    let content_type = head
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case("content-type")
                .then(|| v.trim().to_owned())
        })
        .and_then(|v| HeaderValue::from_str(&v).ok())
        .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"));

    let body = raw_resp[split_pos + 4..].to_vec();
    (status, [(header::CONTENT_TYPE, content_type)], body).into_response()
}

fn inject_hmr_preamble(web_path: &str, code: &str) -> String {
    let escaped = escape_json(web_path);
    if code.contains("$RefreshReg$") || code.contains("$RefreshSig$") {
        format!(
            "import {{ createHotContext as __zerho_createHotContext }} from \"/@zerho/client\";\n\
             import.meta.hot = __zerho_createHotContext(\"{escaped}\");\n\
             const __zerho_prevReg = window.$RefreshReg$;\n\
             const __zerho_prevSig = window.$RefreshSig$;\n\
             window.$RefreshReg$ = (type, id) => {{ if (window.__ZERHO_REFRESH_RUNTIME__) window.__ZERHO_REFRESH_RUNTIME__.register(type, \"{escaped} \" + id); }};\n\
             window.$RefreshSig$ = () => (type) => type;\n\
             {code}\n\
             window.$RefreshReg$ = __zerho_prevReg;\n\
             window.$RefreshSig$ = __zerho_prevSig;\n\
             if (import.meta.hot) {{ import.meta.hot.accept(); }}\n"
        )
    } else {
        format!(
            "import {{ createHotContext as __zerho_createHotContext }} from \"/@zerho/client\";\n\
             import.meta.hot = __zerho_createHotContext(\"{escaped}\");\n\
             {code}"
        )
    }
}

fn render_error_overlay_module(file: &str, message: &str) -> String {
    format!(
        "import {{ showErrorOverlay }} from \"/@zerho/client\";\n\
         showErrorOverlay(\"{}\", \"{}\");\n",
        escape_json(file),
        escape_json(message)
    )
}

fn escape_json(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_hmr_script_before_closing_head() {
        let html = "<!doctype html><html><head><title>App</title></head><body></body></html>";
        let injected = inject_hmr_client(html);
        assert!(injected.contains("/@zerho/client"));
        assert!(injected.find("/@zerho/client") < injected.find("</head>"));
    }

    #[test]
    fn classifies_bare_vs_relative_specifiers() {
        assert!(is_bare_specifier("react"));
        assert!(is_bare_specifier("react/jsx-runtime"));
        assert!(!is_bare_specifier("./counter"));
        assert!(!is_bare_specifier("/src/main.ts"));
        assert!(!is_bare_specifier("https://esm.sh/react"));
    }

    #[test]
    fn injects_hmr_preamble_and_error_overlay_module() {
        let wrapped = inject_hmr_preamble("/src/main.tsx", "export const x = 1;");
        assert!(wrapped.contains("import.meta.hot = __zerho_createHotContext(\"/src/main.tsx\")"));

        let err_mod = render_error_overlay_module("/src/main.tsx", "Unexpected token `;`");
        assert!(err_mod.contains("showErrorOverlay(\"/src/main.tsx\", \"Unexpected token `;`\")"));

        let refresh = inject_hmr_preamble("/src/App.tsx", "$RefreshReg$(App, \"App\");");
        assert!(refresh.contains("window.$RefreshReg$"));
        assert!(refresh.contains("import.meta.hot.accept()"));
    }

    #[tokio::test]
    async fn preview_server_blocks_path_traversal_and_serves_brotli() {
        let dir = std::env::temp_dir().join("zerho-server-security-br-test");
        let dist = dir.join("dist");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dist).expect("create dist");
        std::fs::write(dir.join("secret.txt"), "TOP_SECRET_OUTSIDE_DIST").expect("write secret");
        std::fs::write(dist.join("app.js"), "console.log('plain');").expect("write app.js");
        std::fs::write(dist.join("app.js.br"), b"FAKE_BROTLI_BYTES").expect("write app.js.br");

        let dist_canon = dist.canonicalize().expect("canon dist");

        // 1. Path traversal attempt outside dist_root must be rejected (403 Forbidden).
        let traversal =
            serve_static_file(&dist_canon, &dist_canon.join("../secret.txt"), false).await;
        assert_eq!(traversal.status(), StatusCode::FORBIDDEN);

        // 2. Client accepting Brotli receives pre-compressed .br with Content-Encoding: br.
        let mut br_headers = axum::http::HeaderMap::new();
        br_headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br"),
        );
        assert!(accepts_brotli(&br_headers));
        let br_resp = serve_static_file(&dist_canon, &dist_canon.join("app.js"), true).await;
        assert_eq!(br_resp.status(), StatusCode::OK);
        assert_eq!(
            br_resp
                .headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok()),
            Some("br")
        );
        assert_eq!(
            br_resp
                .headers()
                .get(header::VARY)
                .and_then(|v| v.to_str().ok()),
            Some("Accept-Encoding")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
