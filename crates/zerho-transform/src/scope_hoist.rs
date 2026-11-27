use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Declaration, ExportDefaultDeclarationKind, IdentifierReference, ImportDeclarationSpecifier,
    ObjectProperty, Statement,
};
use oxc::ast_visit::Visit;
use oxc::parser::Parser;
use oxc::semantic::{ReferenceId, SemanticBuilder, SymbolId};
use oxc::span::{GetSpan, SourceType, Span};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Input module passed to [`try_scope_hoist_bundle`] in deterministic post-order DFS
/// (every dependency must appear before its importer).
#[derive(Debug, Clone)]
pub struct HoistedModuleInput {
    pub id: String,
    /// Transformed ESM code (after tree-shaking). For synthetic modules (CSS module exports
    /// or asset URLs), set `synthetic_default_expr` instead.
    pub code: String,
    /// Optional JS expression for synthetic default-export modules (e.g. JSON object or URL string).
    pub synthetic_default_expr: Option<String>,
    /// Map from raw import specifier (as written in source) to target `HoistedModuleInput.id`.
    /// Specifiers mapped to `None` (e.g. plain CSS side-effect imports) are stripped.
    pub resolved_imports: BTreeMap<String, Option<String>>,
}

struct ScopeHoistCollector {
    shorthand_spans: HashSet<(u32, u32)>,
    ref_spans: HashMap<ReferenceId, Span>,
}

impl<'a> Visit<'a> for ScopeHoistCollector {
    fn visit_object_property(&mut self, prop: &ObjectProperty<'a>) {
        if prop.shorthand {
            let s = prop.value.span();
            self.shorthand_spans.insert((s.start, s.end));
        }
        oxc::ast_visit::walk::walk_object_property(self, prop);
    }

    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        if let Some(ref_id) = ident.reference_id.get() {
            self.ref_spans.insert(ref_id, ident.span);
        }
    }
}

/// Attempts to flatten an acyclic ESM module graph into a single scope-hoisted IIFE
/// without `__zerho_registry` or `__zerho_require` wrappers.
///
/// Returns `None` if any module uses `CommonJS`, circular dependencies, namespace imports,
/// `export *`, or `eval`/`with` — allowing the caller to fall back cleanly to registry mode.
pub fn try_scope_hoist_bundle(modules: &[HoistedModuleInput]) -> Option<String> {
    if modules.is_empty() || modules.iter().any(|m| m.code.contains("__zerho_require")) {
        return None;
    }

    let mut export_tables: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut hoisted_bodies: Vec<String> = Vec::with_capacity(modules.len());

    for (mod_idx, module) in modules.iter().enumerate() {
        if let Some(expr) = &module.synthetic_default_expr {
            let ident = format!("__zerho_default_m{mod_idx}");
            hoisted_bodies.push(format!("const {ident} = {expr};"));
            let mut exports = HashMap::new();
            exports.insert("default".to_owned(), ident);
            export_tables.insert(module.id.clone(), exports);
            continue;
        }

        // Bail out to registry mode on CJS wrappers or dynamic scope constructs.
        if module.code.contains("__zerho_cjs_module")
            || module.code.contains("eval(")
            || module.code.contains("with(")
            || module.code.contains("with (")
        {
            return None;
        }

        let (body, exports) = hoist_single_module(module, mod_idx, &export_tables)?;
        hoisted_bodies.push(body);
        export_tables.insert(module.id.clone(), exports);
    }

    let mut out = String::from("(function(){\n\"use strict\";\n");
    for body in hoisted_bodies {
        let trimmed = body.trim();
        if !trimmed.is_empty() {
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.push_str("})();\n");
    Some(out)
}

#[allow(clippy::too_many_lines)]
fn hoist_single_module(
    module: &HoistedModuleInput,
    mod_idx: usize,
    export_tables: &HashMap<String, HashMap<String, String>>,
) -> Option<(String, HashMap<String, String>)> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &module.code, SourceType::mjs()).parse();
    if !parsed.diagnostics.is_empty() {
        return None;
    }

    let semantic_ret = SemanticBuilder::new().build(&parsed.program);
    let scoping = semantic_ret.semantic.scoping();
    let root_scope = scoping.root_scope_id();

    let mut collector = ScopeHoistCollector {
        shorthand_spans: HashSet::new(),
        ref_spans: HashMap::new(),
    };
    collector.visit_program(&parsed.program);

    // Map local root-scope binding name -> SymbolId.
    let mut root_symbols: HashMap<String, SymbolId> = HashMap::new();
    for (name, &sym_id) in scoping.get_bindings(root_scope) {
        root_symbols.insert(name.as_str().to_owned(), sym_id);
    }

    let mut symbol_renames: HashMap<SymbolId, String> = HashMap::new();
    let mut import_symbols: HashSet<SymbolId> = HashSet::new();
    let mut deleted_ranges: Vec<(u32, u32)> = Vec::new();
    let mut edits: Vec<(Span, String)> = Vec::new();
    let mut exports: HashMap<String, String> = HashMap::new();

    // 1. Inspect top-level imports & exports.
    for stmt in &parsed.program.body {
        match stmt {
            Statement::ImportDeclaration(decl) => {
                let spec = decl.source.value.as_str();
                let resolved_opt = module.resolved_imports.get(spec)?;
                if let Some(target_id) = resolved_opt {
                    let target_exports = export_tables.get(target_id)?;
                    if let Some(specifiers) = &decl.specifiers {
                        for s in specifiers {
                            match s {
                                ImportDeclarationSpecifier::ImportSpecifier(named) => {
                                    let imported = named.imported.name().as_str();
                                    let local = named.local.name.as_str();
                                    let hoisted = target_exports.get(imported)?;
                                    let &sym_id = root_symbols.get(local)?;
                                    symbol_renames.insert(sym_id, hoisted.clone());
                                    import_symbols.insert(sym_id);
                                }
                                ImportDeclarationSpecifier::ImportDefaultSpecifier(def) => {
                                    let local = def.local.name.as_str();
                                    let hoisted = target_exports.get("default")?;
                                    let &sym_id = root_symbols.get(local)?;
                                    symbol_renames.insert(sym_id, hoisted.clone());
                                    import_symbols.insert(sym_id);
                                }
                                ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => {
                                    return None;
                                }
                            }
                        }
                    }
                }
                deleted_ranges.push((decl.span.start, decl.span.end));
                edits.push((decl.span, String::new()));
            }
            Statement::ExportDeclaration(decl) => {
                let inner_start = decl.declaration.span().start;
                edits.push((Span::new(decl.span.start, inner_start), String::new()));
                collect_exported_decl_names(&decl.declaration, mod_idx, &mut exports);
            }
            Statement::ExportNamedDeclaration(decl) => {
                if decl.specifiers.is_empty() {
                    deleted_ranges.push((decl.span.start, decl.span.end));
                    edits.push((decl.span, String::new()));
                    continue;
                }
                for spec in &decl.specifiers {
                    let local = spec.local.name().as_str();
                    let exported = spec.exported.name().as_str();
                    let target_ident = root_symbols
                        .get(local)
                        .and_then(|sym| symbol_renames.get(sym).cloned())
                        .unwrap_or_else(|| format!("{local}_m{mod_idx}"));
                    exports.insert(exported.to_owned(), target_ident);
                }
                deleted_ranges.push((decl.span.start, decl.span.end));
                edits.push((decl.span, String::new()));
            }
            Statement::ExportDefaultDeclaration(decl) => {
                let default_ident = format!("__zerho_default_m{mod_idx}");
                match &decl.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        if let Some(id) = &f.id {
                            let name = id.name.as_str();
                            let inner_start = f.span.start;
                            edits.push((Span::new(decl.span.start, inner_start), String::new()));
                            exports.insert("default".to_owned(), format!("{name}_m{mod_idx}"));
                        } else {
                            let inner_start = f.span.start;
                            edits.push((
                                Span::new(decl.span.start, inner_start),
                                format!("const {default_ident} = "),
                            ));
                            exports.insert("default".to_owned(), default_ident);
                        }
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                        if let Some(id) = &c.id {
                            let name = id.name.as_str();
                            let inner_start = c.span.start;
                            edits.push((Span::new(decl.span.start, inner_start), String::new()));
                            exports.insert("default".to_owned(), format!("{name}_m{mod_idx}"));
                        } else {
                            let inner_start = c.span.start;
                            edits.push((
                                Span::new(decl.span.start, inner_start),
                                format!("const {default_ident} = "),
                            ));
                            exports.insert("default".to_owned(), default_ident);
                        }
                    }
                    _ => {
                        let inner_start = decl.declaration.span().start;
                        edits.push((
                            Span::new(decl.span.start, inner_start),
                            format!("const {default_ident} = "),
                        ));
                        exports.insert("default".to_owned(), default_ident);
                    }
                }
            }
            Statement::ExportFromDeclaration(_) | Statement::ExportAllDeclaration(_) => {
                return None;
            }
            _ => {}
        }
    }

    // 2. Rename all non-import root-scope local bindings to `{name}_m{mod_idx}`.
    for (name, &sym_id) in &root_symbols {
        if !import_symbols.contains(&sym_id) {
            symbol_renames.insert(sym_id, format!("{name}_m{mod_idx}"));
        }
    }

    let inside_deleted = |span: Span| -> bool {
        deleted_ranges
            .iter()
            .any(|&(s, e)| span.start >= s && span.end <= e)
    };

    // 3. Emit span edits for declarations and resolved references of every renamed root symbol.
    for (orig_name, &sym_id) in &root_symbols {
        let Some(new_name) = symbol_renames.get(&sym_id) else {
            continue;
        };

        if !import_symbols.contains(&sym_id) {
            let decl_span = scoping.symbol_span(sym_id);
            if !inside_deleted(decl_span) {
                edits.push((decl_span, new_name.clone()));
            }
        }

        for ref_id in scoping.get_resolved_reference_ids(sym_id) {
            let Some(&ref_span) = collector.ref_spans.get(ref_id) else {
                continue;
            };
            if inside_deleted(ref_span) {
                continue;
            }
            if collector
                .shorthand_spans
                .contains(&(ref_span.start, ref_span.end))
            {
                edits.push((ref_span, format!("{orig_name}: {new_name}")));
            } else {
                edits.push((ref_span, new_name.clone()));
            }
        }
    }

    // 4. Apply non-overlapping edits right-to-left.
    edits.sort_by(|a, b| {
        b.0.start
            .cmp(&a.0.start)
            .then_with(|| b.0.end.cmp(&a.0.end))
    });
    let mut output = module.code.clone();
    let mut last_start = usize::MAX;
    for (span, replacement) in edits {
        let start = span.start as usize;
        let end = span.end as usize;
        if end <= last_start && end <= output.len() && start <= end {
            output.replace_range(start..end, &replacement);
            last_start = start;
        }
    }

    Some((output, exports))
}

fn collect_exported_decl_names(
    decl: &Declaration<'_>,
    mod_idx: usize,
    exports: &mut HashMap<String, String>,
) {
    match decl {
        Declaration::VariableDeclaration(var_decl) => {
            for declarator in &var_decl.declarations {
                if let Some(ident) = declarator.id.get_binding_identifier() {
                    let name = ident.name.as_str();
                    exports.insert(name.to_owned(), format!("{name}_m{mod_idx}"));
                }
            }
        }
        Declaration::FunctionDeclaration(func) => {
            if let Some(id) = &func.id {
                let name = id.name.as_str();
                exports.insert(name.to_owned(), format!("{name}_m{mod_idx}"));
            }
        }
        Declaration::ClassDeclaration(class) => {
            if let Some(id) = &class.id {
                let name = id.name.as_str();
                exports.insert(name.to_owned(), format!("{name}_m{mod_idx}"));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_hoists_acyclic_esm_and_prevents_symbol_collisions() {
        let math_mod = HoistedModuleInput {
            id: "src/math.ts".to_owned(),
            code: "const factor = 10;\nexport function scale(n) { return n * factor; }\n"
                .to_owned(),
            synthetic_default_expr: None,
            resolved_imports: BTreeMap::new(),
        };
        let mut entry_imports = BTreeMap::new();
        entry_imports.insert("./math".to_owned(), Some("src/math.ts".to_owned()));
        let entry_mod = HoistedModuleInput {
            id: "src/main.ts".to_owned(),
            code: "import { scale } from './math';\nconst factor = 2;\nconsole.log({ factor, result: scale(factor) });\n".to_owned(),
            synthetic_default_expr: None,
            resolved_imports: entry_imports,
        };

        let hoisted = try_scope_hoist_bundle(&[math_mod, entry_mod])
            .expect("acyclic ESM graph should scope-hoist");

        assert!(!hoisted.contains("__zerho_require"));
        assert!(hoisted.contains("const factor_m0 = 10;"));
        assert!(hoisted.contains("const factor_m1 = 2;"));
        assert!(hoisted.contains("factor: factor_m1"));
        assert!(hoisted.contains("scale_m0(factor_m1)"));
    }
}
