use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Recursively resolves and inlines local CSS `@import "./file.css";` rules while preserving
/// external URLs (`http://`, `https://`) and guarding against circular `@import` cycles.
pub fn inline_css_imports(
    css: &str,
    current_path: &Path,
    mut resolve_and_read: impl FnMut(&str, &Path) -> Option<(PathBuf, String)>,
) -> String {
    let mut visited = HashSet::new();
    visited.insert(current_path.to_path_buf());
    inline_recursive(css, current_path, &mut visited, &mut resolve_and_read)
}

fn inline_recursive(
    css: &str,
    current_path: &Path,
    visited: &mut HashSet<PathBuf>,
    resolve_and_read: &mut impl FnMut(&str, &Path) -> Option<(PathBuf, String)>,
) -> String {
    let mut out = String::with_capacity(css.len());
    for line in css.lines() {
        let trimmed = line.trim();
        if let Some(spec) = parse_css_import_specifier(trimmed) {
            if is_external_css_ref(spec) {
                out.push_str(line);
                out.push('\n');
                continue;
            }
            if let Some((target_path, target_css)) = resolve_and_read(spec, current_path) {
                if visited.insert(target_path.clone()) {
                    let expanded =
                        inline_recursive(&target_css, &target_path, visited, resolve_and_read);
                    out.push_str(&expanded);
                    if !expanded.ends_with('\n') {
                        out.push('\n');
                    }
                }
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn parse_css_import_specifier(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("@import")?.trim();
    let rest = rest.strip_suffix(';').unwrap_or(rest).trim();
    let inner = if let Some(u) = rest.strip_prefix("url(").and_then(|s| s.strip_suffix(')')) {
        u.trim()
    } else {
        rest
    };
    if inner.len() >= 2
        && ((inner.starts_with('"') && inner.ends_with('"'))
            || (inner.starts_with('\'') && inner.ends_with('\'')))
    {
        Some(&inner[1..inner.len() - 1])
    } else {
        None
    }
}

/// Rewrites local `url(...)` references (fonts `.woff2`, images `.svg`/`.png`/`.jpg`/`.webp`)
/// inside CSS using `rewriter`, skipping `data:`, `http://`, `https://`, and `#` fragments.
pub fn rewrite_css_urls(css: &str, mut rewriter: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(css.len());
    let mut cursor = 0usize;

    while let Some(rel_pos) = css[cursor..].find("url(") {
        let start = cursor + rel_pos;
        out.push_str(&css[cursor..start]);

        let after_open = start + 4;
        let Some(close_rel) = css[after_open..].find(')') else {
            out.push_str(&css[start..]);
            return out;
        };
        let end = after_open + close_rel;
        let raw_inner = css[after_open..end].trim();
        let unquoted = if raw_inner.len() >= 2
            && ((raw_inner.starts_with('"') && raw_inner.ends_with('"'))
                || (raw_inner.starts_with('\'') && raw_inner.ends_with('\'')))
        {
            &raw_inner[1..raw_inner.len() - 1]
        } else {
            raw_inner
        };

        if !unquoted.is_empty()
            && !is_external_css_ref(unquoted)
            && let Some(replacement) = rewriter(unquoted)
        {
            out.push_str("url(\"");
            out.push_str(&replacement);
            out.push_str("\")");
        } else {
            out.push_str(&css[start..=end]);
        }

        cursor = end + 1;
    }

    out.push_str(&css[cursor..]);
    out
}

fn is_external_css_ref(url: &str) -> bool {
    url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("data:")
        || url.starts_with("//")
        || url.starts_with('#')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inlines_local_css_imports_and_rewrites_asset_urls() {
        let entry = "@import \"./tokens.css\";\n@import \"https://fonts.googleapis.com/css2\";\nbody { background: url('./bg.png'); font-family: var(--font); }\n@font-face { src: url(\"./inter.woff2\") format(\"woff2\"); }\n";
        let tokens = ":root { --font: 'Red Hat Text'; }\n";

        let inlined = inline_css_imports(entry, Path::new("src/styles.css"), |spec, _| {
            (spec == "./tokens.css").then(|| (PathBuf::from("src/tokens.css"), tokens.to_owned()))
        });
        assert!(inlined.contains(":root { --font: 'Red Hat Text'; }"));
        assert!(inlined.contains("https://fonts.googleapis.com/css2"));
        assert!(!inlined.contains("@import \"./tokens.css\""));

        let rewritten = rewrite_css_urls(&inlined, |url| match url {
            "./bg.png" => Some("/assets/bg-a1b2c3d4.webp".to_owned()),
            "./inter.woff2" => Some("/assets/inter-deadbeef.woff2".to_owned()),
            _ => None,
        });
        assert!(rewritten.contains("url(\"/assets/bg-a1b2c3d4.webp\")"));
        assert!(rewritten.contains("url(\"/assets/inter-deadbeef.woff2\")"));
    }
}
