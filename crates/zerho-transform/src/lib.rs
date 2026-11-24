pub mod barrel;
pub mod cjs;
pub mod css_assets;
pub mod scope_hoist;
pub mod svg;
pub mod workers;

use lightningcss::css_modules::Config as CssModulesConfig;
use lightningcss::stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet};
use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Declaration, Expression, IdentifierReference, ImportDeclarationSpecifier, ImportExpression,
    Statement,
};
use oxc::ast_visit::Visit;
use oxc::codegen::{Codegen, CodegenOptions};
use oxc::minifier::{CompressOptions, MangleOptions, Minifier, MinifierOptions};
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::{SourceType, Span};
use oxc::transformer::{
    JsxOptions, JsxRuntime, ReactRefreshOptions, TransformOptions, Transformer,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use zerho_core::{ContentHash, Framework, ModuleKind, ZerhoError, ZerhoResult};
use zerho_plugin::{PluginContext, PluginRegistry};

pub use barrel::{BarrelExport, parse_pure_barrel, rewrite_barrel_imports};
pub use cjs::{collect_cjs_requires, prebundle_cjs_graph_to_esm, rewrite_cjs_requires};
pub use css_assets::{inline_css_imports, rewrite_css_urls};
pub use scope_hoist::{HoistedModuleInput, try_scope_hoist_bundle};
pub use svg::optimize_svg;
pub use workers::{collect_web_worker_urls, rewrite_web_worker_urls};

/// Input to the native transform pipeline.
#[derive(Debug, Clone)]
pub struct TransformInput {
    pub path: PathBuf,
    pub kind: ModuleKind,
    pub source: String,
    pub framework: Framework,
    pub is_dev: bool,
    pub minify_css: bool,
}

/// Output produced by the native transform pipeline, including AST-extracted static imports
/// and optional scoped class mappings for `*.module.css`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformOutput {
    pub code: String,
    pub imports: Vec<String>,
    pub css_exports: Option<BTreeMap<String, String>>,
    pub hash: ContentHash,
    pub sourcemap: Option<String>,
}

/// Native Rust transform pipeline powered by `oxc` (TS/JSX) and `lightningcss` (CSS).
#[derive(Debug, Default)]
pub struct TransformPipeline {
    plugins: PluginRegistry,
}

impl TransformPipeline {
    pub fn new(plugins: PluginRegistry) -> Self {
        Self { plugins }
    }

    pub fn transform(&self, input: &TransformInput) -> ZerhoResult<TransformOutput> {
        let ctx = PluginContext {
            is_dev: input.is_dev,
        };
        let id = input.path.to_string_lossy();
        let after_plugins =
            self.plugins
                .run_transform(input.source.clone(), &id, input.kind, &ctx)?;

        let (transformed_code, imports, css_exports) = match input.kind {
            ModuleKind::Js | ModuleKind::Ts | ModuleKind::Jsx | ModuleKind::Tsx => {
                let (code, imports) = transform_script(
                    &input.path,
                    &after_plugins.code,
                    input.framework,
                    input.is_dev,
                )?;
                (code, imports, None)
            }
            ModuleKind::Css => {
                let (code, exports) =
                    transform_css(&input.path, &after_plugins.code, input.minify_css)?;
                (code, Vec::new(), exports)
            }
            ModuleKind::Json => (
                format!("export default {};\n", after_plugins.code.trim()),
                Vec::new(),
                None,
            ),
            ModuleKind::Asset | ModuleKind::Virtual => (after_plugins.code, Vec::new(), None),
        };

        let hash = ContentHash::from_bytes(transformed_code.as_bytes());
        Ok(TransformOutput {
            code: transformed_code,
            imports,
            css_exports,
            hash,
            sourcemap: after_plugins.sourcemap,
        })
    }
}

fn transform_script(
    path: &Path,
    source: &str,
    framework: Framework,
    is_dev: bool,
) -> ZerhoResult<(String, Vec<String>)> {
    let source_type = SourceType::from_path(path).unwrap_or_default();
    let allocator = Allocator::default();

    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(ZerhoError::Transform {
            path: path.to_path_buf(),
            reason: messages.join("\n"),
        });
    }

    let mut program = parsed.program;
    let semantic = SemanticBuilder::new().build(&program);
    let scoping = semantic.semantic.into_scoping();

    let is_jsx_file = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tsx") || ext.eq_ignore_ascii_case("jsx"));

    let options = TransformOptions {
        jsx: match framework {
            Framework::React => JsxOptions {
                runtime: JsxRuntime::Automatic,
                development: is_dev,
                refresh: (is_dev && is_jsx_file).then(ReactRefreshOptions::default),
                ..JsxOptions::default()
            },
            Framework::Preact => JsxOptions {
                runtime: JsxRuntime::Automatic,
                import_source: Some("preact".to_owned()),
                development: is_dev,
                ..JsxOptions::default()
            },
            Framework::Solid | Framework::Vanilla => JsxOptions {
                runtime: JsxRuntime::Classic,
                pragma: Some("h".to_owned()),
                pragma_frag: Some("Fragment".to_owned()),
                development: false,
                ..JsxOptions::default()
            },
        },
        ..TransformOptions::default()
    };

    let ret =
        Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);

    if !ret.diagnostics.is_empty() {
        let messages: Vec<String> = ret.diagnostics.iter().map(ToString::to_string).collect();
        return Err(ZerhoError::Transform {
            path: path.to_path_buf(),
            reason: messages.join("\n"),
        });
    }

    // Extract static import/export specifiers from the post-transform AST (after type-only imports are erased
    // and JSX runtime imports are injected).
    let imports = collect_static_imports(&program.body);
    let codegen = Codegen::new().build(&program);
    Ok((codegen.code, imports))
}

fn collect_static_imports(body: &[Statement<'_>]) -> Vec<String> {
    let mut imports = Vec::new();
    for stmt in body {
        let specifier = match stmt {
            Statement::ImportDeclaration(decl) => Some(decl.source.value.as_str()),
            Statement::ExportFromDeclaration(decl) => Some(decl.source.value.as_str()),
            Statement::ExportAllDeclaration(decl) => Some(decl.source.value.as_str()),
            _ => None,
        };
        if let Some(spec) = specifier {
            imports.push(spec.to_owned());
        }
    }
    imports
}

/// AST-accurate ESM import/re-export specifier rewriter using `oxc_parser` spans.
/// Rewrites extensionless relative imports (e.g. `./counter` -> `/src/counter.ts`)
/// and bare `node_modules` imports (e.g. `react` -> `/@zerho/deps/react`).
pub fn rewrite_imports(
    js_code: &str,
    path: &Path,
    mut rewriter: impl FnMut(&str) -> Option<String>,
) -> ZerhoResult<String> {
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
    for stmt in &parsed.program.body {
        let source_lit = match stmt {
            Statement::ImportDeclaration(decl) => Some(&decl.source),
            Statement::ExportFromDeclaration(decl) => Some(&decl.source),
            Statement::ExportAllDeclaration(decl) => Some(&decl.source),
            _ => None,
        };
        if let Some(lit) = source_lit
            && let Some(replacement) = rewriter(lit.value.as_str())
        {
            edits.push((lit.span, format!("\"{}\"", escape_js_string(&replacement))));
        }
    }

    if edits.is_empty() {
        return Ok(js_code.to_owned());
    }

    // Apply replacements from right to left so byte spans remain valid.
    edits.sort_by_key(|b| std::cmp::Reverse(b.0.start));
    let mut output = js_code.to_owned();
    for (span, replacement) in edits {
        let start = span.start as usize;
        let end = span.end as usize;
        if end <= output.len() && start <= end {
            output.replace_range(start..end, &replacement);
        }
    }
    Ok(output)
}

/// Lowers an ESM module into a CommonJS-like factory body (`__zerho_require`, `__zerho_exports`)
/// for zero-waterfall single-file production bundling.
pub fn lower_esm_for_bundle(
    js_code: &str,
    path: &Path,
    mut resolve_dep: impl FnMut(&str) -> Option<String>,
) -> ZerhoResult<String> {
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
    let mut trailing_exports: Vec<String> = Vec::new();

    for stmt in &parsed.program.body {
        match stmt {
            Statement::ImportDeclaration(decl) => {
                let dep_id = resolve_dep(decl.source.value.as_str());
                edits.push((decl.span, lower_import_decl(decl, dep_id.as_deref())));
            }
            Statement::ExportDeclaration(decl) => {
                let code = lower_export_decl(js_code, decl, &mut trailing_exports);
                edits.push((decl.span, code));
            }
            Statement::ExportNamedDeclaration(decl) if !decl.specifiers.is_empty() => {
                let exports: Vec<String> = decl
                    .specifiers
                    .iter()
                    .map(|spec| {
                        format!(
                            "__zerho_exports.{} = {};",
                            spec.exported.name(),
                            spec.local.name()
                        )
                    })
                    .collect();
                edits.push((decl.span, exports.join("\n")));
            }
            Statement::ExportDefaultDeclaration(decl) => {
                let raw = js_code
                    .get(decl.span.start as usize..decl.span.end as usize)
                    .unwrap_or_default();
                let stripped = raw
                    .strip_prefix("export default")
                    .map_or(raw, str::trim_start);
                edits.push((decl.span, format!("__zerho_exports.default = {stripped}")));
            }
            _ => {}
        }
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

    if !trailing_exports.is_empty() {
        output.push('\n');
        output.push_str(&trailing_exports.join("\n"));
        output.push('\n');
    }

    Ok(output)
}

fn lower_import_decl(decl: &oxc::ast::ast::ImportDeclaration<'_>, dep_id: Option<&str>) -> String {
    let Some(dep_id) = dep_id else {
        return String::new();
    };
    let escaped_id = escape_js_string(dep_id);
    match &decl.specifiers {
        None => format!("__zerho_require(\"{escaped_id}\");"),
        Some(specifiers) if specifiers.is_empty() => {
            format!("__zerho_require(\"{escaped_id}\");")
        }
        Some(specifiers) => {
            let mut lines = Vec::new();
            let mut named = Vec::new();
            for s in specifiers {
                match s {
                    ImportDeclarationSpecifier::ImportSpecifier(named_spec) => {
                        let imported = named_spec.imported.name();
                        let local = named_spec.local.name.as_str();
                        if imported == local {
                            named.push(local.to_owned());
                        } else {
                            named.push(format!("{imported}: {local}"));
                        }
                    }
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(def_spec) => {
                        lines.push(format!(
                            "const {} = __zerho_require(\"{escaped_id}\").default;",
                            def_spec.local.name
                        ));
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(ns_spec) => {
                        lines.push(format!(
                            "const {} = __zerho_require(\"{escaped_id}\");",
                            ns_spec.local.name
                        ));
                    }
                }
            }
            if !named.is_empty() {
                lines.push(format!(
                    "const {{ {} }} = __zerho_require(\"{escaped_id}\");",
                    named.join(", ")
                ));
            }
            lines.join("\n")
        }
    }
}

fn lower_export_decl(
    js_code: &str,
    decl: &oxc::ast::ast::ExportDeclaration<'_>,
    trailing_exports: &mut Vec<String>,
) -> String {
    match &decl.declaration {
        Declaration::FunctionDeclaration(func) => {
            if let Some(id) = &func.id {
                trailing_exports.push(format!("__zerho_exports.{0} = {0};", id.name));
            }
            js_code
                .get(func.span.start as usize..func.span.end as usize)
                .unwrap_or_default()
                .to_owned()
        }
        Declaration::ClassDeclaration(cls) => {
            if let Some(id) = &cls.id {
                trailing_exports.push(format!("__zerho_exports.{0} = {0};", id.name));
            }
            js_code
                .get(cls.span.start as usize..cls.span.end as usize)
                .unwrap_or_default()
                .to_owned()
        }
        Declaration::VariableDeclaration(var_decl) => {
            for declarator in &var_decl.declarations {
                if let Some(ident) = declarator.id.get_identifier_name() {
                    trailing_exports.push(format!("__zerho_exports.{ident} = {ident};"));
                }
            }
            js_code
                .get(var_decl.span.start as usize..var_decl.span.end as usize)
                .unwrap_or_default()
                .to_owned()
        }
        _ => js_code
            .get(decl.span.start as usize..decl.span.end as usize)
            .unwrap_or_default()
            .to_owned(),
    }
}

/// Minifies or formats a JavaScript bundle using `oxc_codegen`.
pub fn finalize_js_bundle(js_code: &str, minify: bool) -> ZerhoResult<String> {
    let (code, _) = finalize_js_bundle_with_map(js_code, "bundle.js", minify, false)?;
    Ok(code)
}

/// Minifies (AST constant folding, DCE, identifier mangling) or formats a JavaScript bundle
/// and optionally emits a Source Map v3 JSON string via `oxc_minifier` + `oxc_codegen`.
pub fn finalize_js_bundle_with_map(
    js_code: &str,
    file_name: &str,
    minify: bool,
    emit_map: bool,
) -> ZerhoResult<(String, Option<String>)> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, js_code, SourceType::mjs()).parse();
    if !parsed.diagnostics.is_empty() {
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(ZerhoError::Transform {
            path: PathBuf::from(file_name),
            reason: messages.join("\n"),
        });
    }

    let mut program = parsed.program;
    let scoping = if minify {
        let min_opts = MinifierOptions {
            mangle: Some(MangleOptions::default()),
            compress: Some(CompressOptions::default()),
        };
        Minifier::new(min_opts)
            .minify(&allocator, &mut program)
            .scoping
    } else {
        None
    };

    let options = CodegenOptions {
        minify,
        source_map_path: emit_map.then(|| PathBuf::from(file_name)),
        ..CodegenOptions::default()
    };
    let built = Codegen::new()
        .with_options(options)
        .with_scoping(scoping)
        .build(&program);
    let map_json = built.map.map(|m| m.to_json_string());
    Ok((built.code, map_json))
}

struct DynamicImportCollector {
    imports: Vec<(Span, String)>,
}

impl<'a> Visit<'a> for DynamicImportCollector {
    fn visit_import_expression(&mut self, expr: &ImportExpression<'a>) {
        if let Expression::StringLiteral(lit) = &expr.source {
            self.imports.push((lit.span, lit.value.as_str().to_owned()));
        }
        oxc::ast_visit::walk::walk_import_expression(self, expr);
    }
}

/// Collects all statically analyzable dynamic `import("...")` specifiers via `oxc_ast_visit`.
pub fn collect_dynamic_imports(js_code: &str, path: &Path) -> ZerhoResult<Vec<String>> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, js_code, SourceType::mjs()).parse();
    if !parsed.diagnostics.is_empty() {
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(ZerhoError::Transform {
            path: path.to_path_buf(),
            reason: messages.join("\n"),
        });
    }

    let mut collector = DynamicImportCollector {
        imports: Vec::new(),
    };
    collector.visit_program(&parsed.program);
    Ok(collector.imports.into_iter().map(|(_, s)| s).collect())
}

/// Rewrites dynamic `import("...")` specifiers using AST spans (e.g. `./lazy` -> `/assets/chunk-<hash>.js`).
pub fn rewrite_dynamic_imports(
    js_code: &str,
    path: &Path,
    mut rewriter: impl FnMut(&str) -> Option<String>,
) -> ZerhoResult<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, js_code, SourceType::mjs()).parse();
    if !parsed.diagnostics.is_empty() {
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(ZerhoError::Transform {
            path: path.to_path_buf(),
            reason: messages.join("\n"),
        });
    }

    let mut collector = DynamicImportCollector {
        imports: Vec::new(),
    };
    collector.visit_program(&parsed.program);

    let mut edits: Vec<(Span, String)> = Vec::new();
    for (span, spec) in collector.imports {
        if let Some(replacement) = rewriter(&spec) {
            edits.push((span, format!("\"{}\"", escape_js_string(&replacement))));
        }
    }

    if edits.is_empty() {
        return Ok(js_code.to_owned());
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
    Ok(output)
}

/// Heuristic check for `CommonJS` modules (`module.exports` / `exports.`) without ESM syntax.
pub fn is_likely_commonjs(code: &str) -> bool {
    let has_cjs = code.contains("module.exports") || code.contains("exports.");
    let has_esm = code.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("export ") || trimmed.starts_with("import ")
    });
    has_cjs && !has_esm
}

/// Wraps a `CommonJS` module into a browser-compatible ESM module for Zero-Node pre-bundling.
pub fn wrap_cjs_as_esm(cjs_code: &str) -> String {
    format!(
        "const __zerho_cjs_module = {{ exports: {{}} }};\n\
         (function(module, exports) {{\n\
         {cjs_code}\n\
         }})(__zerho_cjs_module, __zerho_cjs_module.exports);\n\
         export default __zerho_cjs_module.exports;\n"
    )
}

/// Pure-Rust base64 encoder used for `sourcemap = "inline"` without extra crate dependencies.
pub fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 {
            u32::from(chunk[1])
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            u32::from(chunk[2])
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Set of exported symbols imported from a target module by one or more importers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportedSymbols {
    /// Namespace import (`import * as ns`), side-effect import, or entrypoint — keep all exports.
    All,
    /// Specific named/default exports referenced by importers.
    Named(BTreeSet<String>),
}

impl ImportedSymbols {
    pub fn merge(&mut self, other: Self) {
        match (self, other) {
            (Self::All, _) => {}
            (slot, Self::All) => *slot = Self::All,
            (Self::Named(existing), Self::Named(incoming)) => existing.extend(incoming),
        }
    }
}

/// Result of AST-level dead-export elimination on a single module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeShakeResult {
    pub code: String,
    pub removed_exports: Vec<String>,
}

/// Extracts `(specifier, ImportedSymbols)` for every static import/re-export in an ESM module.
pub fn collect_imported_symbols(
    js_code: &str,
    path: &Path,
) -> ZerhoResult<Vec<(String, ImportedSymbols)>> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, js_code, SourceType::mjs()).parse();
    if !parsed.diagnostics.is_empty() {
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(ZerhoError::Transform {
            path: path.to_path_buf(),
            reason: messages.join("\n"),
        });
    }

    let mut result = Vec::new();
    for stmt in &parsed.program.body {
        match stmt {
            Statement::ImportDeclaration(decl) => {
                let spec = decl.source.value.as_str().to_owned();
                let symbols = match &decl.specifiers {
                    None => ImportedSymbols::All,
                    Some(specs) if specs.is_empty() => ImportedSymbols::All,
                    Some(specs) => {
                        let mut named = BTreeSet::new();
                        let mut is_all = false;
                        for s in specs {
                            match s {
                                ImportDeclarationSpecifier::ImportSpecifier(n) => {
                                    named.insert(n.imported.name().to_string());
                                }
                                ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => {
                                    named.insert("default".to_owned());
                                }
                                ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => {
                                    is_all = true;
                                }
                            }
                        }
                        if is_all {
                            ImportedSymbols::All
                        } else {
                            ImportedSymbols::Named(named)
                        }
                    }
                };
                result.push((spec, symbols));
            }
            Statement::ExportAllDeclaration(decl) => {
                result.push((decl.source.value.as_str().to_owned(), ImportedSymbols::All));
            }
            Statement::ExportFromDeclaration(decl) => {
                let mut named = BTreeSet::new();
                for s in &decl.specifiers {
                    named.insert(s.local.name().to_string());
                }
                result.push((
                    decl.source.value.as_str().to_owned(),
                    ImportedSymbols::Named(named),
                ));
            }
            _ => {}
        }
    }
    Ok(result)
}

struct IdentRefCollector {
    refs: BTreeSet<String>,
}

impl<'a> Visit<'a> for IdentRefCollector {
    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        self.refs.insert(ident.name.as_str().to_owned());
    }
}

/// Eliminates unused exported functions, classes, and pure constants via `oxc` AST analysis,
/// preserving any export that is still referenced internally by live statements in the same module.
pub fn prune_unused_exports(
    js_code: &str,
    path: &Path,
    used: &ImportedSymbols,
) -> ZerhoResult<TreeShakeResult> {
    let ImportedSymbols::Named(used_names) = used else {
        return Ok(TreeShakeResult {
            code: js_code.to_owned(),
            removed_exports: Vec::new(),
        });
    };

    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, js_code, SourceType::mjs()).parse();
    if !parsed.diagnostics.is_empty() {
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(ZerhoError::Transform {
            path: path.to_path_buf(),
            reason: messages.join("\n"),
        });
    }

    // First identify candidate dead export statements by index.
    let mut candidate_dead: Vec<(usize, Span, String)> = Vec::new();
    for (idx, stmt) in parsed.program.body.iter().enumerate() {
        match stmt {
            Statement::ExportDeclaration(decl) => {
                if let Some(name) = single_pure_export_name(&decl.declaration)
                    && !used_names.contains(&name)
                {
                    candidate_dead.push((idx, decl.span, name));
                }
            }
            Statement::ExportDefaultDeclaration(decl) if !used_names.contains("default") => {
                candidate_dead.push((idx, decl.span, "default".to_owned()));
            }
            _ => {}
        }
    }

    if candidate_dead.is_empty() {
        return Ok(TreeShakeResult {
            code: js_code.to_owned(),
            removed_exports: Vec::new(),
        });
    }

    // Collect all identifier references from live (non-candidate) statements; iteratively retain
    // any candidate export referenced by a live statement (transitive intra-module dependencies).
    let mut dead_indices: BTreeSet<usize> = candidate_dead.iter().map(|(i, _, _)| *i).collect();
    loop {
        let mut live_refs = IdentRefCollector {
            refs: BTreeSet::new(),
        };
        for (idx, stmt) in parsed.program.body.iter().enumerate() {
            if !dead_indices.contains(&idx) {
                live_refs.visit_statement(stmt);
            }
        }
        let mut resurrected = false;
        for (idx, _, name) in &candidate_dead {
            if dead_indices.contains(idx) && live_refs.refs.contains(name) {
                dead_indices.remove(idx);
                resurrected = true;
            }
        }
        if !resurrected {
            break;
        }
    }

    let mut edits: Vec<Span> = Vec::new();
    let mut removed_exports = Vec::new();
    for (idx, span, name) in candidate_dead {
        if dead_indices.contains(&idx) {
            edits.push(span);
            removed_exports.push(name);
        }
    }

    if edits.is_empty() {
        return Ok(TreeShakeResult {
            code: js_code.to_owned(),
            removed_exports,
        });
    }

    edits.sort_by_key(|span| std::cmp::Reverse(span.start));
    let mut output = js_code.to_owned();
    for span in edits {
        let start = span.start as usize;
        let end = span.end as usize;
        if end <= output.len() && start <= end {
            output.replace_range(start..end, "");
        }
    }

    Ok(TreeShakeResult {
        code: output,
        removed_exports,
    })
}

fn single_pure_export_name(decl: &Declaration<'_>) -> Option<String> {
    match decl {
        Declaration::FunctionDeclaration(f) => f.id.as_ref().map(|id| id.name.as_str().to_owned()),
        Declaration::ClassDeclaration(c) => c.id.as_ref().map(|id| id.name.as_str().to_owned()),
        Declaration::VariableDeclaration(v) if v.declarations.len() == 1 => {
            let d = &v.declarations[0];
            let is_pure_init = d.init.as_ref().is_some_and(|expr| {
                matches!(
                    expr,
                    Expression::StringLiteral(_)
                        | Expression::NumericLiteral(_)
                        | Expression::BooleanLiteral(_)
                        | Expression::NullLiteral(_)
                        | Expression::ArrowFunctionExpression(_)
                        | Expression::FunctionExpression(_)
                        | Expression::ObjectExpression(_)
                        | Expression::ArrayExpression(_)
                )
            });
            if is_pure_init {
                d.id.get_identifier_name().map(|n| n.as_str().to_owned())
            } else {
                None
            }
        }
        _ => None,
    }
}

type CssTransformResult = (String, Option<BTreeMap<String, String>>);

fn is_css_module(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.ends_with(".module.css"))
}

fn transform_css(path: &Path, source: &str, minify: bool) -> ZerhoResult<CssTransformResult> {
    let filename = path.to_string_lossy().into_owned();
    let css_modules = is_css_module(path).then(CssModulesConfig::default);

    let mut stylesheet = StyleSheet::parse(
        source,
        ParserOptions {
            filename,
            css_modules,
            ..ParserOptions::default()
        },
    )
    .map_err(|err| ZerhoError::Transform {
        path: path.to_path_buf(),
        reason: err.to_string(),
    })?;

    if minify {
        stylesheet
            .minify(MinifyOptions::default())
            .map_err(|err| ZerhoError::Transform {
                path: path.to_path_buf(),
                reason: err.to_string(),
            })?;
    }

    let printed = stylesheet
        .to_css(PrinterOptions {
            minify,
            ..PrinterOptions::default()
        })
        .map_err(|err| ZerhoError::Transform {
            path: path.to_path_buf(),
            reason: err.to_string(),
        })?;

    let exports = printed.exports.map(|map| {
        map.into_iter()
            .map(|(k, v)| (k, v.name))
            .collect::<BTreeMap<String, String>>()
    });

    Ok((printed.code, exports))
}

/// Wraps compiled CSS into an ESM module that injects/updates a `<style>` tag in dev mode.
/// When `css_exports` is present (`*.module.css`), exports the scoped class name map as `default`.
pub fn wrap_css_as_hmr_module(id: &str, css: &str) -> String {
    wrap_css_with_exports_as_hmr_module(id, css, None)
}

/// Wraps compiled CSS or CSS Modules into an HMR-capable ESM module.
pub fn wrap_css_with_exports_as_hmr_module(
    id: &str,
    css: &str,
    css_exports: Option<&BTreeMap<String, String>>,
) -> String {
    let escaped_id = escape_js_string(id);
    let escaped_css = escape_js_string(css);
    let export_stmt = match css_exports {
        Some(map) => format!("export default {};\n", serialize_css_exports_json(map)),
        None => "export default __zerhoCss;\n".to_owned(),
    };
    format!(
        "const __zerhoId = \"{escaped_id}\";\n\
         const __zerhoCss = \"{escaped_css}\";\n\
         let __zerhoStyle = document.querySelector(`style[data-zerho-id=\"${{__zerhoId}}\"]`);\n\
         if (!__zerhoStyle) {{\n\
           __zerhoStyle = document.createElement(\"style\");\n\
           __zerhoStyle.setAttribute(\"data-zerho-id\", __zerhoId);\n\
           document.head.appendChild(__zerhoStyle);\n\
         }}\n\
         __zerhoStyle.textContent = __zerhoCss;\n\
         {export_stmt}"
    )
}

/// Serializes a CSS Modules class mapping into a JS object literal string.
pub fn serialize_css_exports_json(map: &BTreeMap<String, String>) -> String {
    let entries: Vec<String> = map
        .iter()
        .map(|(k, v)| format!("\"{}\":\"{}\"", escape_js_string(k), escape_js_string(v)))
        .collect();
    format!("{{{}}}", entries.join(","))
}

fn escape_js_string(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_typescript_types_and_extracts_runtime_imports() {
        let pipeline = TransformPipeline::default();
        let out = pipeline
            .transform(&TransformInput {
                path: PathBuf::from("src/main.ts"),
                kind: ModuleKind::Ts,
                source: "import { increment, type CounterState } from './counter';\n\
                         import './styles.css';\n\
                         export const run = (s: CounterState) => increment(s);"
                    .to_owned(),
                framework: Framework::Vanilla,
                is_dev: true,
                minify_css: false,
            })
            .expect("ts transform should succeed");
        assert!(!out.code.contains("CounterState"));
        assert_eq!(out.imports, vec!["./counter", "./styles.css"]);
    }

    #[test]
    fn rewrites_relative_and_bare_imports_via_ast_spans() {
        let code = "import { a } from \"./counter\";\nimport React from \"react\";\nexport * from \"./utils\";\n";
        let rewritten = rewrite_imports(code, Path::new("src/main.ts"), |spec| match spec {
            "./counter" => Some("/src/counter.ts".to_owned()),
            "react" => Some("/@zerho/deps/react".to_owned()),
            "./utils" => Some("/src/utils.ts".to_owned()),
            _ => None,
        })
        .expect("rewrite should succeed");

        assert!(rewritten.contains("\"/src/counter.ts\""));
        assert!(rewritten.contains("\"/@zerho/deps/react\""));
        assert!(rewritten.contains("\"/src/utils.ts\""));
    }

    #[test]
    fn compiles_react_jsx_with_oxc_and_captures_jsx_runtime_import() {
        let pipeline = TransformPipeline::default();
        let out = pipeline
            .transform(&TransformInput {
                path: PathBuf::from("src/App.tsx"),
                kind: ModuleKind::Tsx,
                source: "export function App(): JSX.Element { return <main><h1>Hello zerho</h1></main>; }"
                    .to_owned(),
                framework: Framework::React,
                is_dev: false,
                minify_css: false,
            })
            .expect("tsx transform should succeed");
        assert!(out.imports.iter().any(|imp| imp == "react/jsx-runtime"));
        assert!(out.code.contains("Hello zerho"));
    }

    #[test]
    fn compiles_and_minifies_css_with_lightningcss() {
        let pipeline = TransformPipeline::default();
        let out = pipeline
            .transform(&TransformInput {
                path: PathBuf::from("src/styles.css"),
                kind: ModuleKind::Css,
                source: ".card { color: #ff0000; & .title { font-weight: 700; } }".to_owned(),
                framework: Framework::React,
                is_dev: false,
                minify_css: true,
            })
            .expect("css transform should succeed");
        assert!(out.code.contains(".card"));
        assert!(out.code.contains("font-weight:700"));
    }

    #[test]
    fn transforms_json_into_esm_module() {
        let pipeline = TransformPipeline::default();
        let out = pipeline
            .transform(&TransformInput {
                path: PathBuf::from("data.json"),
                kind: ModuleKind::Json,
                source: "{\"ok\":true}".to_owned(),
                framework: Framework::React,
                is_dev: true,
                minify_css: false,
            })
            .expect("json transform should succeed");
        assert_eq!(out.code, "export default {\"ok\":true};\n");
    }

    #[test]
    fn detects_and_rewrites_dynamic_imports() {
        let src = "async function load() { return await import('./lazy-panel'); }";
        let found = collect_dynamic_imports(src, Path::new("src/main.ts"))
            .expect("dynamic import collection should succeed");
        assert_eq!(found, vec!["./lazy-panel"]);

        let rewritten = rewrite_dynamic_imports(src, Path::new("src/main.ts"), |spec| {
            (spec == "./lazy-panel").then(|| "/assets/chunk-12345678.js".to_owned())
        })
        .expect("dynamic import rewrite should succeed");
        assert!(rewritten.contains("\"/assets/chunk-12345678.js\""));
    }

    #[test]
    fn wraps_commonjs_into_esm_and_emits_sourcemap_v3() {
        let cjs = "module.exports = { badge: 'zerho-cjs' };";
        assert!(is_likely_commonjs(cjs));
        let esm = wrap_cjs_as_esm(cjs);
        assert!(esm.contains("export default __zerho_cjs_module.exports;"));

        let (min, map) = finalize_js_bundle_with_map(&esm, "assets/index.js", true, true)
            .expect("sourcemap generation should succeed");
        assert!(!min.is_empty());
        let map_json = map.expect("source map should be present");
        assert!(map_json.contains("\"version\":3"));
    }

    #[test]
    fn tree_shakes_unused_exports_while_preserving_internal_helpers() {
        let code = r#"
export function internalHelper(x) { return x * 2; }
export function usedFn(x) { return internalHelper(x) + 1; }
export function deadFn() { return "DEAD_FN_MARKER"; }
export const DEAD_CONST = "DEAD_CONST_MARKER";
"#;
        let mut used = BTreeSet::new();
        used.insert("usedFn".to_owned());

        let result = prune_unused_exports(
            code,
            Path::new("src/math.js"),
            &ImportedSymbols::Named(used),
        )
        .expect("tree shaking should succeed");

        assert!(result.code.contains("function usedFn"));
        assert!(result.code.contains("function internalHelper"));
        assert!(!result.code.contains("DEAD_FN_MARKER"));
        assert!(!result.code.contains("DEAD_CONST_MARKER"));
        assert_eq!(result.removed_exports, vec!["deadFn", "DEAD_CONST"]);
    }

    #[test]
    fn compiles_css_modules_with_scoped_class_names() {
        let pipeline = TransformPipeline::default();
        let out = pipeline
            .transform(&TransformInput {
                path: PathBuf::from("src/card.module.css"),
                kind: ModuleKind::Css,
                source: ".card { padding: 1rem; } .title { font-weight: 700; }".to_owned(),
                framework: Framework::React,
                is_dev: true,
                minify_css: true,
            })
            .expect("css module transform should succeed");

        let exports = out
            .css_exports
            .expect("css module should produce exports map");
        let scoped_card = exports.get("card").expect("card class should be exported");
        assert_ne!(scoped_card, "card");
        assert!(out.code.contains(scoped_card));

        let hmr =
            wrap_css_with_exports_as_hmr_module("/src/card.module.css", &out.code, Some(&exports));
        assert!(hmr.contains(&format!("\"card\":\"{scoped_card}\"")));
    }

    #[test]
    fn emits_react_fast_refresh_registrations_in_dev_mode() {
        let pipeline = TransformPipeline::default();
        let src = "export function Counter() { return <button>Hi</button>; }\n";

        let dev_out = pipeline
            .transform(&TransformInput {
                path: PathBuf::from("src/Counter.tsx"),
                kind: ModuleKind::Tsx,
                source: src.to_owned(),
                framework: Framework::React,
                is_dev: true,
                minify_css: false,
            })
            .expect("dev react transform");
        assert!(
            dev_out.code.contains("$RefreshReg$"),
            "dev React TSX should emit Fast Refresh $RefreshReg$ registration"
        );

        let prod_out = pipeline
            .transform(&TransformInput {
                path: PathBuf::from("src/Counter.tsx"),
                kind: ModuleKind::Tsx,
                source: src.to_owned(),
                framework: Framework::React,
                is_dev: false,
                minify_css: false,
            })
            .expect("prod react transform");
        assert!(!prod_out.code.contains("$RefreshReg$"));
    }
}
