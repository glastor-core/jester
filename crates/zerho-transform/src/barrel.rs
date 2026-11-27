use oxc::allocator::Allocator;
use oxc::ast::ast::{ImportDeclarationSpecifier, Statement};
use oxc::parser::Parser;
use oxc::span::{SourceType, Span};
use std::collections::BTreeMap;
use std::path::Path;
use zerho_core::{ZerhoError, ZerhoResult};

/// A single re-export entry inside a pure barrel file (`export { orig as exported } from './leaf'`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarrelExport {
    /// Specifier as written in the barrel file (e.g. `./Button`).
    pub source_specifier: String,
    /// Original exported name in the leaf module (e.g. `default` or `Button`).
    pub original_name: String,
}

/// Checks whether a module is a **pure barrel file** (contains only side-effect-free
/// `export { ... } from '...'` statements) and returns a map of `exported_name -> BarrelExport`.
///
/// Returns `None` if the module contains any runtime statement, local declaration, or `export *`.
pub fn parse_pure_barrel(source: &str, path: &Path) -> Option<BTreeMap<String, BarrelExport>> {
    parse_barrel_exports(source, path, false)
}

/// Extracts `export { orig as exported } from './leaf'` re-exports from a barrel file.
///
/// When `side_effect_free` is `true` (e.g. owning `package.json` declares `"sideEffects": false`),
/// auxiliary statements such as namespace re-exports (`import * as index; export { index as icons }`)
/// are tolerated so callers importing direct re-exports (e.g. `import { Zap } from 'lucide-react'`)
/// bypass thousands of unused sibling modules.
pub fn parse_barrel_exports(
    source: &str,
    path: &Path,
    side_effect_free: bool,
) -> Option<BTreeMap<String, BarrelExport>> {
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::mjs());
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() || parsed.program.body.is_empty() {
        return None;
    }

    let mut map = BTreeMap::new();
    for stmt in &parsed.program.body {
        match stmt {
            Statement::ExportFromDeclaration(decl) => {
                if decl.export_kind.is_type() {
                    continue;
                }
                let src = decl.source.value.as_str().to_owned();
                for spec in &decl.specifiers {
                    if spec.export_kind.is_type() {
                        continue;
                    }
                    let exported = spec.exported.name().as_str().to_owned();
                    let original = spec.local.name().as_str().to_owned();
                    map.insert(
                        exported,
                        BarrelExport {
                            source_specifier: src.clone(),
                            original_name: original,
                        },
                    );
                }
            }
            // Ignore empty statements or pure TS type-only declarations.
            Statement::EmptyStatement(_)
            | Statement::TSTypeAliasDeclaration(_)
            | Statement::TSInterfaceDeclaration(_) => {}
            _ if side_effect_free => {}
            _ => return None,
        }
    }

    (!map.is_empty()).then_some(map)
}

/// Rewrites named imports targeting pure barrel files directly to their leaf module specifiers,
/// eliminating barrel file loading and unused transitive module parsing.
///
/// `resolve_barrel_export` receives `(barrel_specifier, imported_name)` and returns
/// `Some((resolved_leaf_specifier, original_export_name))` if the target is a pure barrel.
pub fn rewrite_barrel_imports(
    js_code: &str,
    path: &Path,
    mut resolve_barrel_export: impl FnMut(&str, &str) -> Option<(String, String)>,
) -> ZerhoResult<(String, usize)> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, js_code, SourceType::mjs()).parse();
    if !parsed.diagnostics.is_empty() {
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(ZerhoError::Transform {
            path: path.to_path_buf(),
            reason: messages.join("\n"),
        });
    }

    let mut edits: Vec<(Span, String)> = Vec::new();
    let mut bypassed = 0usize;

    for stmt in &parsed.program.body {
        let Statement::ImportDeclaration(decl) = stmt else {
            continue;
        };
        let Some(specifiers) = &decl.specifiers else {
            continue;
        };
        if specifiers.is_empty() {
            continue;
        }
        // Only optimize pure named imports (not default or namespace imports from the barrel).
        if !specifiers
            .iter()
            .all(|s| matches!(s, ImportDeclarationSpecifier::ImportSpecifier(_)))
        {
            continue;
        }

        let barrel_spec = decl.source.value.as_str();
        let mut grouped: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        let mut all_resolved = true;

        for s in specifiers {
            if let ImportDeclarationSpecifier::ImportSpecifier(named) = s {
                let imported = named.imported.name().as_str();
                let local = named.local.name.as_str();
                if let Some((leaf_spec, orig_name)) = resolve_barrel_export(barrel_spec, imported) {
                    grouped
                        .entry(leaf_spec)
                        .or_default()
                        .push((orig_name, local.to_owned()));
                } else {
                    all_resolved = false;
                    break;
                }
            }
        }

        if all_resolved && !grouped.is_empty() {
            let mut replacement_lines = Vec::new();
            for (leaf_spec, items) in grouped {
                let escaped_leaf = leaf_spec.replace('\\', "\\\\").replace('"', "\\\"");
                let mut named_parts = Vec::new();
                for (orig, local) in items {
                    if orig == "default" {
                        replacement_lines.push(format!("import {local} from \"{escaped_leaf}\";"));
                    } else if orig == local {
                        named_parts.push(local);
                    } else {
                        named_parts.push(format!("{orig} as {local}"));
                    }
                }
                if !named_parts.is_empty() {
                    replacement_lines.push(format!(
                        "import {{ {} }} from \"{escaped_leaf}\";",
                        named_parts.join(", ")
                    ));
                }
            }
            edits.push((decl.span, replacement_lines.join("\n")));
            bypassed += 1;
        }
    }

    if edits.is_empty() {
        return Ok((js_code.to_owned(), 0));
    }

    edits.sort_by_key(|b| std::cmp::Reverse(b.0.start));
    let mut output = js_code.to_owned();
    for (span, replacement) in edits {
        let start = span.start as usize;
        let end = span.end as usize;
        if end <= output.len() && start <= end {
            output.replace_range(start..end, &replacement);
        }
    }
    Ok((output, bypassed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pure_barrel_and_rewrites_named_imports_to_leaves() {
        let barrel_src = "\
            export { Button } from './Button';\n\
            export { default as Modal } from './Modal';\n\
            export { UnusedWidget } from './UnusedWidget';\n";
        let map = parse_pure_barrel(barrel_src, Path::new("src/components/index.ts"))
            .expect("should recognize pure barrel");
        assert_eq!(map.len(), 3);
        assert_eq!(map["Modal"].original_name, "default");

        let entry_src = "import { Button, Modal as AppModal } from './components';\nconsole.log(Button, AppModal);\n";
        let (rewritten, count) =
            rewrite_barrel_imports(entry_src, Path::new("src/main.ts"), |spec, sym| {
                if spec == "./components" {
                    let entry = map.get(sym)?;
                    Some((
                        format!(
                            "./components/{}",
                            entry.source_specifier.trim_start_matches("./")
                        ),
                        entry.original_name.clone(),
                    ))
                } else {
                    None
                }
            })
            .expect("barrel rewrite should succeed");

        assert_eq!(count, 1);
        assert!(rewritten.contains("import { Button } from \"./components/Button\";"));
        assert!(rewritten.contains("import AppModal from \"./components/Modal\";"));
        assert!(!rewritten.contains("UnusedWidget"));
    }

    #[test]
    fn rejects_impure_barrel_with_side_effects() {
        let impure = "console.log('side effect');\nexport { Button } from './Button';\n";
        assert!(parse_pure_barrel(impure, Path::new("index.ts")).is_none());
    }
}
