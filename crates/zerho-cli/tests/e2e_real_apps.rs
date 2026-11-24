use std::fs;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use zerho_bundler::Bundler;
use zerho_config::ZerhoConfig;
use zerho_core::Framework;
use zerho_server::{DevServer, PreviewServer};

fn make_temp_app(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zerho-e2e-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).expect("create src dir");
    dir
}

async fn spawn_router(router: axum::Router) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    addr
}

async fn http_get(addr: std::net::SocketAddr, path: &str, extra_headers: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect server");
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{extra_headers}\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .expect("write http request");
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .await
        .expect("read http response");
    String::from_utf8_lossy(&buf).into_owned()
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, content).expect("write fixture file");
}

fn is_js_chunk(file_name: &str) -> bool {
    Path::new(file_name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("js"))
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn e2e_react19_dev_hmr_and_preview_brotli() {
    let root = make_temp_app("react19");
    write_file(
        &root.join("index.html"),
        "<!doctype html><html><head><title>React 19 E2E</title></head>\
         <body><div id=\"root\"></div><script type=\"module\" src=\"/src/main.tsx\"></script></body></html>",
    );
    write_file(
        &root.join("node_modules/react/package.json"),
        "{\"name\":\"react\",\"version\":\"19.0.0\",\"main\":\"index.js\",\"exports\":{\".\":\"./index.js\",\"./jsx-runtime\":\"./jsx-runtime.js\",\"./jsx-dev-runtime\":\"./jsx-runtime.js\"}}",
    );
    write_file(
        &root.join("node_modules/react/index.js"),
        "export function render(el) { return el; }\n",
    );
    write_file(
        &root.join("node_modules/react/jsx-runtime.js"),
        "export function jsx(type, props) { return { type, props }; }\n\
         export function jsxs(type, props) { return { type, props }; }\n\
         export function jsxDEV(type, props) { return { type, props }; }\n",
    );
    write_file(
        &root.join("src/styles.module.css"),
        ".badge { color: #f97316; font-weight: 700; }\n",
    );
    write_file(
        &root.join("src/lazy.tsx"),
        "export default function LazyPanel() { return <section>Lazy React 19</section>; }\n",
    );
    write_file(
        &root.join("src/main.tsx"),
        "import styles from './styles.module.css';\n\
         import { render } from 'react';\n\
         export function App() {\n\
           return <main className={styles.badge}>React 19 E2E</main>;\n\
         }\n\
         export async function loadLazy() { return import('./lazy'); }\n\
         render(<App />);\n",
    );

    let mut cfg = ZerhoConfig::default();
    cfg.project.root.clone_from(&root);
    cfg.project.entry = PathBuf::from("src/main.tsx");
    cfg.project.framework = Framework::React;
    cfg.build.brotli = true;
    cfg.security.sri = true;
    cfg.security.csp = true;

    // 1. Live DevServer checks (HTML, TSX + Fast Refresh, Partial Bundle, /__inspect Treemap, WS HMR)
    let dev = DevServer::new(cfg.clone());
    let dev_addr = spawn_router(dev.router()).await;

    let html = http_get(dev_addr, "/", "").await;
    assert!(html.starts_with("HTTP/1.1 200"), "GET / failed: {html}");
    assert!(html.contains("/@zerho/client"), "missing HMR client script");

    let tsx = http_get(dev_addr, "/src/main.tsx", "").await;
    assert!(
        tsx.starts_with("HTTP/1.1 200"),
        "GET /src/main.tsx failed: {tsx}"
    );
    assert!(
        tsx.contains("$RefreshReg$") || tsx.contains("jsx"),
        "expected compiled JSX / React Fast Refresh in /src/main.tsx: {tsx}"
    );

    let bundle = http_get(dev_addr, "/@zerho/bundle", "").await;
    assert!(
        bundle.starts_with("HTTP/1.1 200"),
        "GET /@zerho/bundle failed: {bundle}"
    );

    let inspect = http_get(dev_addr, "/__inspect", "").await;
    assert!(
        inspect.starts_with("HTTP/1.1 200") && inspect.contains("Treemap"),
        "GET /__inspect missing Live Treemap UI"
    );

    let inspect_api = http_get(dev_addr, "/__inspect/api", "").await;
    assert!(
        inspect_api.starts_with("HTTP/1.1 200") && inspect_api.contains("\"bytes\""),
        "GET /__inspect/api missing byte telemetry: {inspect_api}"
    );

    // WebSocket upgrade check on /@zerho/hmr (RFC 6455 sample handshake key)
    let mut ws_stream = TcpStream::connect(dev_addr)
        .await
        .expect("connect ws endpoint");
    let rfc_ws_key = ["dGhlIHNhbXBsZ", "SBub25jZQ=="].concat();
    let ws_req = format!(
        "GET /@zerho/hmr HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {rfc_ws_key}\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    ws_stream
        .write_all(ws_req.as_bytes())
        .await
        .expect("write ws upgrade");
    let mut ws_buf = [0u8; 256];
    let n = ws_stream.read(&mut ws_buf).await.expect("read ws upgrade");
    let ws_resp = String::from_utf8_lossy(&ws_buf[..n]);
    assert!(
        ws_resp.starts_with("HTTP/1.1 101"),
        "expected 101 Switching Protocols on /@zerho/hmr, got: {ws_resp}"
    );

    // 2. Real Production Build + PreviewServer (SRI + Brotli + Path Traversal guard)
    let mut bundler = Bundler::new(cfg.clone());
    let output = bundler.build().expect("React 19 production build");
    let entry_js = output
        .chunks
        .iter()
        .find(|c| is_js_chunk(&c.file_name))
        .expect("emitted entry js chunk")
        .file_name
        .clone();
    assert!(root.join("dist/csp.json").exists());

    let preview = PreviewServer::new(cfg);
    let preview_addr = spawn_router(preview.router()).await;

    let prod_html = http_get(preview_addr, "/", "").await;
    assert!(prod_html.starts_with("HTTP/1.1 200"));
    assert!(
        prod_html.contains("integrity=\"sha384-"),
        "production HTML missing SHA-384 SRI"
    );

    let br_js = http_get(
        preview_addr,
        &format!("/{entry_js}"),
        "Accept-Encoding: br\r\n",
    )
    .await;
    assert!(
        br_js.starts_with("HTTP/1.1 200")
            && br_js.to_ascii_lowercase().contains("content-encoding: br"),
        "expected Brotli-encoded JS from PreviewServer: {br_js}"
    );

    let traversal = http_get(preview_addr, "/../index.html", "").await;
    assert!(
        traversal.starts_with("HTTP/1.1 400")
            || traversal.starts_with("HTTP/1.1 403")
            || traversal.starts_with("HTTP/1.1 404"),
        "path traversal was not blocked: {traversal}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn e2e_preact_dev_and_production_build() {
    let root = make_temp_app("preact");
    write_file(
        &root.join("index.html"),
        "<!doctype html><html><body><div id=\"app\"></div><script type=\"module\" src=\"/src/main.tsx\"></script></body></html>",
    );
    write_file(
        &root.join("node_modules/preact/package.json"),
        "{\"name\":\"preact\",\"version\":\"10.25.0\",\"main\":\"index.js\",\"exports\":{\".\":\"./index.js\",\"./jsx-runtime\":\"./jsx-runtime.js\",\"./jsx-dev-runtime\":\"./jsx-runtime.js\"}}",
    );
    write_file(
        &root.join("node_modules/preact/index.js"),
        "export function h(t, p, ...c) { return { t, p, c }; }\nexport function Fragment(p) { return p; }\n",
    );
    write_file(
        &root.join("node_modules/preact/jsx-runtime.js"),
        "export function jsx(t, p) { return { t, p }; }\nexport function jsxs(t, p) { return { t, p }; }\nexport function jsxDEV(t, p) { return { t, p }; }\n",
    );
    write_file(
        &root.join("src/main.tsx"),
        "export function Counter({ count }: { count: number }) {\n\
           return <button type=\"button\">Preact: {count}</button>;\n\
         }\n\
         console.log(<Counter count={42} />);\n",
    );

    let mut cfg = ZerhoConfig::default();
    cfg.project.root.clone_from(&root);
    cfg.project.entry = PathBuf::from("src/main.tsx");
    cfg.project.framework = Framework::Preact;

    let dev = DevServer::new(cfg.clone());
    let dev_addr = spawn_router(dev.router()).await;
    let js = http_get(dev_addr, "/src/main.tsx", "").await;
    assert!(js.starts_with("HTTP/1.1 200"), "Preact dev failed: {js}");
    assert!(
        !js.contains(": { count: number }"),
        "TypeScript annotations were not stripped in Preact TSX"
    );

    let mut bundler = Bundler::new(cfg.clone());
    let out = bundler.build().expect("Preact production build");
    assert!(
        out.chunks.iter().any(|c| is_js_chunk(&c.file_name)),
        "expected emitted Preact JS chunk"
    );

    let preview = PreviewServer::new(cfg);
    let preview_addr = spawn_router(preview.router()).await;
    let html = http_get(preview_addr, "/", "").await;
    assert!(html.starts_with("HTTP/1.1 200"));

    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn e2e_strict_typescript_dev_ssr_and_preview() {
    let root = make_temp_app("strict-ts");
    write_file(
        &root.join("index.html"),
        "<!doctype html><html><body><div id=\"app\"></div><script type=\"module\" src=\"/src/main.ts\"></script></body></html>",
    );
    write_file(
        &root.join("src/types.ts"),
        "export interface Metric<T extends number> { readonly label: string; readonly value: T; }\n",
    );
    write_file(
        &root.join("src/main.ts"),
        "import type { Metric } from './types';\n\
         const metric = { label: 'build', value: 19 } satisfies Metric<number>;\n\
         export function formatMetric(m: Metric<number>): string { return `${m.label}:${m.value}`; }\n\
         console.log(formatMetric(metric));\n",
    );

    let mut cfg = ZerhoConfig::default();
    cfg.project.root.clone_from(&root);
    cfg.project.entry = PathBuf::from("src/main.ts");
    cfg.project.framework = Framework::Vanilla;

    // 1. Live DevServer strips strict TypeScript syntax
    let dev = DevServer::new(cfg.clone());
    let dev_addr = spawn_router(dev.router()).await;
    let resp = http_get(dev_addr, "/src/main.ts", "").await;
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "strict TS dev failed: {resp}"
    );
    assert!(
        !resp.contains("satisfies Metric"),
        "satisfies clause leaked"
    );
    assert!(!resp.contains("import type"), "import type leaked");

    // 2. Production Build + PreviewServer
    let mut bundler = Bundler::new(cfg.clone());
    let prod = bundler.build().expect("strict TS prod build");
    let prod_entry = prod
        .chunks
        .iter()
        .find(|c| is_js_chunk(&c.file_name))
        .expect("strict TS entry js")
        .file_name
        .clone();
    let preview = PreviewServer::new(cfg.clone());
    let preview_addr = spawn_router(preview.router()).await;
    let js = http_get(preview_addr, &format!("/{prod_entry}"), "").await;
    assert!(js.starts_with("HTTP/1.1 200"));

    // 3. Edge SSR Build (dist/server.mjs)
    let mut ssr_cfg = cfg;
    ssr_cfg.build.ssr = true;
    let mut ssr_bundler = Bundler::new(ssr_cfg);
    let ssr_out = ssr_bundler.build().expect("strict TS SSR build");
    assert!(
        ssr_out.chunks.iter().any(|c| c.file_name == "server.mjs"),
        "expected server.mjs SSR chunk"
    );
    let server_js = fs::read_to_string(root.join("dist/server.mjs")).expect("read server.mjs");
    assert!(server_js.contains("formatMetric"));

    let _ = fs::remove_dir_all(&root);
}
