use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use zerho_config::{SourceMapMode, ZerhoConfig};
use zerho_core::{Framework, ZerhoError, ZerhoResult};

/// Summary of a non-destructive migration from Vite (`vite.config.*` / `package.json`) to `zerho.toml`.
#[derive(Debug, Clone)]
pub struct MigrateSummary {
    pub detected_files: Vec<String>,
    pub generated_toml: String,
    pub written_files: Vec<PathBuf>,
}

/// Inspects an existing Vite/Node frontend project (`package.json`, `vite.config.*`, `index.html`,
/// `tsconfig.json`) and generates a ready-to-run `zerho.toml`, `zerho.schema.json`, and `src/zerho-env.d.ts`.
pub fn migrate_from_vite(root: &Path, dry_run: bool, force: bool) -> ZerhoResult<MigrateSummary> {
    let target_toml = root.join("zerho.toml");
    if target_toml.exists() && !dry_run && !force {
        return Err(ZerhoError::Config(format!(
            "`{}` already exists (pass `--force` to overwrite or `--dry-run` to preview)",
            target_toml.display()
        )));
    }

    let mut config = ZerhoConfig::default();
    let mut detected_files = Vec::new();

    // 1. Inspect package.json (name & framework)
    let pkg_path = root.join("package.json");
    if let Ok(pkg) = fs::read_to_string(&pkg_path) {
        detected_files.push("package.json".to_owned());
        if let Some(name) = extract_json_string_field(&pkg, "name")
            && !name.trim().is_empty()
        {
            config.project.name = name;
        }
        if pkg.contains("\"preact\"") {
            config.project.framework = Framework::Preact;
        } else if pkg.contains("\"react\"") {
            config.project.framework = Framework::React;
        } else if pkg.contains("\"solid-js\"") {
            config.project.framework = Framework::Solid;
        } else {
            config.project.framework = Framework::Vanilla;
        }
    }

    // 2. Inspect index.html for <script type="module" src="..."> entry point
    let html_path = root.join("index.html");
    if let Ok(html) = fs::read_to_string(&html_path) {
        detected_files.push("index.html".to_owned());
        if let Some(entry) = detect_html_entry(&html) {
            config.project.entry = PathBuf::from(entry.trim_start_matches('/'));
        }
    }

    // 3. Inspect vite.config.{ts,mts,js,mjs}
    for candidate in [
        "vite.config.ts",
        "vite.config.mts",
        "vite.config.js",
        "vite.config.mjs",
    ] {
        let path = root.join(candidate);
        if let Ok(vite_cfg) = fs::read_to_string(&path) {
            detected_files.push(candidate.to_owned());
            parse_vite_config_hints(&vite_cfg, &mut config);
            break;
        }
    }

    // 4. Detect tsconfig.json (native paths support in zerho)
    if root.join("tsconfig.json").is_file() {
        detected_files.push("tsconfig.json".to_owned());
    }

    let generated_toml = format!("#:schema ./zerho.schema.json\n\n{}", config.to_toml());
    let mut written_files = Vec::new();

    if !dry_run {
        fs::write(&target_toml, &generated_toml).map_err(|err| ZerhoError::Io {
            path: Some(target_toml.clone()),
            reason: err.to_string(),
        })?;
        written_files.push(target_toml);

        let schema_path = root.join("zerho.schema.json");
        if !schema_path.exists() || force {
            fs::write(&schema_path, ZerhoConfig::json_schema()).map_err(|err| ZerhoError::Io {
                path: Some(schema_path.clone()),
                reason: err.to_string(),
            })?;
            written_files.push(schema_path);
        }
    }

    Ok(MigrateSummary {
        detected_files,
        generated_toml,
        written_files,
    })
}

fn parse_vite_config_hints(src: &str, config: &mut ZerhoConfig) {
    if let Some(port) = extract_js_number_prop(src, "port")
        && let Ok(p) = u16::try_from(port)
        && p > 0
    {
        config.server.port = p;
    }
    if let Some(out_dir) = extract_js_string_prop(src, "outDir")
        && !out_dir.is_empty()
    {
        config.build.out_dir = PathBuf::from(out_dir);
    }
    if let Some(pub_dir) = extract_js_string_prop(src, "publicDir")
        && !pub_dir.is_empty()
    {
        config.project.public_dir = PathBuf::from(pub_dir);
    }
    if src.contains("sourcemap: false") {
        config.build.sourcemap = SourceMapMode::None;
    } else if src.contains("sourcemap: 'inline'") || src.contains("sourcemap: \"inline\"") {
        config.build.sourcemap = SourceMapMode::Inline;
    } else if src.contains("sourcemap: 'hidden'") || src.contains("sourcemap: \"hidden\"") {
        config.build.sourcemap = SourceMapMode::Hidden;
    }
    if let Some(proxies) = extract_vite_proxies(src) {
        for (prefix, target) in proxies {
            config.server.proxy.insert(prefix, target);
        }
    }
}

fn detect_html_entry(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(pos) = rest.find("<script") {
        let after = &rest[pos..];
        let end = after.find('>')?;
        let tag = &after[..end];
        if tag.contains("type=\"module\"") || tag.contains("type='module'") {
            for attr in ["src=\"", "src='"] {
                if let Some(s) = tag.find(attr) {
                    let val_start = &tag[s + attr.len()..];
                    let quote = attr.chars().last()?;
                    if let Some(q_end) = val_start.find(quote) {
                        return Some(val_start[..q_end].to_owned());
                    }
                }
            }
        }
        rest = &after[end..];
    }
    None
}

fn extract_json_string_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let pos = json.find(&needle)?;
    let after_colon = json[pos + needle.len()..].split_once(':')?.1.trim_start();
    let rest = after_colon.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

fn extract_js_string_prop(src: &str, key: &str) -> Option<String> {
    let needle = format!("{key}:");
    let pos = src.find(&needle)?;
    let after = src[pos + needle.len()..].trim_start();
    let quote = after.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let inner = &after[1..];
    let end = inner.find(quote)?;
    Some(inner[..end].to_owned())
}

fn extract_js_number_prop(src: &str, key: &str) -> Option<u64> {
    let needle = format!("{key}:");
    let pos = src.find(&needle)?;
    let after = src[pos + needle.len()..].trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u64>().ok()
}

fn extract_vite_proxies(src: &str) -> Option<BTreeMap<String, String>> {
    let pos = src.find("proxy:")?;
    let after = &src[pos + 6..];
    let open = after.find('{')?;
    let block = &after[open + 1..];
    let mut map = BTreeMap::new();
    for line in block.lines().take(20) {
        let trimmed = line.trim();
        if trimmed.starts_with('}') {
            break;
        }
        if let Some((raw_k, raw_v)) = trimmed.split_once(':') {
            let k = raw_k.trim().trim_matches(|c| c == '\'' || c == '"');
            let v = raw_v
                .trim()
                .trim_end_matches(',')
                .trim_matches(|c| c == '\'' || c == '"');
            if k.starts_with('/') && (v.starts_with("http://") || v.starts_with("https://")) {
                map.insert(k.to_owned(), v.to_owned());
            }
        }
    }
    (!map.is_empty()).then_some(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_vite_react_project_to_valid_zerho_toml() {
        let dir = std::env::temp_dir().join("zerho-vite-migrate-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create dir");

        fs::write(
            dir.join("package.json"),
            r#"{"name": "acme-dashboard", "dependencies": {"react": "^19.0.0"}}"#,
        )
        .expect("write package.json");
        fs::write(
            dir.join("index.html"),
            r#"<!doctype html><html><body><script type="module" src="/src/app.tsx"></script></body></html>"#,
        )
        .expect("write index.html");
        fs::write(
            dir.join("vite.config.ts"),
            r"export default {
  server: {
    port: 3000,
    proxy: {
      '/api': 'http://127.0.0.1:8080',
    },
  },
  build: {
    outDir: 'build',
    sourcemap: 'inline',
  },
};",
        )
        .expect("write vite.config.ts");

        let summary = migrate_from_vite(&dir, false, false).expect("migration should succeed");
        assert!(summary.detected_files.contains(&"vite.config.ts".to_owned()));
        assert!(dir.join("zerho.toml").is_file());
        assert!(dir.join("zerho.schema.json").is_file());

        let parsed = ZerhoConfig::load(&dir.join("zerho.toml")).expect("generated toml must parse");
        assert_eq!(parsed.project.name, "acme-dashboard");
        assert_eq!(parsed.project.framework, Framework::React);
        assert_eq!(parsed.project.entry, PathBuf::from("src/app.tsx"));
        assert_eq!(parsed.server.port, 3000);
        assert_eq!(
            parsed.server.proxy.get("/api").map(String::as_str),
            Some("http://127.0.0.1:8080")
        );
        assert_eq!(parsed.build.out_dir, PathBuf::from("build"));
        assert_eq!(parsed.build.sourcemap, SourceMapMode::Inline);

        let _ = fs::remove_dir_all(&dir);
    }
}
