use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use oxc::allocator::Allocator;
use oxc::ast::ast::{Argument, CallExpression, Expression};
use oxc::ast_visit::Visit;
use oxc::parser::Parser;
use oxc::span::{SourceType, Span};
use zerho_core::{ZerhoError, ZerhoResult};

struct CjsRequireCollector {
    calls: Vec<(Span, String)>,
}

impl<'a> Visit<'a> for CjsRequireCollector {
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
    let mut collector = CjsRequireCollector { calls: Vec::new() };
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

    let mut collector = CjsRequireCollector { calls: Vec::new() };
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
    let mut queue: Vec<(PathBuf, String)> = vec![(entry_path.to_path_buf(), entry_source.to_owned())];

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

    let mut out = String::from(
        "const __zerho_cjs_registry = {\n",
    );
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
