use crate::security::{compress_brotli, compute_gzip_size, compute_sri_sha384};
use crate::{Chunk, CollectedModule};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use zerho_core::{ContentHash, ModuleKind, ZerhoError, ZerhoResult};
use zerho_transform::optimize_svg;

pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path, optimize_svgs: bool) -> ZerhoResult<()> {
    fs::create_dir_all(dst).map_err(|err| ZerhoError::Io {
        path: Some(dst.to_path_buf()),
        reason: err.to_string(),
    })?;
    let entries = fs::read_dir(src).map_err(|err| ZerhoError::Io {
        path: Some(src.to_path_buf()),
        reason: err.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| ZerhoError::Io {
            path: Some(src.to_path_buf()),
            reason: err.to_string(),
        })?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to, optimize_svgs)?;
        } else if optimize_svgs
            && from
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
            && let Ok(raw) = fs::read_to_string(&from)
        {
            fs::write(&to, optimize_svg(&raw)).map_err(|err| ZerhoError::Io {
                path: Some(to),
                reason: err.to_string(),
            })?;
        } else {
            fs::copy(&from, &to).map_err(|err| ZerhoError::Io {
                path: Some(to),
                reason: err.to_string(),
            })?;
        }
    }
    Ok(())
}

pub(crate) fn emit_css_bundle(
    out_dir: &Path,
    collected: &[CollectedModule],
    emit_sri: bool,
    emit_brotli: bool,
    chunks: &mut Vec<Chunk>,
) -> ZerhoResult<Option<(String, Option<String>)>> {
    let mut css_parts = Vec::new();
    for module in collected {
        if module.kind == ModuleKind::Css && !module.code.trim().is_empty() {
            css_parts.push(module.code.as_str());
        }
    }
    if css_parts.is_empty() {
        return Ok(None);
    }
    let combined_css = css_parts.join("\n");
    let css_hash = ContentHash::from_bytes(combined_css.as_bytes());
    let file_name = format!("assets/index-{}.css", css_hash.short_hex());
    let css_path = out_dir.join(&file_name);
    fs::write(&css_path, &combined_css).map_err(|err| ZerhoError::Io {
        path: Some(css_path),
        reason: err.to_string(),
    })?;
    let integrity = emit_sri.then(|| compute_sri_sha384(combined_css.as_bytes()));
    let gzip_bytes = compute_gzip_size(combined_css.as_bytes());
    let br_buf = compress_brotli(combined_css.as_bytes());
    let brotli_bytes = br_buf.len();
    let brotli_file = if emit_brotli {
        let br_name = format!("{file_name}.br");
        let br_path = out_dir.join(&br_name);
        fs::write(&br_path, &br_buf).map_err(|err| ZerhoError::Io {
            path: Some(br_path),
            reason: err.to_string(),
        })?;
        Some(br_name)
    } else {
        None
    };
    chunks.push(Chunk {
        file_name: file_name.clone(),
        hash: css_hash,
        bytes: combined_css.len(),
        gzip_bytes,
        brotli_bytes,
        brotli_file,
        sourcemap_file: None,
        integrity: integrity.clone(),
    });
    Ok(Some((file_name, integrity)))
}

pub(crate) fn is_css_module_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.ends_with(".module.css"))
}

fn join_base(base: &str, rel_file: &str) -> String {
    let clean = rel_file.trim_start_matches('/');
    format!("{base}{clean}")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn rewrite_html_for_production(
    html: &str,
    entry: &Path,
    base: &str,
    js_file: &str,
    js_sri: Option<&str>,
    css_file: Option<&str>,
    css_sri: Option<&str>,
    preload_chunks: &[(String, Option<String>)],
) -> String {
    let entry_web = format!("/{}", entry.to_string_lossy().replace('\\', "/"));
    let js_url = join_base(base, js_file);
    let script_attr = match js_sri {
        Some(sri) => format!("src=\"{js_url}\" integrity=\"{sri}\" crossorigin=\"anonymous\""),
        None => format!("src=\"{js_url}\""),
    };
    let mut out = html.replace(&format!("src=\"{entry_web}\""), &script_attr);

    let mut head_inject = String::new();
    for (preload_file, preload_sri) in preload_chunks {
        let preload_url = join_base(base, preload_file);
        match preload_sri {
            Some(sri) => {
                let _ = writeln!(
                    head_inject,
                    "  <link rel=\"modulepreload\" href=\"{preload_url}\" integrity=\"{sri}\" crossorigin=\"anonymous\" />"
                );
            }
            None => {
                let _ = writeln!(
                    head_inject,
                    "  <link rel=\"modulepreload\" href=\"{preload_url}\" crossorigin=\"anonymous\" />"
                );
            }
        }
    }

    if let Some(css) = css_file {
        let css_url = join_base(base, css);
        match css_sri {
            Some(sri) => {
                let _ = writeln!(
                    head_inject,
                    "  <link rel=\"stylesheet\" href=\"{css_url}\" integrity=\"{sri}\" crossorigin=\"anonymous\" />"
                );
            }
            None => {
                let _ = writeln!(
                    head_inject,
                    "  <link rel=\"stylesheet\" href=\"{css_url}\" />"
                );
            }
        }
    }

    if !head_inject.is_empty() {
        if let Some(pos) = out.find("</head>") {
            out.insert_str(pos, &head_inject);
        } else {
            out.push_str(&head_inject);
        }
    }
    out
}
