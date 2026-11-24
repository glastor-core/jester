//! Native Zero-Node Test Runner (`zerho test`) powered by `oxc`.
//!
//! Discovers `*.test.{ts,tsx,js,jsx}` and `*.spec.{ts,tsx,js,jsx}` files, compiles
//! TypeScript/JSX natively via `zerho-transform`, and evaluates `describe` / `test` /
//! `it` + `expect(...)` assertions in 100% Safe Rust without Vitest or Node.js.

use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Argument, BinaryOperator, BindingPattern, CallExpression, Expression, MemberExpression,
    Statement, UnaryOperator,
};
use oxc::ast_visit::Visit;
use oxc::parser::Parser;
use oxc::span::SourceType;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use zerho_core::{Framework, ModuleKind, ZerhoError, ZerhoResult};
use zerho_plugin::PluginRegistry;
use zerho_transform::{TransformInput, TransformPipeline};

#[derive(Debug, Clone, PartialEq)]
enum JsVal {
    Null,
    Undefined,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Self>),
}

impl JsVal {
    fn is_truthy(&self) -> bool {
        match self {
            Self::Null | Self::Undefined => false,
            Self::Bool(b) => *b,
            Self::Num(n) => *n != 0.0 && !n.is_nan(),
            Self::Str(s) => !s.is_empty(),
            Self::Arr(_) => true,
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
        }
    }
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

        let file_results = execute_compiled_test_module(&rel, &compiled.code)?;
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

fn execute_compiled_test_module(file_label: &str, code: &str) -> ZerhoResult<Vec<TestOutcome>> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, code, SourceType::mjs()).parse();
    if parsed.panicked {
        return Err(ZerhoError::Transform {
            path: PathBuf::from(file_label),
            reason: "failed to parse compiled test module".to_owned(),
        });
    }

    let mut env: HashMap<String, JsVal> = HashMap::new();
    let mut results = Vec::new();
    eval_statements(&parsed.program.body, "", &mut env, &mut results);
    Ok(results)
}

fn eval_statements(
    stmts: &[Statement<'_>],
    suite_prefix: &str,
    env: &mut HashMap<String, JsVal>,
    results: &mut Vec<TestOutcome>,
) {
    for stmt in stmts {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    if let BindingPattern::BindingIdentifier(ident) = &d.id
                        && let Some(init) = &d.init
                        && let Ok(val) = eval_expr(init, env)
                    {
                        env.insert(ident.name.to_string(), val);
                    }
                }
            }
            Statement::ExpressionStatement(expr_stmt) => {
                if let Expression::CallExpression(call) = &expr_stmt.expression {
                    handle_test_call(call, suite_prefix, env, results);
                }
            }
            _ => {}
        }
    }
}

struct FunctionBodyFinder<'a, 'b> {
    suite_prefix: &'b str,
    is_suite: bool,
    env: HashMap<String, JsVal>,
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
                self.suite_prefix,
                &mut self.env,
                self.results,
            );
        } else {
            self.test_outcome = Some(run_test_body(&body.statements, &mut self.env));
        }
    }
}

fn handle_test_call(
    call: &CallExpression<'_>,
    suite_prefix: &str,
    env: &mut HashMap<String, JsVal>,
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
    let label = match eval_expr(first_arg, env) {
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
        suite_prefix: &full_name,
        is_suite,
        env: env.clone(),
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

fn run_test_body(stmts: &[Statement<'_>], env: &mut HashMap<String, JsVal>) -> Result<(), String> {
    for stmt in stmts {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    if let BindingPattern::BindingIdentifier(ident) = &d.id
                        && let Some(init) = &d.init
                    {
                        let val = eval_expr(init, env)?;
                        env.insert(ident.name.to_string(), val);
                    }
                }
            }
            Statement::ExpressionStatement(expr_stmt) => {
                if let Expression::CallExpression(call) = &expr_stmt.expression {
                    eval_expect_call(call, env)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn eval_expect_call(call: &CallExpression<'_>, env: &HashMap<String, JsVal>) -> Result<(), String> {
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
    let actual = eval_expr(actual_expr, env)?;
    let expected = call
        .arguments
        .first()
        .and_then(Argument::as_expression)
        .map(|e| eval_expr(e, env))
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
fn eval_expr(expr: &Expression<'_>, env: &HashMap<String, JsVal>) -> Result<JsVal, String> {
    match expr {
        Expression::NumericLiteral(n) => Ok(JsVal::Num(n.value)),
        Expression::StringLiteral(s) => Ok(JsVal::Str(s.value.to_string())),
        Expression::BooleanLiteral(b) => Ok(JsVal::Bool(b.value)),
        Expression::NullLiteral(_) => Ok(JsVal::Null),
        Expression::Identifier(id) => {
            if id.name.as_str() == "undefined" {
                Ok(JsVal::Undefined)
            } else {
                env.get(id.name.as_str())
                    .cloned()
                    .ok_or_else(|| format!("undefined identifier `{}`", id.name))
            }
        }
        Expression::ParenthesizedExpression(p) => eval_expr(&p.expression, env),
        Expression::ArrayExpression(arr) => {
            let mut items = Vec::with_capacity(arr.elements.len());
            for el in &arr.elements {
                if let Some(e) = el.as_expression() {
                    items.push(eval_expr(e, env)?);
                }
            }
            Ok(JsVal::Arr(items))
        }
        Expression::TemplateLiteral(tpl) => {
            let mut out = String::new();
            for (i, q) in tpl.quasis.iter().enumerate() {
                out.push_str(q.value.raw.as_str());
                if let Some(e) = tpl.expressions.get(i) {
                    let v = eval_expr(e, env)?;
                    match v {
                        JsVal::Str(s) => out.push_str(&s),
                        other => out.push_str(&other.display()),
                    }
                }
            }
            Ok(JsVal::Str(out))
        }
        Expression::UnaryExpression(u) => {
            let v = eval_expr(&u.argument, env)?;
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
            let left = eval_expr(&bin.left, env)?;
            let right = eval_expr(&bin.right, env)?;
            eval_binary(bin.operator, &left, &right)
        }
        Expression::StaticMemberExpression(member) => {
            let obj = eval_expr(&member.object, env)?;
            if member.property.name.as_str() == "length" {
                match obj {
                    JsVal::Str(s) => Ok(JsVal::Num(f64::from(
                        u32::try_from(s.len()).unwrap_or(u32::MAX),
                    ))),
                    JsVal::Arr(a) => Ok(JsVal::Num(f64::from(
                        u32::try_from(a.len()).unwrap_or(u32::MAX),
                    ))),
                    _ => Err("property `length` not found".to_owned()),
                }
            } else {
                Err(format!("unsupported property `.{}`", member.property.name))
            }
        }
        Expression::ArrowFunctionExpression(_) => Ok(JsVal::Undefined),
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
}
