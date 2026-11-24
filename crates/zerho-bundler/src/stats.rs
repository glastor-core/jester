use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use zerho_core::{ZerhoError, ZerhoResult};

use crate::Chunk;

/// Metadata for an individual source module included in the `dist/stats.html` Bundle Analyzer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleStat {
    pub id: String,
    pub raw_bytes: usize,
    pub kind: String,
}

/// Emits a zero-dependency, interactive Bundle Analyzer & Treemap HTML report (`dist/stats.html`)
/// styled with Red Hat typography and Material Symbols Outlined icons.
#[allow(clippy::too_many_lines)]
pub fn emit_bundle_stats_html(
    out_dir: &Path,
    project_name: &str,
    chunks: &[Chunk],
    modules: &[ModuleStat],
    tree_shaken_exports: usize,
    barrel_imports_bypassed: usize,
    scope_hoisted: bool,
) -> ZerhoResult<PathBuf> {
    let total_raw: usize = chunks.iter().map(|c| c.bytes).sum();
    let total_gzip: usize = chunks.iter().map(|c| c.gzip_bytes).sum();
    let total_br: usize = chunks.iter().map(|c| c.brotli_bytes).sum();
    let total_mod_bytes: usize = modules.iter().map(|m| m.raw_bytes).sum::<usize>().max(1);

    let mut sorted_mods = modules.to_vec();
    sorted_mods.sort_by(|a, b| b.raw_bytes.cmp(&a.raw_bytes).then_with(|| a.id.cmp(&b.id)));

    let mut chunk_rows = String::new();
    for c in chunks {
        let sri_badge = if c.integrity.is_some() {
            "<span class=\"pill ok\">SHA-384</span>"
        } else {
            "<span class=\"pill\">—</span>"
        };
        let _ = writeln!(
            chunk_rows,
            "<tr><td class=\"mono\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num accent\">{}</td><td>{sri_badge}</td></tr>",
            escape_html(&c.file_name),
            fmt_kb(c.bytes),
            fmt_kb(c.gzip_bytes),
            fmt_kb(c.brotli_bytes),
        );
    }

    let mut treemap_items = String::new();
    for m in &sorted_mods {
        let pct_x10 = (m.raw_bytes.saturating_mul(1000) / total_mod_bytes).max(1);
        let pct_str = format!("{}.{}%", pct_x10 / 10, pct_x10 % 10);
        let width_pct = (pct_x10 / 10).clamp(4, 100);
        let _ = writeln!(
            treemap_items,
            "<div class=\"mod-row\" data-name=\"{}\"><div class=\"mod-meta\"><span class=\"mono\">{}</span><span class=\"tag\">{}</span><span class=\"mono muted\">{} ({pct_str})</span></div><div class=\"bar-track\"><div class=\"bar-fill\" style=\"width:{width_pct}%\"></div></div></div>",
            escape_html(&m.id.to_ascii_lowercase()),
            escape_html(&m.id),
            escape_html(&m.kind),
            fmt_kb(m.raw_bytes),
        );
    }

    let mode_label = if scope_hoisted {
        "Scope Hoisted"
    } else {
        "Module Registry"
    };

    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>zerho Bundle Analyzer — {project}</title>
  <link rel="preconnect" href="https://fonts.googleapis.com" />
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
  <link href="https://fonts.googleapis.com/css2?family=Red+Hat+Display:wght@600;700&family=Red+Hat+Mono:wght@400;500&family=Red+Hat+Text:wght@400;500;600&display=swap" rel="stylesheet" />
  <link href="https://fonts.googleapis.com/css2?family=Material+Symbols+Outlined:opsz,wght,FILL,GRAD@20,500,0,0" rel="stylesheet" />
  <style>
    :root {{ --bg:#0b0f17; --card:#121826; --border:#1f293d; --text:#e5e7eb; --muted:#94a3b8; --accent:#f97316; --ok:#22c55e; }}
    * {{ box-sizing:border-box; }}
    body {{ margin:0; padding:32px 24px; background:var(--bg); color:var(--text); font-family:'Red Hat Text',sans-serif; }}
    .wrap {{ max-width:1100px; margin:0 auto; }}
    h1,h2 {{ font-family:'Red Hat Display',sans-serif; margin:0 0 12px; display:flex; align-items:center; gap:8px; }}
    .mono {{ font-family:'Red Hat Mono',monospace; font-size:0.9rem; }}
    .muted {{ color:var(--muted); }}
    .accent {{ color:var(--accent); font-weight:600; }}
    .grid {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(170px,1fr)); gap:12px; margin:20px 0 28px; }}
    .kpi {{ background:var(--card); border:1px solid var(--border); border-radius:10px; padding:16px; }}
    .kpi-label {{ font-size:0.78rem; color:var(--muted); text-transform:uppercase; letter-spacing:.04em; }}
    .kpi-val {{ font-family:'Red Hat Display',sans-serif; font-size:1.5rem; margin-top:6px; }}
    .card {{ background:var(--card); border:1px solid var(--border); border-radius:12px; padding:20px; margin-bottom:24px; }}
    table {{ width:100%; border-collapse:collapse; }}
    th,td {{ text-align:left; padding:10px 12px; border-bottom:1px solid var(--border); }}
    th {{ color:var(--muted); font-size:0.8rem; text-transform:uppercase; }}
    .num {{ text-align:right; font-family:'Red Hat Mono',monospace; }}
    .pill {{ display:inline-block; padding:2px 8px; border-radius:999px; font-size:0.75rem; background:#1e293b; color:var(--muted); }}
    .pill.ok {{ background:rgba(34,197,94,.15); color:var(--ok); }}
    .tag {{ font-size:0.72rem; padding:2px 6px; border-radius:4px; background:#1e293b; color:var(--muted); text-transform:uppercase; }}
    .search {{ width:100%; padding:10px 14px; border-radius:8px; border:1px solid var(--border); background:#0f172a; color:var(--text); margin-bottom:16px; font-family:'Red Hat Mono',monospace; }}
    .mod-row {{ margin-bottom:12px; }}
    .mod-meta {{ display:flex; justify-content:space-between; align-items:center; gap:10px; margin-bottom:4px; }}
    .bar-track {{ height:8px; background:#1e293b; border-radius:999px; overflow:hidden; }}
    .bar-fill {{ height:100%; background:linear-gradient(90deg,#f97316,#fb923c); border-radius:999px; }}
  </style>
</head>
<body>
  <div class="wrap">
    <h1><span class="material-symbols-outlined">analytics</span> zerho Bundle Analyzer — {project}</h1>
    <p class="muted">Zero-Node 100% Rust build report • Mode: <strong class="accent">{mode_label}</strong></p>

    <div class="grid">
      <div class="kpi"><div class="kpi-label">Total Raw</div><div class="kpi-val">{raw}</div></div>
      <div class="kpi"><div class="kpi-label">Total Gzip</div><div class="kpi-val">{gzip}</div></div>
      <div class="kpi"><div class="kpi-label">Total Brotli</div><div class="kpi-val accent">{br}</div></div>
      <div class="kpi"><div class="kpi-label">Modules</div><div class="kpi-val">{mod_count}</div></div>
      <div class="kpi"><div class="kpi-label">Exports Shaken</div><div class="kpi-val">{shaken}</div></div>
      <div class="kpi"><div class="kpi-label">Barrels Bypassed</div><div class="kpi-val">{barrels}</div></div>
    </div>

    <div class="card">
      <h2><span class="material-symbols-outlined">inventory_2</span> Emitted Chunks</h2>
      <table>
        <thead><tr><th>Chunk</th><th class="num">Raw</th><th class="num">Gzip</th><th class="num">Brotli</th><th>SRI</th></tr></thead>
        <tbody>{chunk_rows}</tbody>
      </table>
    </div>

    <div class="card">
      <h2><span class="material-symbols-outlined">account_tree</span> Module Size Treemap</h2>
      <input id="q" class="search" type="search" placeholder="Filter modules by path..." />
      <div id="mods">{treemap_items}</div>
    </div>
  </div>
  <script>
    const q = document.getElementById('q');
    q.addEventListener('input', () => {{
      const term = q.value.trim().toLowerCase();
      document.querySelectorAll('.mod-row').forEach(el => {{
        el.style.display = !term || el.dataset.name.includes(term) ? '' : 'none';
      }});
    }});
  </script>
</body>
</html>
"#,
        project = escape_html(project_name),
        raw = fmt_kb(total_raw),
        gzip = fmt_kb(total_gzip),
        br = fmt_kb(total_br),
        mod_count = modules.len(),
        shaken = tree_shaken_exports,
        barrels = barrel_imports_bypassed,
    );

    let stats_path = out_dir.join("stats.html");
    fs::write(&stats_path, html).map_err(|err| ZerhoError::Io {
        path: Some(stats_path.clone()),
        reason: err.to_string(),
    })?;
    Ok(stats_path)
}

fn fmt_kb(bytes: usize) -> String {
    format!("{}.{:02} KB", bytes / 1024, (bytes % 1024) * 100 / 1024)
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerho_core::ContentHash;

    #[test]
    fn emits_interactive_stats_html_report() {
        let dir = std::env::temp_dir().join("zerho-stats-html-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create dir");

        let chunks = vec![Chunk {
            file_name: "assets/index-12345678.js".to_owned(),
            hash: ContentHash::from_bytes(b"js"),
            bytes: 4096,
            gzip_bytes: 1500,
            brotli_bytes: 1200,
            brotli_file: Some("assets/index-12345678.js.br".to_owned()),
            sourcemap_file: None,
            integrity: Some("sha384-abc".to_owned()),
        }];
        let modules = vec![
            ModuleStat {
                id: "src/main.ts".to_owned(),
                raw_bytes: 2048,
                kind: "ts".to_owned(),
            },
            ModuleStat {
                id: "src/utils.ts".to_owned(),
                raw_bytes: 1024,
                kind: "ts".to_owned(),
            },
        ];

        let path = emit_bundle_stats_html(&dir, "demo-app", &chunks, &modules, 5, 2, true)
            .expect("emit stats html");
        let content = fs::read_to_string(&path).expect("read stats html");
        assert!(content.contains("Red Hat Display"));
        assert!(content.contains("Material+Symbols+Outlined"));
        assert!(content.contains("Scope Hoisted"));
        assert!(content.contains("assets/index-12345678.js"));
        assert!(content.contains("src/main.ts"));

        let _ = fs::remove_dir_all(&dir);
    }
}
