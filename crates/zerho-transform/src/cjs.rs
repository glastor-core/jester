use oxc::allocator::Allocator;
use oxc::ast::ast::{Argument, BinaryOperator, CallExpression, Expression, IfStatement};
use oxc::ast_visit::Visit;
use oxc::parser::Parser;
use oxc::span::{SourceType, Span};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use zerho_core::{ZerhoError, ZerhoResult};

struct CjsRequireCollector {
    calls: Vec<(Span, String)>,
    /// When `Some(true)` (production), skip dead `process.env.NODE_ENV !== 'production'` branches.
    is_production: Option<bool>,
}

fn is_process_env_node_env(expr: &Expression<'_>) -> bool {
    let Expression::StaticMemberExpression(outer) = expr else {
        return false;
    };
    if outer.property.name.as_str() != "NODE_ENV" {
        return false;
    }
    let Expression::StaticMemberExpression(inner) = &outer.object else {
        return false;
    };
    if inner.property.name.as_str() != "env" {
        return false;
    }
    matches!(&inner.object, Expression::Identifier(id) if id.name.as_str() == "process")
}

fn eval_node_env_condition(expr: &Expression<'_>, is_production: bool) -> Option<bool> {
    let Expression::BinaryExpression(bin) = expr else {
        return None;
    };
    let is_eq = match bin.operator {
        BinaryOperator::StrictEquality | BinaryOperator::Equality => true,
        BinaryOperator::StrictInequality | BinaryOperator::Inequality => false,
        _ => return None,
    };
    let target_env = if is_process_env_node_env(&bin.left) {
        if let Expression::StringLiteral(lit) = &bin.right {
            lit.value.as_str()
        } else {
            return None;
        }
    } else if is_process_env_node_env(&bin.right) {
        if let Expression::StringLiteral(lit) = &bin.left {
            lit.value.as_str()
        } else {
            return None;
        }
    } else {
        return None;
    };
    let current = if is_production {
        "production"
    } else {
        "development"
    };
    let eq_result = current == target_env;
    Some(if is_eq { eq_result } else { !eq_result })
}

impl<'a> Visit<'a> for CjsRequireCollector {
    fn visit_if_statement(&mut self, stmt: &IfStatement<'a>) {
        if let Some(is_prod) = self.is_production
            && let Some(branch) = eval_node_env_condition(&stmt.test, is_prod)
        {
            if branch {
                self.visit_statement(&stmt.consequent);
            } else if let Some(alt) = &stmt.alternate {
                self.visit_statement(alt);
            }
            return;
        }
        oxc::ast_visit::walk::walk_if_statement(self, stmt);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if let Expression::Identifier(ident) = &call.callee
            && ident.name == "require"
            && let Some(Argument::StringLiteral(lit)) = call.arguments.first()
        {
            self.calls.push((lit.span, lit.value.as_str().to_owned()));
        }
        oxc::ast_visit::walk::walk_call_expression(self, call);
    }
}

/// Extracts all statically analyzable `require("...")` specifiers from a `CommonJS` module via `oxc`.
pub fn collect_cjs_requires(source: &str, path: &Path) -> ZerhoResult<Vec<String>> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::cjs()).parse();
    if !parsed.diagnostics.is_empty() {
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(ZerhoError::Transform {
            path: path.to_path_buf(),
            reason: messages.join("\n"),
        });
    }
    let mut collector = CjsRequireCollector {
        calls: Vec::new(),
        is_production: None,
    };
    collector.visit_program(&parsed.program);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (_, spec) in collector.calls {
        if seen.insert(spec.clone()) {
            out.push(spec);
        }
    }
    Ok(out)
}

/// Rewrites `require("...")` string literal arguments in a `CommonJS` module using `rewriter`.
pub fn rewrite_cjs_requires(
    source: &str,
    path: &Path,
    rewriter: impl FnMut(&str) -> Option<String>,
) -> ZerhoResult<String> {
    rewrite_cjs_requires_inner(source, path, None, rewriter)
}

/// Rewrites `require("...")` string literal arguments while pruning dead `process.env.NODE_ENV`
/// branches (e.g. skipping `react-dom-client.development.js` during production builds).
pub fn rewrite_cjs_requires_for_env(
    source: &str,
    path: &Path,
    is_production: bool,
    rewriter: impl FnMut(&str) -> Option<String>,
) -> ZerhoResult<String> {
    rewrite_cjs_requires_inner(source, path, Some(is_production), rewriter)
}

fn rewrite_cjs_requires_inner(
    source: &str,
    path: &Path,
    is_production: Option<bool>,
    mut rewriter: impl FnMut(&str) -> Option<String>,
) -> ZerhoResult<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::cjs()).parse();
    if !parsed.diagnostics.is_empty() {
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(ZerhoError::Transform {
            path: path.to_path_buf(),
            reason: messages.join("\n"),
        });
    }

    let mut collector = CjsRequireCollector {
        calls: Vec::new(),
        is_production,
    };
    collector.visit_program(&parsed.program);

    let mut edits: Vec<(Span, String)> = Vec::new();
    for (span, spec) in collector.calls {
        if let Some(replacement) = rewriter(&spec) {
            let escaped = replacement.replace('\\', "\\\\").replace('"', "\\\"");
            edits.push((span, format!("\"{escaped}\"")));
        }
    }
    if edits.is_empty() {
        return Ok(source.to_owned());
    }

    edits.sort_by_key(|b| std::cmp::Reverse(b.0.start));
    let mut output = source.to_owned();
    for (span, replacement) in edits {
        let start = span.start as usize;
        let end = span.end as usize;
        if end <= output.len() && start <= end {
            output.replace_range(start..end, &replacement);
        }
    }
    Ok(output)
}

/// Pre-bundles a multi-file `CommonJS` package graph (e.g. `react-dom/client` -> `scheduler`)
/// into a single self-contained browser ESM module with an embedded `__zerho_cjs_require` registry.
///
/// `resolve_and_read` receives `(specifier, importer_path)` and returns `Some((canonical_path, source))`.
pub fn prebundle_cjs_graph_to_esm(
    entry_path: &Path,
    entry_source: &str,
    mut resolve_and_read: impl FnMut(&str, &Path) -> Option<(PathBuf, String)>,
) -> ZerhoResult<String> {
    let mut modules: BTreeMap<String, String> = BTreeMap::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut queue: Vec<(PathBuf, String)> =
        vec![(entry_path.to_path_buf(), entry_source.to_owned())];

    let entry_key = normalize_cjs_key(entry_path);

    while let Some((current_path, current_src)) = queue.pop() {
        if !visited.insert(current_path.clone()) {
            continue;
        }
        let mut local_map: BTreeMap<String, String> = BTreeMap::new();
        for spec in collect_cjs_requires(&current_src, &current_path)? {
            if let Some((target_path, target_src)) = resolve_and_read(&spec, &current_path) {
                let target_key = normalize_cjs_key(&target_path);
                local_map.insert(spec, target_key);
                if !visited.contains(&target_path) {
                    queue.push((target_path, target_src));
                }
            }
        }
        let rewritten = rewrite_cjs_requires(&current_src, &current_path, |spec| {
            local_map.get(spec).cloned()
        })?;
        modules.insert(normalize_cjs_key(&current_path), rewritten);
    }

    let mut out = String::from("const __zerho_cjs_registry = {\n");
    for (id, body) in &modules {
        let escaped_id = id.replace('\\', "\\\\").replace('"', "\\\"");
        out.push_str("  \"");
        out.push_str(&escaped_id);
        out.push_str("\": function(require, module, exports) {\n");
        out.push_str(body);
        out.push_str("\n  },\n");
    }
    let escaped_entry = entry_key.replace('\\', "\\\\").replace('"', "\\\"");
    out.push_str(
        "};\n\
         const __zerho_cjs_cache = {};\n\
         function __zerho_cjs_require(id) {\n\
           if (__zerho_cjs_cache[id]) return __zerho_cjs_cache[id].exports;\n\
           const factory = __zerho_cjs_registry[id];\n\
           if (!factory) throw new Error(\"[zerho cjs] unresolved require: \" + id);\n\
           const mod = { exports: {} };\n\
           __zerho_cjs_cache[id] = mod;\n\
           factory(__zerho_cjs_require, mod, mod.exports);\n\
           return mod.exports;\n\
         }\n\
         const __zerho_cjs_entry = __zerho_cjs_require(\"",
    );
    out.push_str(&escaped_entry);
    out.push_str("\");\nexport default __zerho_cjs_entry;\n");
    Ok(out)
}

fn normalize_cjs_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Scans CJS source for static `exports.<ident> =` assignments so browser ESM wrappers
/// can re-export named bindings (`export const createRoot = __zerho_cjs_entry.createRoot;`).
fn collect_static_cjs_export_names(source: &str, out: &mut std::collections::BTreeSet<String>) {
    let bytes = source.as_bytes();
    let needle = b"exports.";
    let mut i = 0;
    while i + needle.len() < bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let prev_ok = i == 0
                || !(bytes[i - 1].is_ascii_alphanumeric()
                    || bytes[i - 1] == b'_'
                    || bytes[i - 1] == b'$'
                    || bytes[i - 1] == b'.');
            if prev_ok {
                let start = i + needle.len();
                let mut end = start;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric()
                        || bytes[end] == b'_'
                        || bytes[end] == b'$')
                {
                    end += 1;
                }
                if end > start {
                    let mut after = end;
                    while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                        after += 1;
                    }
                    if after < bytes.len()
                        && bytes[after] == b'='
                        && bytes.get(after + 1) != Some(&b'=')
                    {
                        let name = &source[start..end];
                        if is_valid_esm_export_ident(name) {
                            out.insert(name.to_owned());
                        }
                    }
                }
            }
        }
        i += 1;
    }
}

fn is_valid_esm_export_ident(name: &str) -> bool {
    if name.is_empty()
        || matches!(
            name,
            "default"
                | "__esModule"
                | "break"
                | "case"
                | "catch"
                | "class"
                | "const"
                | "continue"
                | "debugger"
                | "delete"
                | "do"
                | "else"
                | "enum"
                | "export"
                | "extends"
                | "false"
                | "finally"
                | "for"
                | "function"
                | "if"
                | "import"
                | "in"
                | "instanceof"
                | "new"
                | "null"
                | "return"
                | "super"
                | "switch"
                | "this"
                | "throw"
                | "true"
                | "try"
                | "typeof"
                | "var"
                | "void"
                | "while"
                | "with"
        )
    {
        return false;
    }
    let first = name.as_bytes()[0];
    first.is_ascii_alphabetic() || first == b'_' || first == b'$'
}

fn is_bare_cjs_specifier(spec: &str) -> bool {
    !spec.starts_with('.') && !spec.starts_with('/') && !spec.starts_with("http")
}

/// Dev-server `CommonJS` -> browser ESM pre-bundler:
/// - Inlines relative `require("./...")` files within the same package (pruning dead `NODE_ENV` branches).
/// - Keeps bare external `require("react")` / `require("scheduler")` as top-level ESM imports from
///   `/@zerho/deps/<spec>` so the browser shares a single singleton instance across packages.
/// - Emits real ESM named exports (`export const createRoot = __zerho_cjs_entry.createRoot;`) alongside `export default`.
pub fn prebundle_cjs_for_dev(
    entry_path: &Path,
    entry_source: &str,
    mut resolve_and_read: impl FnMut(&str, &Path) -> Option<(PathBuf, String)>,
) -> ZerhoResult<String> {
    let mut modules: BTreeMap<String, String> = BTreeMap::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut queue: Vec<(PathBuf, String)> =
        vec![(entry_path.to_path_buf(), entry_source.to_owned())];
    let mut external_bare: BTreeMap<String, String> = BTreeMap::new();
    let mut export_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let entry_key = normalize_cjs_key(entry_path);

    while let Some((current_path, current_src)) = queue.pop() {
        if !visited.insert(current_path.clone()) {
            continue;
        }
        collect_static_cjs_export_names(&current_src, &mut export_names);

        let mut local_map: BTreeMap<String, String> = BTreeMap::new();
        let rewritten = rewrite_cjs_requires_for_env(&current_src, &current_path, false, |spec| {
            if is_bare_cjs_specifier(spec) {
                let next_idx = external_bare.len();
                let alias = external_bare
                    .entry(spec.to_owned())
                    .or_insert_with(|| format!("__zerho_ext_{next_idx}"))
                    .clone();
                let _ = alias;
                return Some(format!("__ext__:{spec}"));
            }
            if let Some(existing) = local_map.get(spec) {
                return Some(existing.clone());
            }
            if let Some((target_path, target_src)) = resolve_and_read(spec, &current_path) {
                let target_key = normalize_cjs_key(&target_path);
                local_map.insert(spec.to_owned(), target_key.clone());
                if !visited.contains(&target_path) {
                    queue.push((target_path, target_src));
                }
                Some(target_key)
            } else {
                None
            }
        })?;
        modules.insert(normalize_cjs_key(&current_path), rewritten);
    }

    let mut out = String::new();
    for (spec, alias) in &external_bare {
        out.push_str("import ");
        out.push_str(alias);
        out.push_str(" from \"/@zerho/deps/");
        out.push_str(spec);
        out.push_str("\";\n");
    }
    out.push_str("const __zerho_ext_map = {\n");
    for (spec, alias) in &external_bare {
        out.push_str("  \"__ext__:");
        out.push_str(spec);
        out.push_str("\": ");
        out.push_str(alias);
        out.push_str(",\n");
    }
    out.push_str("};\nconst __zerho_cjs_registry = {\n");
    for (id, body) in &modules {
        let escaped_id = id.replace('\\', "\\\\").replace('"', "\\\"");
        out.push_str("  \"");
        out.push_str(&escaped_id);
        out.push_str("\": function(require, module, exports) {\n");
        out.push_str(body);
        out.push_str("\n  },\n");
    }
    let escaped_entry = entry_key.replace('\\', "\\\\").replace('"', "\\\"");
    out.push_str(
        "};\n\
         const __zerho_cjs_cache = {};\n\
         function __zerho_cjs_require(id) {\n\
           if (id in __zerho_ext_map) return __zerho_ext_map[id];\n\
           if (__zerho_cjs_cache[id]) return __zerho_cjs_cache[id].exports;\n\
           const factory = __zerho_cjs_registry[id];\n\
           if (!factory) throw new Error(\"[zerho cjs] unresolved require: \" + id);\n\
           const mod = { exports: {} };\n\
           __zerho_cjs_cache[id] = mod;\n\
           factory(__zerho_cjs_require, mod, mod.exports);\n\
           return mod.exports;\n\
         }\n\
         const __zerho_cjs_entry = __zerho_cjs_require(\"",
    );
    out.push_str(&escaped_entry);
    out.push_str("\");\nexport default __zerho_cjs_entry;\n");
    for name in export_names {
        out.push_str("export const ");
        out.push_str(&name);
        out.push_str(" = __zerho_cjs_entry?.");
        out.push_str(&name);
        out.push_str(";\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prebundles_nested_commonjs_require_graph_into_self_contained_esm() {
        let entry_path = PathBuf::from("node_modules/pkg/index.js");
        let entry_src = "const helper = require('./helper');\nmodule.exports = { run: () => helper.val + 1 };\n";
        let helper_path = PathBuf::from("node_modules/pkg/helper.js");
        let helper_src = "const leaf = require('leaf-dep');\nexports.val = leaf * 2;\n";
        let leaf_path = PathBuf::from("node_modules/leaf-dep/index.js");
        let leaf_src = "module.exports = 21;\n";

        let bundled = prebundle_cjs_graph_to_esm(&entry_path, entry_src, |spec, _| match spec {
            "./helper" => Some((helper_path.clone(), helper_src.to_owned())),
            "leaf-dep" => Some((leaf_path.clone(), leaf_src.to_owned())),
            _ => None,
        })
        .expect("deep CJS prebundle should succeed");

        assert!(bundled.contains("node_modules/pkg/index.js"));
        assert!(bundled.contains("node_modules/pkg/helper.js"));
        assert!(bundled.contains("node_modules/leaf-dep/index.js"));
        assert!(bundled.contains("require(\"node_modules/pkg/helper.js\")"));
        assert!(bundled.contains("require(\"node_modules/leaf-dep/index.js\")"));
        assert!(bundled.contains("export default __zerho_cjs_entry;"));
    }
}
