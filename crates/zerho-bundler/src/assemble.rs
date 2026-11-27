use crate::CollectedModule;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use zerho_core::{ModuleKind, ZerhoResult};
use zerho_transform::{
    HoistedModuleInput, ImportedSymbols, prune_unused_exports, serialize_css_exports_json,
};

/// Returns `true` when the nearest ancestor `package.json` declares `"sideEffects": false`
/// or `"sideEffects": []`, allowing aggressive dead-module elimination during tree-shaking.
pub(crate) fn is_side_effect_free(path: &Path) -> bool {
    let mut current = path.parent();
    while let Some(dir) = current {
        let pkg = dir.join("package.json");
        if pkg.is_file() {
            if let Ok(raw) = fs::read_to_string(&pkg) {
                return package_json_declares_no_side_effects(&raw);
            }
            return false;
        }
        current = dir.parent();
    }
    false
}

fn package_json_declares_no_side_effects(raw: &str) -> bool {
    let Some(idx) = raw.find("\"sideEffects\"") else {
        return false;
    };
    let rest = raw[idx + "\"sideEffects\"".len()..].trim_start();
    let Some(after_colon) = rest.strip_prefix(':') else {
        return false;
    };
    let val = after_colon.trim_start();
    if val.starts_with("false") {
        return true;
    }
    if let Some(after_bracket) = val.strip_prefix('[')
        && after_bracket.trim_start().starts_with(']')
    {
        return true;
    }
    false
}

pub(crate) fn shake_modules(
    collected: &mut [CollectedModule],
    usage_map: &HashMap<PathBuf, ImportedSymbols>,
) -> ZerhoResult<usize> {
    let counts = collected
        .par_iter_mut()
        .map(|module| -> ZerhoResult<usize> {
            if !module.kind.is_script() {
                return Ok(0);
            }
            // CommonJS-prebundled modules expose all named properties via `export default __zerho_cjs_entry`,
            // so `default` must never be pruned when importers use named ESM syntax (`import { createRoot } from 'react-dom/client'`).
            if module.code.contains("__zerho_cjs_entry") {
                return Ok(0);
            }
            let used = usage_map
                .get(&module.canonical)
                .unwrap_or(&ImportedSymbols::All);
            let shaken = prune_unused_exports(&module.code, &module.canonical, used)?;
            let mut removed = shaken.removed_exports.len();
            module.code = shaken.code;

            // When the owning package declares `"sideEffects": false`, drop any remaining
            // top-level dead helper code if zero exports survive tree-shaking.
            if matches!(used, ImportedSymbols::Named(_))
                && is_side_effect_free(&module.canonical)
                && !module.code.contains("export ")
                && !module.code.contains("export{")
                && !module.code.trim().is_empty()
            {
                module.code.clear();
                if removed == 0 {
                    removed = 1;
                }
            }

            Ok(removed)
        })
        .collect::<ZerhoResult<Vec<usize>>>()?;
    Ok(counts.into_iter().sum())
}

pub(crate) fn build_hoist_inputs(collected: &[CollectedModule]) -> Vec<HoistedModuleInput> {
    let mut out = Vec::with_capacity(collected.len());
    for module in collected {
        if module.kind.is_script() || module.kind == ModuleKind::Json {
            out.push(HoistedModuleInput {
                id: module.id.clone(),
                code: module.code.clone(),
                synthetic_default_expr: None,
                resolved_imports: module.resolved_imports.clone(),
            });
        } else if let Some(exports) = &module.css_exports {
            out.push(HoistedModuleInput {
                id: module.id.clone(),
                code: String::new(),
                synthetic_default_expr: Some(serialize_css_exports_json(exports)),
                resolved_imports: BTreeMap::new(),
            });
        } else if let Some(asset_url) = &module.emitted_asset_url {
            let synthetic = if Path::new(asset_url)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("wasm"))
            {
                format!(
                    "async function(imports){{return WebAssembly.instantiateStreaming(fetch(\"{asset_url}\"),imports);}}"
                )
            } else {
                format!("\"{asset_url}\"")
            };
            out.push(HoistedModuleInput {
                id: module.id.clone(),
                code: String::new(),
                synthetic_default_expr: Some(synthetic),
                resolved_imports: BTreeMap::new(),
            });
        }
    }
    out
}

pub(crate) fn emit_post_order(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
    loaded: &mut HashMap<PathBuf, (CollectedModule, Vec<PathBuf>)>,
    out: &mut Vec<CollectedModule>,
) {
    if !visited.insert(path.to_path_buf()) {
        return;
    }
    let Some((module, deps)) = loaded.remove(path) else {
        return;
    };
    for dep in deps {
        emit_post_order(&dep, visited, loaded, out);
    }
    out.push(module);
}

fn push_module_factory(out: &mut String, module: &CollectedModule) {
    if module.kind.is_script() || module.kind == ModuleKind::Json {
        out.push_str("  \"");
        out.push_str(&module.id);
        out.push_str("\": function(__zerho_require, __zerho_exports) {\n");
        out.push_str(&module.code);
        out.push_str("\n  },\n");
    } else if let Some(exports) = &module.css_exports {
        let json = serialize_css_exports_json(exports);
        out.push_str("  \"");
        out.push_str(&module.id);
        out.push_str(
            "\": function(__zerho_require, __zerho_exports) {\n    __zerho_exports.default = ",
        );
        out.push_str(&json);
        out.push_str(";\n  },\n");
    } else if let Some(asset_url) = &module.emitted_asset_url {
        out.push_str("  \"");
        out.push_str(&module.id);
        if Path::new(asset_url)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("wasm"))
        {
            out.push_str(
                "\": function(__zerho_require, __zerho_exports) {\n    __zerho_exports.default = async function(imports) { return WebAssembly.instantiateStreaming(fetch(\"",
            );
            out.push_str(asset_url);
            out.push_str("\"), imports); };\n  },\n");
        } else {
            out.push_str(
                "\": function(__zerho_require, __zerho_exports) {\n    __zerho_exports.default = \"",
            );
            out.push_str(asset_url);
            out.push_str("\";\n  },\n");
        }
    }
}

pub(crate) fn assemble_js_bundle(modules: &[CollectedModule], entry_id: &str) -> String {
    let mut out = String::from(
        "(function(){\n\
         const __zerho_registry = {\n",
    );

    for module in modules {
        push_module_factory(&mut out, module);
    }

    out.push_str(
        "};\n\
         const __zerho_cache = {};\n\
         function __zerho_require(id) {\n\
           if (__zerho_cache[id]) return __zerho_cache[id].exports;\n\
           const factory = __zerho_registry[id];\n\
           if (!factory) throw new Error(\"[zerho] missing module: \" + id);\n\
           const mod = { exports: {} };\n\
           __zerho_cache[id] = mod;\n\
           factory(__zerho_require, mod.exports);\n\
           if (mod.exports.default && (typeof mod.exports.default === \"object\" || typeof mod.exports.default === \"function\") && Object.keys(mod.exports).length === 1) {\n\
             Object.assign(mod.exports, mod.exports.default);\n\
           }\n\
           return mod.exports;\n\
         }\n\
         __zerho_require(\"",
    );
    out.push_str(entry_id);
    out.push_str("\");\n})();\n");
    out
}

/// Emits a reusable Library Mode bundle in either ESM (`.mjs`) or `CommonJS` (`.cjs`) format.
pub(crate) fn assemble_library_bundle(
    modules: &[CollectedModule],
    entry_id: &str,
    is_esm: bool,
) -> String {
    let mut out = String::from("const __zerho_registry = {\n");
    for module in modules {
        push_module_factory(&mut out, module);
    }
    out.push_str(
        "};\n\
         const __zerho_cache = {};\n\
         function __zerho_require(id) {\n\
           if (__zerho_cache[id]) return __zerho_cache[id].exports;\n\
           const factory = __zerho_registry[id];\n\
           if (!factory) throw new Error(\"[zerho lib] missing module: \" + id);\n\
           const mod = { exports: {} };\n\
           __zerho_cache[id] = mod;\n\
           factory(__zerho_require, mod.exports);\n\
           if (mod.exports.default && (typeof mod.exports.default === \"object\" || typeof mod.exports.default === \"function\") && Object.keys(mod.exports).length === 1) {\n\
             Object.assign(mod.exports, mod.exports.default);\n\
           }\n\
           return mod.exports;\n\
         }\n\
         const __zerho_lib_exports = __zerho_require(\"",
    );
    out.push_str(entry_id);
    if is_esm {
        out.push_str("\");\nexport default __zerho_lib_exports.default ?? __zerho_lib_exports;\nexport const __exports = __zerho_lib_exports;\n");
    } else {
        out.push_str("\");\nmodule.exports = __zerho_lib_exports;\n");
    }
    out
}

/// Emits a deduplicated shared ESM chunk imported by multiple dynamic async chunks.
pub(crate) fn assemble_shared_esm_chunk(modules: &[CollectedModule]) -> String {
    let mut out = String::from("export const __zerho_shared = {\n");
    for module in modules {
        push_module_factory(&mut out, module);
    }
    out.push_str("};\n");
    out
}

/// Emits a split async ESM chunk loadable directly via native browser `import(...)`.
pub(crate) fn assemble_async_esm_chunk(
    modules: &[CollectedModule],
    entry_id: &str,
    shared_chunk_url: Option<&str>,
) -> String {
    let mut out = String::new();
    if let Some(shared_url) = shared_chunk_url {
        let _ = writeln!(out, "import {{ __zerho_shared }} from \"{shared_url}\";");
        out.push_str("const __zerho_registry = Object.assign({}, __zerho_shared, {\n");
    } else {
        out.push_str("const __zerho_registry = {\n");
    }
    for module in modules {
        push_module_factory(&mut out, module);
    }
    if shared_chunk_url.is_some() {
        out.push_str("});\n");
    } else {
        out.push_str("};\n");
    }
    out.push_str(
        "const __zerho_cache = {};\n\
         function __zerho_require(id) {\n\
           if (__zerho_cache[id]) return __zerho_cache[id].exports;\n\
           const factory = __zerho_registry[id];\n\
           if (!factory) throw new Error(\"[zerho] missing chunk module: \" + id);\n\
           const mod = { exports: {} };\n\
           __zerho_cache[id] = mod;\n\
           factory(__zerho_require, mod.exports);\n\
           if (mod.exports.default && (typeof mod.exports.default === \"object\" || typeof mod.exports.default === \"function\") && Object.keys(mod.exports).length === 1) {\n\
             Object.assign(mod.exports, mod.exports.default);\n\
           }\n\
           return mod.exports;\n\
         }\n\
         const __zerho_chunk_exports = __zerho_require(\"",
    );
    out.push_str(entry_id);
    out.push_str("\");\nexport default __zerho_chunk_exports.default ?? __zerho_chunk_exports;\n");
    out
}
