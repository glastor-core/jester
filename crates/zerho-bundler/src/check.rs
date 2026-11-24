use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use rayon::prelude::*;
use zerho_config::{ZerhoConfig, load_env_vars};
use zerho_core::{ModuleId, ModuleKind, ZerhoError, ZerhoResult};
use zerho_graph::ModuleGraph;
use zerho_resolver::{OxcResolver, Resolver};
use zerho_transform::{TransformInput, TransformPipeline, collect_dynamic_imports};

use crate::security::scan_for_leaked_secrets;

/// Diagnostic severity emitted by `zerho check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckSeverity {
    Error,
    Warning,
}

impl CheckSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

/// A single actionable issue found by `zerho check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckIssue {
    pub severity: CheckSeverity,
    pub code: &'static str,
    pub file: String,
    pub message: String,
}

/// Result of running `zerho check` across the entire module dependency graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub modules_checked: usize,
    pub cycles: Vec<Vec<String>>,
    pub issues: Vec<CheckIssue>,
    pub elapsed_ms: u128,
}

impl CheckReport {
    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == CheckSeverity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == CheckSeverity::Warning)
            .count()
    }
}

struct CheckedNode {
    canonical: PathBuf,
    kind: ModuleKind,
    hash: zerho_core::ContentHash,
    dep_paths: Vec<PathBuf>,
    dep_ids: Vec<ModuleId>,
    issues: Vec<CheckIssue>,
}

/// Audits the full module graph in parallel (`rayon`) without Node.js or `tsc`:
/// - Broken static and dynamic imports
/// - Circular dependency chains (`A -> B -> A`)
/// - Missing `import.meta.env.<PREFIX>*` variables not declared in `.env*`
/// - Accidentally leaked credentials/secrets in client modules
pub fn run_project_check(config: &ZerhoConfig) -> ZerhoResult<CheckReport> {
    let start = Instant::now();
    config.validate()?;

    let root = config
        .project
        .root
        .canonicalize()
        .unwrap_or_else(|_| config.project.root.clone());
    let entry_path = root.join(&config.project.entry);
    let entry_canon = entry_path.canonicalize().map_err(|err| ZerhoError::Io {
        path: Some(entry_path),
        reason: format!("entry file not found: {err}"),
    })?;

    let resolver = OxcResolver::new(&root);
    let pipeline = TransformPipeline::default();
    let env_prefix = &config.project.env_prefix;
    let defined_env = load_env_vars(&root, "production", env_prefix);
    let dev_env = load_env_vars(&root, "development", env_prefix);
    let mut known_env_keys: BTreeSet<String> = defined_env.into_keys().collect();
    known_env_keys.extend(dev_env.into_keys());

    let mut graph = ModuleGraph::new();
    let mut discovered: HashSet<PathBuf> = HashSet::new();
    let mut frontier: Vec<PathBuf> = vec![entry_canon.clone()];
    discovered.insert(entry_canon);

    let mut issues: Vec<CheckIssue> = Vec::new();
    let mut modules_checked = 0usize;

    while !frontier.is_empty() {
        let batch: Vec<CheckedNode> = frontier
            .par_iter()
            .map(|canonical| {
                check_single_module(
                    canonical,
                    &root,
                    config,
                    &resolver,
                    &pipeline,
                    env_prefix,
                    &known_env_keys,
                )
            })
            .collect();

        let mut next_frontier = Vec::new();
        for node in batch {
            modules_checked += 1;
            issues.extend(node.issues);
            let mod_id = ModuleId::from_specifier(&node.canonical.to_string_lossy());
            graph.upsert_module(mod_id, node.canonical, node.kind, node.hash, node.dep_ids);
            for dep in node.dep_paths {
                if discovered.insert(dep.clone()) {
                    next_frontier.push(dep);
                }
            }
        }
        frontier = next_frontier;
    }

    let raw_cycles = graph.find_cycles();
    let mut cycles = Vec::with_capacity(raw_cycles.len());
    for chain in raw_cycles {
        let rel_chain: Vec<String> = chain.iter().map(|p| rel_display(&root, p)).collect();
        let formatted = rel_chain.join(" -> ");
        let first_file = rel_chain.first().cloned().unwrap_or_default();
        issues.push(CheckIssue {
            severity: CheckSeverity::Warning,
            code: "circular-dependency",
            file: first_file,
            message: format!("circular dependency detected: {formatted}"),
        });
        cycles.push(rel_chain);
    }

    Ok(CheckReport {
        modules_checked,
        cycles,
        issues,
        elapsed_ms: start.elapsed().as_millis(),
    })
}

#[allow(clippy::too_many_lines)]
fn check_single_module(
    canonical: &Path,
    root: &Path,
    config: &ZerhoConfig,
    resolver: &OxcResolver,
    pipeline: &TransformPipeline,
    env_prefix: &str,
    known_env_keys: &BTreeSet<String>,
) -> CheckedNode {
    let rel = rel_display(root, canonical);
    let kind = ModuleKind::from_path(canonical);
    let mut issues = Vec::new();

    if kind == ModuleKind::Asset {
        return CheckedNode {
            canonical: canonical.to_path_buf(),
            kind,
            hash: zerho_core::ContentHash::from_bytes(rel.as_bytes()),
            dep_paths: Vec::new(),
            dep_ids: Vec::new(),
            issues,
        };
    }

    let Ok(raw_source) = fs::read_to_string(canonical) else {
        issues.push(CheckIssue {
            severity: CheckSeverity::Error,
            code: "io-read",
            file: rel,
            message: "failed to read module source file".to_owned(),
        });
        return CheckedNode {
            canonical: canonical.to_path_buf(),
            kind,
            hash: zerho_core::ContentHash::from_bytes(b""),
            dep_paths: Vec::new(),
            dep_ids: Vec::new(),
            issues,
        };
    };

    if config.security.secret_scan
        && let Err(err) = scan_for_leaked_secrets(&rel, &raw_source)
    {
        issues.push(CheckIssue {
            severity: CheckSeverity::Error,
            code: "secret-leak",
            file: rel.clone(),
            message: err.to_string(),
        });
    }

    if kind.is_script() && !env_prefix.is_empty() {
        for var_name in extract_import_meta_env_refs(&raw_source, env_prefix) {
            if !known_env_keys.contains(&var_name) && std::env::var(&var_name).is_err() {
                issues.push(CheckIssue {
                    severity: CheckSeverity::Warning,
                    code: "missing-env",
                    file: rel.clone(),
                    message: format!(
                        "`import.meta.env.{var_name}` is referenced but not defined in `.env*`"
                    ),
                });
            }
        }
    }

    let transformed = match pipeline.transform(&TransformInput {
        path: canonical.to_path_buf(),
        kind,
        source: raw_source,
        framework: config.project.framework,
        is_dev: false,
        minify_css: false,
    }) {
        Ok(out) => out,
        Err(err) => {
            issues.push(CheckIssue {
                severity: CheckSeverity::Error,
                code: "transform-error",
                file: rel,
                message: err.to_string(),
            });
            return CheckedNode {
                canonical: canonical.to_path_buf(),
                kind,
                hash: zerho_core::ContentHash::from_bytes(b""),
                dep_paths: Vec::new(),
                dep_ids: Vec::new(),
                issues,
            };
        }
    };

    let mut dep_paths = Vec::new();
    let mut dep_ids = Vec::new();

    for spec in &transformed.imports {
        match resolver.resolve(spec, Some(canonical)) {
            Ok(target) if !target.is_virtual => {
                let dep_canon = target.path.canonicalize().unwrap_or(target.path);
                dep_ids.push(ModuleId::from_specifier(&dep_canon.to_string_lossy()));
                dep_paths.push(dep_canon);
            }
            Ok(_) => {}
            Err(_) => {
                issues.push(CheckIssue {
                    severity: CheckSeverity::Error,
                    code: "unresolved-import",
                    file: rel.clone(),
                    message: format!("could not resolve static import `{spec}`"),
                });
            }
        }
    }

    if kind.is_script()
        && let Ok(dyn_specs) = collect_dynamic_imports(&transformed.code, canonical)
    {
        for spec in dyn_specs {
            match resolver.resolve(&spec, Some(canonical)) {
                Ok(target) if !target.is_virtual => {
                    let dep_canon = target.path.canonicalize().unwrap_or(target.path);
                    dep_paths.push(dep_canon);
                }
                Ok(_) => {}
                Err(_) => {
                    issues.push(CheckIssue {
                        severity: CheckSeverity::Error,
                        code: "unresolved-dynamic-import",
                        file: rel.clone(),
                        message: format!("could not resolve dynamic import `{spec}`"),
                    });
                }
            }
        }
    }

    CheckedNode {
        canonical: canonical.to_path_buf(),
        kind,
        hash: transformed.hash,
        dep_paths,
        dep_ids,
        issues,
    }
}

fn extract_import_meta_env_refs(source: &str, env_prefix: &str) -> BTreeSet<String> {
    let needle = format!("import.meta.env.{env_prefix}");
    let mut vars = BTreeSet::new();
    let mut rest = source;
    while let Some(pos) = rest.find(&needle) {
        let after = &rest[pos + "import.meta.env.".len()..];
        let ident_len = after
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if ident_len > env_prefix.len() {
            vars.insert(after[..ident_len].to_owned());
        }
        rest = &after[ident_len..];
    }
    vars
}

fn rel_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cycles_broken_imports_and_missing_env_vars() {
        let dir = std::env::temp_dir().join("zerho-check-auditor-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src");

        fs::write(
            dir.join("src/a.ts"),
            "import { bVal } from './b';\nimport './missing-module';\nexport const aVal = bVal + import.meta.env.ZERHO_UNDECLARED_URL;\n",
        )
        .expect("write a.ts");
        fs::write(
            dir.join("src/b.ts"),
            "import { aVal } from './a';\nexport const bVal = aVal;\n",
        )
        .expect("write b.ts");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/a.ts");

        let report = run_project_check(&cfg).expect("check should complete");
        assert_eq!(report.modules_checked, 2);
        assert_eq!(report.cycles.len(), 1);
        assert!(report.issues.iter().any(|i| i.code == "circular-dependency"));
        assert!(report.issues.iter().any(|i| i.code == "unresolved-import"));
        assert!(report.issues.iter().any(|i| i.code == "missing-env"));
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.warning_count(), 2);

        let _ = fs::remove_dir_all(&dir);
    }
}
