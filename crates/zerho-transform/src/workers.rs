use oxc::allocator::Allocator;
use oxc::ast::ast::{Argument, Expression, NewExpression};
use oxc::ast_visit::Visit;
use oxc::parser::Parser;
use oxc::span::{GetSpan, SourceType, Span};
use std::collections::HashSet;
use std::path::Path;
use zerho_core::{ZerhoError, ZerhoResult};

struct WorkerUrlCollector {
    /// `(full_new_url_span, specifier)` for every `new Worker(new URL('./w.ts', import.meta.url))`.
    entries: Vec<(Span, String)>,
}

impl<'a> Visit<'a> for WorkerUrlCollector {
    fn visit_new_expression(&mut self, expr: &NewExpression<'a>) {
        if let Expression::Identifier(callee) = &expr.callee
            && (callee.name == "Worker" || callee.name == "SharedWorker")
            && let Some(Argument::NewExpression(url_new)) = expr.arguments.first()
            && let Expression::Identifier(url_callee) = &url_new.callee
            && url_callee.name == "URL"
            && let Some(Argument::StringLiteral(spec_lit)) = url_new.arguments.first()
        {
            self.entries
                .push((url_new.span(), spec_lit.value.as_str().to_owned()));
        }
        oxc::ast_visit::walk::walk_new_expression(self, expr);
    }
}

/// Collects all statically analyzable Web Worker entry specifiers from
/// `new Worker(new URL('./worker.ts', import.meta.url))` expressions via `oxc`.
pub fn collect_web_worker_urls(js_code: &str, path: &Path) -> ZerhoResult<Vec<String>> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, js_code, SourceType::mjs()).parse();
    if !parsed.diagnostics.is_empty() {
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(ZerhoError::Transform {
            path: path.to_path_buf(),
            reason: messages.join("\n"),
        });
    }
    let mut collector = WorkerUrlCollector {
        entries: Vec::new(),
    };
    collector.visit_program(&parsed.program);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (_, spec) in collector.entries {
        if seen.insert(spec.clone()) {
            out.push(spec);
        }
    }
    Ok(out)
}

/// Rewrites `new Worker(new URL('./worker.ts', import.meta.url))` to point to the emitted
/// hashed worker bundle URL (e.g. `new Worker("/assets/worker-abcd1234.js")`).
pub fn rewrite_web_worker_urls(
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
    let mut collector = WorkerUrlCollector {
        entries: Vec::new(),
    };
    collector.visit_program(&parsed.program);

    let mut edits: Vec<(Span, String)> = Vec::new();
    for (url_span, spec) in collector.entries {
        if let Some(emitted_url) = rewriter(&spec) {
            let escaped = emitted_url.replace('\\', "\\\\").replace('"', "\\\"");
            edits.push((url_span, format!("\"{escaped}\"")));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_and_rewrites_web_worker_new_url_expressions() {
        let src = "const w = new Worker(new URL('./compute.worker.ts', import.meta.url), { type: 'module' });\n";
        let urls =
            collect_web_worker_urls(src, Path::new("src/main.ts")).expect("collect worker urls");
        assert_eq!(urls, vec!["./compute.worker.ts"]);

        let rewritten = rewrite_web_worker_urls(src, Path::new("src/main.ts"), |spec| {
            (spec == "./compute.worker.ts").then(|| "/assets/worker-deadbeef.js".to_owned())
        })
        .expect("rewrite worker urls");
        assert!(
            rewritten.contains("new Worker(\"/assets/worker-deadbeef.js\", { type: 'module' })")
        );
    }
}
