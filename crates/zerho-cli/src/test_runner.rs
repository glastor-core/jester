//! Native Zero-Node Test Runner (`zerho test`) powered by `oxc`.
//!
//! Discovers `*.test.{ts,tsx,js,jsx}` and `*.spec.{ts,tsx,js,jsx}` files, compiles
//! TypeScript/JSX natively via `zerho-transform`, resolves local module imports via
//! `zerho-resolver`, and evaluates `describe` / `test` / `it` + `expect(...)` assertions
//! (including objects, user-defined functions, and local imports) in 100% Safe Rust.

use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Argument, ArrowFunctionExpression, BinaryOperator, BindingPattern, CallExpression, Declaration,
    ExportDefaultDeclarationKind, Expression, Function, MemberExpression, ObjectPropertyKind,
    PropertyKey, Statement, UnaryOperator,
};
use oxc::ast_visit::Visit;
use oxc::parser::Parser;
use oxc::span::{GetSpan, SourceType};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use zerho_core::{Framework, ModuleKind, ZerhoError, ZerhoResult};
use zerho_plugin::PluginRegistry;
use zerho_resolver::{OxcResolver, Resolver};
use zerho_transform::{TransformInput, TransformPipeline};

const MAX_CALL_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
enum JsVal {
    Null,
    Undefined,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Self>),
    Obj(BTreeMap<String, Self>),
}

impl JsVal {
    fn is_truthy(&self) -> bool {
        match self {
            Self::Null | Self::Undefined => false,
            Self::Bool(b) => *b,
            Self::Num(n) => *n != 0.0 && !n.is_nan(),
            Self::Str(s) => !s.is_empty(),
            Self::Arr(_) | Self::Obj(_) => true,
        }
    }

    fn as_num(&self) -> Option<f64> {
        match self {
            Self::Num(n) => Some(*n),
            Self::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn display(&self) -> String {
        match self {
            Self::Null => "null".to_owned(),
            Self::Undefined => "undefined".to_owned(),
            Self::Bool(b) => b.to_string(),
            Self::Num(n) => {
                if n.fract() == 0.0 {
                    format!("{n:.0}")
                } else {
                    n.to_string()
                }
            }
            Self::Str(s) => format!("\"{s}\""),
            Self::Arr(items) => {
                let inner: Vec<String> = items.iter().map(Self::display).collect();
                format!("[{}]", inner.join(", "))
            }
            Self::Obj(map) => {
                let pairs: Vec<String> = map
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.display()))
                    .collect();
                format!("{{ {} }}", pairs.join(", "))
            }
        }
    }
}

#[derive(Debug, Clone)]
struct FnDef {
    params: Vec<String>,
    /// Standalone JS source wrapping the function body for safe re-parsing on call.
    wrapped_source: String,
    is_expr_body: bool,
    /// Captured module/lexical variables visible when the function was declared.
    closure_vars: HashMap<String, JsVal>,
}

#[derive(Debug, Clone, Default)]
struct EvalCtx {
    vars: HashMap<String, JsVal>,
    fns: HashMap<String, FnDef>,
    depth: usize,
}

#[derive(Debug, Clone, Default)]
struct ModuleExports {
    vars: HashMap<String, JsVal>,
    fns: HashMap<String, FnDef>,
}

/// Summary returned by [`run_native_tests`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRunSummary {
    pub files: usize,
    pub passed: usize,
    pub failed: usize,
    pub duration_ms: u128,
    pub failures: Vec<String>,
}

/// Discovers and executes all `*.test.*` / `*.spec.*` files under `root`.
pub fn run_native_tests(root: &Path, filter: Option<&str>) -> ZerhoResult<TestRunSummary> {
    let started = Instant::now();
    let mut files = Vec::new();
    collect_test_files(root, &mut files)?;
    files.sort();

    if let Some(substr) = filter {
        files.retain(|p| p.to_string_lossy().contains(substr));
    }

    let pipeline = TransformPipeline::new(PluginRegistry::new());
    let resolver = OxcResolver::new(root);
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut failures = Vec::new();

    for file in &files {
        let raw = fs::read_to_string(file).map_err(|err| ZerhoError::Io {
            path: Some(file.clone()),
            reason: err.to_string(),
        })?;
        let kind = ModuleKind::from_path(file);
        let compiled = pipeline.transform(&TransformInput {
            path: file.clone(),
            source: raw,
            kind,
            is_dev: false,
            framework: Framework::React,
            minify_css: false,
        })?;

        let rel = file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");

        let file_results =
            execute_compiled_test_module(&rel, file, &compiled.code, &resolver, &pipeline)?;
        for (name, outcome) in file_results {
            match outcome {
                Ok(()) => {
                    passed += 1;
                    println!("  PASS  {rel} :: {name}");
                }
                Err(msg) => {
                    failed += 1;
                    let line = format!("{rel} :: {name} — {msg}");
                    println!("  FAIL  {line}");
                    failures.push(line);
                }
            }
        }
    }

    Ok(TestRunSummary {
        files: files.len(),
        passed,
        failed,
        duration_ms: started.elapsed().as_millis(),
        failures,
    })
}

fn collect_test_files(dir: &Path, out: &mut Vec<PathBuf>) -> ZerhoResult<()> {
    let entries = fs::read_dir(dir).map_err(|err| ZerhoError::Io {
        path: Some(dir.to_path_buf()),
        reason: err.to_string(),
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if path.is_dir() {
            if matches!(
                name,
                "node_modules" | "dist" | "target" | ".git" | ".zerho" | ".astro"
            ) {
                continue;
            }
            collect_test_files(&path, out)?;
        } else if is_test_filename(name) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_test_filename(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        ".test.ts",
        ".test.tsx",
        ".test.js",
        ".test.jsx",
        ".spec.ts",
        ".spec.tsx",
        ".spec.js",
        ".spec.jsx",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

type TestOutcome = (String, Result<(), String>);

fn execute_compiled_test_module(
    file_label: &str,
    file_path: &Path,
    code: &str,
    resolver: &OxcResolver,
    pipeline: &TransformPipeline,
) -> ZerhoResult<Vec<TestOutcome>> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, code, SourceType::mjs()).parse();
    if parsed.panicked {
        return Err(ZerhoError::Transform {
            path: PathBuf::from(file_label),
            reason: "failed to parse compiled test module".to_owned(),
        });
    }

    let mut ctx = EvalCtx::default();
    load_local_imports(
        &parsed.program.body,
        file_path,
        resolver,
        pipeline,
        &mut ctx,
        0,
    );

    let mut results = Vec::new();
    eval_statements(&parsed.program.body, code, "", &mut ctx, &mut results);
    Ok(results)
}

fn load_local_imports(
    stmts: &[Statement<'_>],
    importer: &Path,
    resolver: &OxcResolver,
    pipeline: &TransformPipeline,
    ctx: &mut EvalCtx,
    depth: usize,
) {
    if depth >= 16 {
        return;
    }
    for stmt in stmts {
        let Statement::ImportDeclaration(decl) = stmt else {
            continue;
        };
        let spec = decl.source.value.as_str();
        let Ok(target) = resolver.resolve(spec, Some(importer)) else {
            continue;
        };
        if target.is_virtual || !target.kind.is_script() {
            continue;
        }
        let Ok(exports) = eval_module_exports(&target.path, resolver, pipeline, depth + 1) else {
            continue;
        };
        let Some(specifiers) = &decl.specifiers else {
            continue;
        };
        for s in specifiers {
            match s {
                oxc::ast::ast::ImportDeclarationSpecifier::ImportSpecifier(named) => {
                    let exported = named.imported.name().as_str();
                    let local = named.local.name.to_string();
                    if let Some(val) = exports.vars.get(exported) {
                        ctx.vars.insert(local.clone(), val.clone());
                    }
                    if let Some(f) = exports.fns.get(exported) {
                        ctx.fns.insert(local, f.clone());
                    }
                }
                oxc::ast::ast::ImportDeclarationSpecifier::ImportDefaultSpecifier(def) => {
                    let local = def.local.name.to_string();
                    if let Some(val) = exports.vars.get("default") {
                        ctx.vars.insert(local.clone(), val.clone());
                    }
                    if let Some(f) = exports.fns.get("default") {
                        ctx.fns.insert(local, f.clone());
                    }
                }
                oxc::ast::ast::ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => {}
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
fn eval_module_exports(
    path: &Path,
    resolver: &OxcResolver,
    pipeline: &TransformPipeline,
    depth: usize,
) -> ZerhoResult<ModuleExports> {
    let raw = fs::read_to_string(path).map_err(|err| ZerhoError::Io {
        path: Some(path.to_path_buf()),
        reason: err.to_string(),
    })?;
    let compiled = pipeline.transform(&TransformInput {
        path: path.to_path_buf(),
        source: raw,
        kind: ModuleKind::from_path(path),
        is_dev: false,
        framework: Framework::React,
        minify_css: false,
    })?;

    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &compiled.code, SourceType::mjs()).parse();
    if parsed.panicked {
        return Ok(ModuleExports::default());
    }

    let mut ctx = EvalCtx::default();
    load_local_imports(
        &parsed.program.body,
        path,
        resolver,
        pipeline,
        &mut ctx,
        depth,
    );

    let mut exports = ModuleExports::default();
    for stmt in &parsed.program.body {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                bind_var_decl(decl, &compiled.code, &mut ctx);
            }
            Statement::FunctionDeclaration(func) => {
                if let Some((name, mut def)) = extract_function_def(func, &compiled.code) {
                    def.closure_vars.clone_from(&ctx.vars);
                    ctx.fns.insert(name, def);
                }
            }
            Statement::ExportDeclaration(exp) => match &exp.declaration {
                Declaration::VariableDeclaration(vdecl) => {
                    bind_var_decl(vdecl, &compiled.code, &mut ctx);
                    for d in &vdecl.declarations {
                        if let BindingPattern::BindingIdentifier(ident) = &d.id {
                            let name = ident.name.to_string();
                            if let Some(v) = ctx.vars.get(&name) {
                                exports.vars.insert(name.clone(), v.clone());
                            }
                            if let Some(f) = ctx.fns.get(&name) {
                                exports.fns.insert(name, f.clone());
                            }
                        }
                    }
                }
                Declaration::FunctionDeclaration(func) => {
                    if let Some((name, mut def)) = extract_function_def(func, &compiled.code) {
                        def.closure_vars.clone_from(&ctx.vars);
                        ctx.fns.insert(name.clone(), def.clone());
                        exports.fns.insert(name, def);
                    }
                }
                _ => {}
            },
            Statement::ExportNamedDeclaration(exp) => {
                for spec in &exp.specifiers {
                    let local = spec.local.name().as_str();
                    let exported = spec.exported.name().to_string();
                    if let Some(v) = ctx.vars.get(local) {
                        exports.vars.insert(exported.clone(), v.clone());
                    }
                    if let Some(f) = ctx.fns.get(local) {
                        exports.fns.insert(exported, f.clone());
                    }
                }
            }
            Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
                    if let Some((_, mut def)) = extract_function_def(func, &compiled.code) {
                        def.closure_vars.clone_from(&ctx.vars);
                        exports.fns.insert("default".to_owned(), def);
                    } else if let Some(mut def) = extract_anon_function_def(func, &compiled.code) {
                        def.closure_vars.clone_from(&ctx.vars);
                        exports.fns.insert("default".to_owned(), def);
                    }
                }
                other => {
                    if let Some(expr) = other.as_expression() {
                        if let Expression::ArrowFunctionExpression(arrow) = expr {
                            if let Some(mut def) = extract_arrow_def(arrow, &compiled.code) {
                                def.closure_vars.clone_from(&ctx.vars);
                                exports.fns.insert("default".to_owned(), def);
                            }
                        } else if let Ok(val) = eval_expr(expr, &ctx) {
                            exports.vars.insert("default".to_owned(), val);
                        }
                    }
                }
            },
            _ => {}
        }
    }
    Ok(exports)
}

fn bind_var_decl(decl: &oxc::ast::ast::VariableDeclaration<'_>, source: &str, ctx: &mut EvalCtx) {
    for d in &decl.declarations {
        if let BindingPattern::BindingIdentifier(ident) = &d.id
            && let Some(init) = &d.init
        {
            if let Expression::ArrowFunctionExpression(arrow) = init {
                if let Some(def) = extract_arrow_def(arrow, source) {
                    ctx.fns.insert(ident.name.to_string(), def);
                }
            } else if let Expression::FunctionExpression(func) = init {
                if let Some(def) = extract_anon_function_def(func, source) {
                    ctx.fns.insert(ident.name.to_string(), def);
                }
            } else if let Ok(val) = eval_expr(init, ctx) {
                ctx.vars.insert(ident.name.to_string(), val);
            }
        }
    }
}

fn extract_param_names(params: &oxc::ast::ast::FormalParameters<'_>) -> Vec<String> {
    params
        .items
        .iter()
        .filter_map(|p| match &p.pattern {
            BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
            _ => None,
        })
        .collect()
}

fn extract_function_def(func: &Function<'_>, source: &str) -> Option<(String, FnDef)> {
    let name = func.id.as_ref()?.name.to_string();
    let def = extract_anon_function_def(func, source)?;
    Some((name, def))
}

fn extract_anon_function_def(func: &Function<'_>, source: &str) -> Option<FnDef> {
    let body = func.body.as_ref()?;
    let span = body.span();
    let slice = source.get(span.start as usize..span.end as usize)?;
    Some(FnDef {
        params: extract_param_names(&func.params),
        wrapped_source: format!("function __zerho_fn(){slice}"),
        is_expr_body: false,
        closure_vars: HashMap::new(),
    })
}

fn extract_arrow_def(arrow: &ArrowFunctionExpression<'_>, source: &str) -> Option<FnDef> {
    let span = arrow.body.span();
    let slice = source.get(span.start as usize..span.end as usize)?;
    if slice.trim_start().starts_with('{') {
        Some(FnDef {
            params: extract_param_names(&arrow.params),
            wrapped_source: format!("function __zerho_fn(){slice}"),
            is_expr_body: false,
            closure_vars: HashMap::new(),
        })
    } else {
        Some(FnDef {
            params: extract_param_names(&arrow.params),
            wrapped_source: format!("function __zerho_fn(){{ return ({slice}); }}"),
            is_expr_body: true,
            closure_vars: HashMap::new(),
        })
    }
}

fn eval_statements(
    stmts: &[Statement<'_>],
    source: &str,
    suite_prefix: &str,
    ctx: &mut EvalCtx,
    results: &mut Vec<TestOutcome>,
) {
    for stmt in stmts {
        match stmt {
            Statement::VariableDeclaration(decl) => bind_var_decl(decl, source, ctx),
            Statement::FunctionDeclaration(func) => {
                if let Some((name, def)) = extract_function_def(func, source) {
                    ctx.fns.insert(name, def);
                }
            }
            Statement::ExpressionStatement(expr_stmt) => {
                if let Expression::CallExpression(call) = &expr_stmt.expression {
                    handle_test_call(call, source, suite_prefix, ctx, results);
                }
            }
            _ => {}
        }
    }
}

struct FunctionBodyFinder<'a, 'b> {
    source: &'b str,
    suite_prefix: &'b str,
    is_suite: bool,
    ctx: EvalCtx,
    results: &'b mut Vec<TestOutcome>,
    test_outcome: Option<Result<(), String>>,
    handled: bool,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> Visit<'a> for FunctionBodyFinder<'a, '_> {
    fn visit_function_body(&mut self, body: &oxc::ast::ast::FunctionBody<'a>) {
        if self.handled {
            return;
        }
        self.handled = true;
        if self.is_suite {
            eval_statements(
                &body.statements,
                self.source,
                self.suite_prefix,
                &mut self.ctx,
                self.results,
            );
        } else {
            self.test_outcome = Some(run_test_body(&body.statements, self.source, &mut self.ctx));
        }
    }
}

fn handle_test_call(
    call: &CallExpression<'_>,
    source: &str,
    suite_prefix: &str,
    ctx: &EvalCtx,
    results: &mut Vec<TestOutcome>,
) {
    let Expression::Identifier(callee) = &call.callee else {
        return;
    };
    let callee_name = callee.name.as_str();
    if !matches!(callee_name, "describe" | "test" | "it") {
        return;
    }
    let Some(first_arg) = call.arguments.first().and_then(Argument::as_expression) else {
        return;
    };
    let label = match eval_expr(first_arg, ctx) {
        Ok(JsVal::Str(s)) => s,
        _ => "<unnamed>".to_owned(),
    };
    let full_name = if suite_prefix.is_empty() {
        label
    } else {
        format!("{suite_prefix} > {label}")
    };
    let Some(second_arg) = call.arguments.get(1).and_then(Argument::as_expression) else {
        return;
    };

    let is_suite = callee_name == "describe";
    let mut finder = FunctionBodyFinder {
        source,
        suite_prefix: &full_name,
        is_suite,
        ctx: ctx.clone(),
        results,
        test_outcome: None,
        handled: false,
        _marker: std::marker::PhantomData,
    };
    finder.visit_expression(second_arg);
    if !is_suite && let Some(outcome) = finder.test_outcome {
        results.push((full_name, outcome));
    }
}

fn run_test_body(stmts: &[Statement<'_>], source: &str, ctx: &mut EvalCtx) -> Result<(), String> {
    for stmt in stmts {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    if let BindingPattern::BindingIdentifier(ident) = &d.id
                        && let Some(init) = &d.init
                    {
                        if let Expression::ArrowFunctionExpression(arrow) = init {
                            if let Some(def) = extract_arrow_def(arrow, source) {
                                ctx.fns.insert(ident.name.to_string(), def);
                            }
                        } else if let Expression::FunctionExpression(func) = init {
                            if let Some(def) = extract_anon_function_def(func, source) {
                                ctx.fns.insert(ident.name.to_string(), def);
                            }
                        } else {
                            let val = eval_expr(init, ctx)?;
                            ctx.vars.insert(ident.name.to_string(), val);
                        }
                    }
                }
            }
            Statement::FunctionDeclaration(func) => {
                if let Some((name, def)) = extract_function_def(func, source) {
                    ctx.fns.insert(name, def);
                }
            }
            Statement::ExpressionStatement(expr_stmt) => {
                if let Expression::CallExpression(call) = &expr_stmt.expression {
                    eval_expect_call(call, ctx)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn eval_user_fn(def: &FnDef, args: &[JsVal], parent: &EvalCtx) -> Result<JsVal, String> {
    if parent.depth >= MAX_CALL_DEPTH {
        return Err("maximum recursion depth exceeded in native test evaluator".to_owned());
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &def.wrapped_source, SourceType::mjs()).parse();
    if parsed.panicked {
        return Err("failed to parse user function body".to_owned());
    }
    let Some(Statement::FunctionDeclaration(func)) = parsed.program.body.first() else {
        return Err("invalid user function wrapper".to_owned());
    };
    let Some(body) = &func.body else {
        return Ok(JsVal::Undefined);
    };

    let mut merged_vars = parent.vars.clone();
    for (k, v) in &def.closure_vars {
        merged_vars.entry(k.clone()).or_insert_with(|| v.clone());
    }
    let mut local = EvalCtx {
        vars: merged_vars,
        fns: parent.fns.clone(),
        depth: parent.depth + 1,
    };
    for (idx, param) in def.params.iter().enumerate() {
        let val = args.get(idx).cloned().unwrap_or(JsVal::Undefined);
        local.vars.insert(param.clone(), val);
    }

    let _ = def.is_expr_body;
    eval_fn_block(&body.statements, &def.wrapped_source, &mut local)
}

fn eval_fn_block(
    stmts: &[Statement<'_>],
    source: &str,
    ctx: &mut EvalCtx,
) -> Result<JsVal, String> {
    for stmt in stmts {
        match stmt {
            Statement::VariableDeclaration(decl) => bind_var_decl(decl, source, ctx),
            Statement::FunctionDeclaration(func) => {
                if let Some((name, def)) = extract_function_def(func, source) {
                    ctx.fns.insert(name, def);
                }
            }
            Statement::ReturnStatement(ret) => {
                return match &ret.argument {
                    Some(expr) => eval_expr(expr, ctx),
                    None => Ok(JsVal::Undefined),
                };
            }
            Statement::IfStatement(if_stmt) => {
                let cond = eval_expr(&if_stmt.test, ctx)?;
                let branch = if cond.is_truthy() {
                    Some(&if_stmt.consequent)
                } else {
                    if_stmt.alternate.as_ref()
                };
                if let Some(target) = branch {
                    match target {
                        Statement::BlockStatement(block) => {
                            let val = eval_fn_block(&block.body, source, ctx)?;
                            if val != JsVal::Undefined {
                                return Ok(val);
                            }
                        }
                        Statement::ReturnStatement(ret) => {
                            return match &ret.argument {
                                Some(expr) => eval_expr(expr, ctx),
                                None => Ok(JsVal::Undefined),
                            };
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(JsVal::Undefined)
}

fn eval_expect_call(call: &CallExpression<'_>, ctx: &EvalCtx) -> Result<(), String> {
    let Some(MemberExpression::StaticMemberExpression(member)) = call.callee.as_member_expression()
    else {
        return Ok(());
    };
    let matcher = member.property.name.as_str();

    let (expect_call, negated) = match &member.object {
        Expression::CallExpression(c) => (c.as_ref(), false),
        Expression::StaticMemberExpression(inner)
            if inner.property.name.as_str() == "not"
                && matches!(&inner.object, Expression::CallExpression(_)) =>
        {
            if let Expression::CallExpression(c) = &inner.object {
                (c.as_ref(), true)
            } else {
                return Ok(());
            }
        }
        _ => return Ok(()),
    };

    let Expression::Identifier(id) = &expect_call.callee else {
        return Ok(());
    };
    if id.name.as_str() != "expect" {
        return Ok(());
    }
    let Some(actual_expr) = expect_call
        .arguments
        .first()
        .and_then(Argument::as_expression)
    else {
        return Err("expect() requires an argument".to_owned());
    };
    let actual = eval_expr(actual_expr, ctx)?;
    let expected = call
        .arguments
        .first()
        .and_then(Argument::as_expression)
        .map(|e| eval_expr(e, ctx))
        .transpose()?;

    let raw_pass = match matcher {
        "toBe" | "toEqual" => expected.as_ref().is_some_and(|exp| &actual == exp),
        "toBeTruthy" => actual.is_truthy(),
        "toBeFalsy" => !actual.is_truthy(),
        "toContain" => match (&actual, expected.as_ref()) {
            (JsVal::Str(hay), Some(JsVal::Str(needle))) => hay.contains(needle.as_str()),
            (JsVal::Arr(items), Some(needle)) => items.contains(needle),
            _ => false,
        },
        "toBeGreaterThan" => match (actual.as_num(), expected.as_ref().and_then(JsVal::as_num)) {
            (Some(a), Some(b)) => a > b,
            _ => false,
        },
        "toBeLessThan" => match (actual.as_num(), expected.as_ref().and_then(JsVal::as_num)) {
            (Some(a), Some(b)) => a < b,
            _ => false,
        },
        other => {
            return Err(format!("unsupported matcher `.{other}()`"));
        }
    };

    let passed = if negated { !raw_pass } else { raw_pass };
    if passed {
        Ok(())
    } else {
        let not_label = if negated { "not." } else { "" };
        let exp_str = expected.map_or_else(String::new, |v| v.display());
        Err(format!(
            "expected {} {not_label}{matcher}({exp_str})",
            actual.display()
        ))
    }
}

#[allow(clippy::too_many_lines)]
fn eval_expr(expr: &Expression<'_>, ctx: &EvalCtx) -> Result<JsVal, String> {
    match expr {
        Expression::NumericLiteral(n) => Ok(JsVal::Num(n.value)),
        Expression::StringLiteral(s) => Ok(JsVal::Str(s.value.to_string())),
        Expression::BooleanLiteral(b) => Ok(JsVal::Bool(b.value)),
        Expression::NullLiteral(_) => Ok(JsVal::Null),
        Expression::Identifier(id) => {
            if id.name.as_str() == "undefined" {
                Ok(JsVal::Undefined)
            } else {
                ctx.vars
                    .get(id.name.as_str())
                    .cloned()
                    .ok_or_else(|| format!("undefined identifier `{}`", id.name))
            }
        }
        Expression::ParenthesizedExpression(p) => eval_expr(&p.expression, ctx),
        Expression::ArrayExpression(arr) => {
            let mut items = Vec::with_capacity(arr.elements.len());
            for el in &arr.elements {
                if let Some(e) = el.as_expression() {
                    items.push(eval_expr(e, ctx)?);
                }
            }
            Ok(JsVal::Arr(items))
        }
        Expression::ObjectExpression(obj) => {
            let mut map = BTreeMap::new();
            for prop in &obj.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop {
                    let key = match &p.key {
                        PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                        PropertyKey::StringLiteral(s) => s.value.to_string(),
                        PropertyKey::NumericLiteral(n) => n.value.to_string(),
                        _ => continue,
                    };
                    let val = eval_expr(&p.value, ctx)?;
                    map.insert(key, val);
                }
            }
            Ok(JsVal::Obj(map))
        }
        Expression::CallExpression(call) => {
            if let Expression::Identifier(callee) = &call.callee
                && let Some(def) = ctx.fns.get(callee.name.as_str())
            {
                let mut evaluated_args = Vec::with_capacity(call.arguments.len());
                for arg in &call.arguments {
                    if let Some(e) = arg.as_expression() {
                        evaluated_args.push(eval_expr(e, ctx)?);
                    }
                }
                return eval_user_fn(def, &evaluated_args, ctx);
            }
            Err("unsupported call expression in native test evaluator".to_owned())
        }
        Expression::ConditionalExpression(cond) => {
            let test = eval_expr(&cond.test, ctx)?;
            if test.is_truthy() {
                eval_expr(&cond.consequent, ctx)
            } else {
                eval_expr(&cond.alternate, ctx)
            }
        }
        Expression::TemplateLiteral(tpl) => {
            let mut out = String::new();
            for (i, q) in tpl.quasis.iter().enumerate() {
                out.push_str(q.value.raw.as_str());
                if let Some(e) = tpl.expressions.get(i) {
                    let v = eval_expr(e, ctx)?;
                    match v {
                        JsVal::Str(s) => out.push_str(&s),
                        other => out.push_str(&other.display()),
                    }
                }
            }
            Ok(JsVal::Str(out))
        }
        Expression::UnaryExpression(u) => {
            let v = eval_expr(&u.argument, ctx)?;
            match u.operator {
                UnaryOperator::LogicalNot => Ok(JsVal::Bool(!v.is_truthy())),
                UnaryOperator::UnaryNegation => v
                    .as_num()
                    .map(|n| JsVal::Num(-n))
                    .ok_or_else(|| "cannot negate non-number".to_owned()),
                UnaryOperator::UnaryPlus => v
                    .as_num()
                    .map(JsVal::Num)
                    .ok_or_else(|| "cannot coerce non-number".to_owned()),
                _ => Err("unsupported unary operator".to_owned()),
            }
        }
        Expression::BinaryExpression(bin) => {
            let left = eval_expr(&bin.left, ctx)?;
            let right = eval_expr(&bin.right, ctx)?;
            eval_binary(bin.operator, &left, &right)
        }
        Expression::StaticMemberExpression(member) => {
            let obj = eval_expr(&member.object, ctx)?;
            let prop = member.property.name.as_str();
            match obj {
                JsVal::Obj(map) => Ok(map.get(prop).cloned().unwrap_or(JsVal::Undefined)),
                JsVal::Str(s) if prop == "length" => Ok(JsVal::Num(f64::from(
                    u32::try_from(s.len()).unwrap_or(u32::MAX),
                ))),
                JsVal::Arr(a) if prop == "length" => Ok(JsVal::Num(f64::from(
                    u32::try_from(a.len()).unwrap_or(u32::MAX),
                ))),
                _ => Err(format!("unsupported property `.{prop}`")),
            }
        }
        Expression::ComputedMemberExpression(member) => {
            let obj = eval_expr(&member.object, ctx)?;
            let key = eval_expr(&member.expression, ctx)?;
            match (obj, key) {
                (JsVal::Obj(map), JsVal::Str(k)) => {
                    Ok(map.get(&k).cloned().unwrap_or(JsVal::Undefined))
                }
                (JsVal::Arr(items), JsVal::Num(n)) if n >= 0.0 && n.fract() == 0.0 => {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let idx = n as usize;
                    Ok(items.get(idx).cloned().unwrap_or(JsVal::Undefined))
                }
                _ => Err("unsupported computed member access".to_owned()),
            }
        }
        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => {
            Ok(JsVal::Undefined)
        }
        _ => Err("unsupported expression in native test evaluator".to_owned()),
    }
}

fn eval_binary(op: BinaryOperator, left: &JsVal, right: &JsVal) -> Result<JsVal, String> {
    match op {
        BinaryOperator::Addition => match (left, right) {
            (JsVal::Str(a), JsVal::Str(b)) => Ok(JsVal::Str(format!("{a}{b}"))),
            (JsVal::Str(a), b) => Ok(JsVal::Str(format!("{a}{}", b.display()))),
            (a, JsVal::Str(b)) => Ok(JsVal::Str(format!("{}{b}", a.display()))),
            _ => match (left.as_num(), right.as_num()) {
                (Some(a), Some(b)) => Ok(JsVal::Num(a + b)),
                _ => Err("invalid `+` operands".to_owned()),
            },
        },
        BinaryOperator::Subtraction => num_bin(left, right, |a, b| a - b),
        BinaryOperator::Multiplication => num_bin(left, right, |a, b| a * b),
        BinaryOperator::Division => num_bin(left, right, |a, b| a / b),
        BinaryOperator::Remainder => num_bin(left, right, |a, b| a % b),
        BinaryOperator::StrictEquality | BinaryOperator::Equality => Ok(JsVal::Bool(left == right)),
        BinaryOperator::StrictInequality | BinaryOperator::Inequality => {
            Ok(JsVal::Bool(left != right))
        }
        BinaryOperator::LessThan => cmp_bin(left, right, |a, b| a < b),
        BinaryOperator::LessEqualThan => cmp_bin(left, right, |a, b| a <= b),
        BinaryOperator::GreaterThan => cmp_bin(left, right, |a, b| a > b),
        BinaryOperator::GreaterEqualThan => cmp_bin(left, right, |a, b| a >= b),
        _ => Err("unsupported binary operator".to_owned()),
    }
}

fn num_bin(left: &JsVal, right: &JsVal, f: impl FnOnce(f64, f64) -> f64) -> Result<JsVal, String> {
    match (left.as_num(), right.as_num()) {
        (Some(a), Some(b)) => Ok(JsVal::Num(f(a, b))),
        _ => Err("expected numeric operands".to_owned()),
    }
}

fn cmp_bin(left: &JsVal, right: &JsVal, f: impl FnOnce(f64, f64) -> bool) -> Result<JsVal, String> {
    match (left.as_num(), right.as_num()) {
        (Some(a), Some(b)) => Ok(JsVal::Bool(f(a, b))),
        _ => Err("expected numeric operands for comparison".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_native_typescript_tests_without_node_and_reports_failures() {
        let root = std::env::temp_dir().join(format!("zerho-native-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).expect("create src");

        fs::write(
            root.join("src/math.test.ts"),
            "const base: number = 20;\n\
             describe('math suite', () => {\n\
               test('adds numbers and checks template', () => {\n\
                 const sum: number = base + 22;\n\
                 expect(sum).toBe(42);\n\
                 expect(`ans=${sum}`).toContain('42');\n\
                 expect([1, 2, 3].length).toBeGreaterThan(2);\n\
               });\n\
               it('catches failing assertion', () => {\n\
                 expect(2 + 2).toBe(5);\n\
               });\n\
             });\n",
        )
        .expect("write test file");

        let summary = run_native_tests(&root, None).expect("run native tests");
        assert_eq!(summary.files, 1);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert!(summary.failures[0].contains("expected 4 toBe(5)"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn evaluates_local_imports_objects_and_user_functions() {
        let root =
            std::env::temp_dir().join(format!("zerho-native-test-imports-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).expect("create src");

        fs::write(
            root.join("src/utils.ts"),
            "export const OFFSET: number = 2;\n\
             export function add(a: number, b: number): number {\n\
               return a + b + OFFSET;\n\
             }\n\
             export const makeUser = (name: string) => ({ name, active: true });\n\
             export default function double(n: number): number { return n * 2; }\n",
        )
        .expect("write utils.ts");

        fs::write(
            root.join("src/utils.test.ts"),
            "import double, { add, makeUser } from './utils';\n\
             describe('local imports + objects + functions', () => {\n\
               test('calls imported TS functions and reads object properties', () => {\n\
                 const total = add(20, 20);\n\
                 expect(total).toBe(42);\n\
                 expect(double(21)).toBe(42);\n\
                 const user = makeUser('Ada');\n\
                 expect(user.name).toBe('Ada');\n\
                 expect(user).toEqual({ name: 'Ada', active: true });\n\
               });\n\
             });\n",
        )
        .expect("write utils.test.ts");

        let summary = run_native_tests(&root, None).expect("run native import tests");
        assert_eq!(summary.files, 1);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 0, "failures: {:?}", summary.failures);

        let _ = fs::remove_dir_all(&root);
    }
}
