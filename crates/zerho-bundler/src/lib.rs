use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use zerho_config::{SourceMapMode, ZerhoConfig, build_import_meta_env_json, load_env_vars};
use zerho_core::{ContentHash, ModuleId, ModuleKind, ZerhoError, ZerhoResult};
use zerho_graph::ModuleGraph;
use zerho_plugin::{DefinePlugin, PluginRegistry};
use zerho_resolver::{OxcResolver, Resolver};
use zerho_transform::{
    ImportedSymbols, TransformInput, TransformPipeline, collect_dynamic_imports,
    collect_imported_symbols, encode_base64, finalize_js_bundle_with_map, is_likely_commonjs,
    lower_esm_for_bundle, prune_unused_exports, rewrite_dynamic_imports,
    serialize_css_exports_json, wrap_cjs_as_esm,
};

/// Emitted production chunk metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub file_name: String,
    pub hash: ContentHash,
    pub bytes: usize,
    pub sourcemap_file: Option<String>,
}

/// Result of a production build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleOutput {
    pub out_dir: PathBuf,
    pub chunks: Vec<Chunk>,
    pub module_count: usize,
    pub tree_shaken_exports: usize,
}

#[derive(Debug)]
struct CollectedModule {
    id: String,
    canonical: PathBuf,
    kind: ModuleKind,
    code: String,
    css_exports: Option<BTreeMap<String, String>>,
    emitted_asset_url: Option<String>,
}

/// Production bundler powered by `oxc_resolver`, `oxc`, `lightningcss`, and `blake3`,
/// supporting CJS→ESM pre-bundling, dynamic `import()` code splitting, and Source Maps v3.
#[derive(Debug)]
pub struct Bundler {
    config: ZerhoConfig,
    root: PathBuf,
    graph: ModuleGraph,
    resolver: OxcResolver,
    pipeline: TransformPipeline,
}

impl Bundler {
    pub fn new(config: ZerhoConfig) -> Self {
        let root = config
            .project
            .root
            .canonicalize()
            .unwrap_or_else(|_| config.project.root.clone());
        let resolver = OxcResolver::new(&root);

        let env_vars = load_env_vars(&root, "production", &config.project.env_prefix);
        let env_json = build_import_meta_env_json("production", false, &env_vars);
        let mut defines = BTreeMap::new();
        defines.insert("import.meta.env".to_owned(), env_json);
        let mut registry = PluginRegistry::new();
        registry.register(DefinePlugin::new(defines));

        Self {
            config,
            root,
            graph: ModuleGraph::new(),
            resolver,
            pipeline: TransformPipeline::new(registry),
        }
    }

    pub fn graph(&self) -> &ModuleGraph {
        &self.graph
    }

    /// Walks the dependency graph from `project.entry`, emits main + split dynamic chunks,
    /// source maps, minified CSS, `index.html`, and `manifest.json` into `build.out_dir`.
    pub fn build(&mut self) -> ZerhoResult<BundleOutput> {
        self.config.validate()?;

        let entry_path = self.root.join(&self.config.project.entry);
        let entry_canon = entry_path.canonicalize().map_err(|err| ZerhoError::Io {
            path: Some(entry_path.clone()),
            reason: format!("entry file not found: {err}"),
        })?;

        let out_dir = if self.config.build.out_dir.is_absolute() {
            self.config.build.out_dir.clone()
        } else {
            self.root.join(&self.config.build.out_dir)
        };
        let assets_dir = out_dir.join("assets");
        fs::create_dir_all(&assets_dir).map_err(|err| ZerhoError::Io {
            path: Some(assets_dir.clone()),
            reason: err.to_string(),
        })?;

        let mut visited = HashSet::new();
        let mut collected = Vec::new();
        let mut dynamic_targets: Vec<PathBuf> = Vec::new();
        let mut usage_map: HashMap<PathBuf, ImportedSymbols> = HashMap::new();
        let mut asset_chunks: Vec<Chunk> = Vec::new();
        usage_map.insert(entry_canon.clone(), ImportedSymbols::All);

        self.collect_module(
            &entry_canon,
            &out_dir,
            &mut visited,
            &mut collected,
            &mut dynamic_targets,
            &mut usage_map,
            &mut asset_chunks,
        )?;

        let mut tree_shaken_exports = self.shake_and_lower_modules(&mut collected, &usage_map)?;

        let mut chunks = Vec::new();
        let (split_chunk_files, dynamic_url_map, split_shaken) =
            self.emit_split_chunks(&out_dir, dynamic_targets, &mut chunks)?;
        tree_shaken_exports += split_shaken;
        chunks.extend(asset_chunks);

        // Rewrite dynamic `import(...)` specifiers in entry modules to point to emitted split chunks.
        if !dynamic_url_map.is_empty() {
            for module in &mut collected {
                if module.kind.is_script() {
                    let importer = module.canonical.clone();
                    module.code = rewrite_dynamic_imports(&module.code, &importer, |spec| {
                        let resolved = self.resolver.resolve(spec, Some(&importer)).ok()?;
                        let canon = resolved.path.canonicalize().unwrap_or(resolved.path);
                        dynamic_url_map.get(&canon).cloned()
                    })?;
                }
            }
        }

        // 2. Assemble, minify, and emit the entry JS bundle (+ source map).
        let entry_id = self.module_key(&entry_canon);
        let raw_js = assemble_js_bundle(&collected, &entry_id);
        let entry_chunk = self.emit_js_chunk(&out_dir, "index", &raw_js)?;
        let js_file = entry_chunk.file_name.clone();
        chunks.insert(0, entry_chunk);

        // 3. Emit CSS, copy public/ static assets, production index.html, and manifest.json.
        let css_file = emit_css_bundle(&out_dir, &collected, &mut chunks)?;
        let public_dir = self.root.join(&self.config.project.public_dir);
        if public_dir.is_dir() {
            copy_dir_recursive(&public_dir, &out_dir)?;
        }
        self.emit_html_and_manifest(
            &out_dir,
            &js_file,
            css_file.as_deref(),
            &split_chunk_files,
            collected.len(),
            tree_shaken_exports,
        )?;

        Ok(BundleOutput {
            out_dir,
            chunks,
            module_count: collected.len(),
            tree_shaken_exports,
        })
    }

    fn shake_and_lower_modules(
        &self,
        collected: &mut [CollectedModule],
        usage_map: &HashMap<PathBuf, ImportedSymbols>,
    ) -> ZerhoResult<usize> {
        let mut shaken_count = 0;
        for module in collected {
            if !module.kind.is_script() {
                continue;
            }
            let used = usage_map
                .get(&module.canonical)
                .unwrap_or(&ImportedSymbols::All);
            let shaken = prune_unused_exports(&module.code, &module.canonical, used)?;
            shaken_count += shaken.removed_exports.len();

            let importer = module.canonical.clone();
            module.code = lower_esm_for_bundle(&shaken.code, &importer, |spec| {
                let resolved = self.resolver.resolve(spec, Some(&importer)).ok()?;
                if resolved.is_virtual {
                    return None;
                }
                if resolved.kind == ModuleKind::Css && !is_css_module_path(&resolved.path) {
                    return None;
                }
                let target = resolved.path.canonicalize().unwrap_or(resolved.path);
                Some(self.module_key(&target))
            })?;
        }
        Ok(shaken_count)
    }

    fn emit_split_chunks(
        &mut self,
        out_dir: &Path,
        dynamic_targets: Vec<PathBuf>,
        chunks: &mut Vec<Chunk>,
    ) -> ZerhoResult<(Vec<String>, HashMap<PathBuf, String>, usize)> {
        let mut split_chunk_files = Vec::new();
        let mut dynamic_url_map = HashMap::new();
        let mut total_shaken = 0;
        if !self.config.build.splitting {
            return Ok((split_chunk_files, dynamic_url_map, total_shaken));
        }

        let mut processed_dynamic = HashSet::new();
        for target in dynamic_targets {
            if !processed_dynamic.insert(target.clone()) {
                continue;
            }
            let mut chunk_visited = HashSet::new();
            let mut chunk_modules = Vec::new();
            let mut nested_dynamic = Vec::new();
            let mut chunk_usage = HashMap::new();
            chunk_usage.insert(target.clone(), ImportedSymbols::All);

            self.collect_module(
                &target,
                out_dir,
                &mut chunk_visited,
                &mut chunk_modules,
                &mut nested_dynamic,
                &mut chunk_usage,
                chunks,
            )?;
            total_shaken += self.shake_and_lower_modules(&mut chunk_modules, &chunk_usage)?;

            let target_id = self.module_key(&target);
            let raw_chunk = assemble_async_esm_chunk(&chunk_modules, &target_id);
            let emitted = self.emit_js_chunk(out_dir, "chunk", &raw_chunk)?;
            dynamic_url_map.insert(target, format!("/{}", emitted.file_name));
            split_chunk_files.push(emitted.file_name.clone());
            chunks.push(emitted);
        }
        Ok((split_chunk_files, dynamic_url_map, total_shaken))
    }

    fn emit_html_and_manifest(
        &self,
        out_dir: &Path,
        js_file: &str,
        css_file: Option<&str>,
        split_chunk_files: &[String],
        module_count: usize,
        tree_shaken_exports: usize,
    ) -> ZerhoResult<()> {
        let src_html = self.root.join("index.html");
        if src_html.is_file() {
            let html = fs::read_to_string(&src_html).map_err(|err| ZerhoError::Io {
                path: Some(src_html),
                reason: err.to_string(),
            })?;
            let prod_html =
                rewrite_html_for_production(&html, &self.config.project.entry, js_file, css_file);
            let out_html = out_dir.join("index.html");
            fs::write(&out_html, prod_html).map_err(|err| ZerhoError::Io {
                path: Some(out_html),
                reason: err.to_string(),
            })?;
        }

        let css_json = css_file.map_or_else(|| "[]".to_owned(), |f| format!("[\"{f}\"]"));
        let split_json = format!(
            "[{}]",
            split_chunk_files
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let manifest = format!(
            "{{\n  \"entry\": \"{}\",\n  \"file\": \"{js_file}\",\n  \"css\": {css_json},\n  \"chunks\": {split_json},\n  \"sourcemap\": \"{}\",\n  \"modules\": {module_count},\n  \"tree_shaken_exports\": {tree_shaken_exports}\n}}\n",
            self.config.project.entry.to_string_lossy().replace('\\', "/"),
            self.config.build.sourcemap.as_str(),
        );
        let manifest_path = out_dir.join("manifest.json");
        fs::write(&manifest_path, manifest).map_err(|err| ZerhoError::Io {
            path: Some(manifest_path),
            reason: err.to_string(),
        })
    }

    fn emit_js_chunk(&self, out_dir: &Path, prefix: &str, raw_js: &str) -> ZerhoResult<Chunk> {
        let emit_map = self.config.build.sourcemap != SourceMapMode::None;
        let (mut final_js, map_json) =
            finalize_js_bundle_with_map(raw_js, &format!("{prefix}.js"), self.config.build.minify, emit_map)?;

        let js_hash = ContentHash::from_bytes(final_js.as_bytes());
        let file_name = format!("assets/{prefix}-{}.js", js_hash.short_hex());
        let mut sourcemap_file = None;

        if let Some(map) = map_json {
            match self.config.build.sourcemap {
                SourceMapMode::Linked => {
                    let map_file = format!("{file_name}.map");
                    let map_basename = Path::new(&map_file)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("bundle.js.map");
                    let map_path = out_dir.join(&map_file);
                    fs::write(&map_path, map).map_err(|err| ZerhoError::Io {
                        path: Some(map_path),
                        reason: err.to_string(),
                    })?;
                    let _ = write!(final_js, "\n//# sourceMappingURL={map_basename}\n");
                    sourcemap_file = Some(map_file);
                }
                SourceMapMode::Hidden => {
                    let map_file = format!("{file_name}.map");
                    let map_path = out_dir.join(&map_file);
                    fs::write(&map_path, map).map_err(|err| ZerhoError::Io {
                        path: Some(map_path),
                        reason: err.to_string(),
                    })?;
                    sourcemap_file = Some(map_file);
                }
                SourceMapMode::Inline => {
                    let b64 = encode_base64(map.as_bytes());
                    let _ = write!(
                        final_js,
                        "\n//# sourceMappingURL=data:application/json;base64,{b64}\n"
                    );
                }
                SourceMapMode::None => {}
            }
        }

        let js_path = out_dir.join(&file_name);
        fs::write(&js_path, &final_js).map_err(|err| ZerhoError::Io {
            path: Some(js_path),
            reason: err.to_string(),
        })?;

        Ok(Chunk {
            file_name,
            hash: js_hash,
            bytes: final_js.len(),
            sourcemap_file,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_module(
        &mut self,
        path: &Path,
        out_dir: &Path,
        visited: &mut HashSet<PathBuf>,
        collected: &mut Vec<CollectedModule>,
        dynamic_targets: &mut Vec<PathBuf>,
        usage_map: &mut HashMap<PathBuf, ImportedSymbols>,
        asset_chunks: &mut Vec<Chunk>,
    ) -> ZerhoResult<()> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !visited.insert(canonical.clone()) {
            return Ok(());
        }

        let kind = ModuleKind::from_path(&canonical);
        if kind == ModuleKind::Asset {
            let module = self.emit_imported_asset(&canonical, out_dir, asset_chunks)?;
            collected.push(module);
            return Ok(());
        }

        let raw_source = fs::read_to_string(&canonical).map_err(|err| ZerhoError::Io {
            path: Some(canonical.clone()),
            reason: err.to_string(),
        })?;

        // Automatic CJS -> ESM pre-bundling for CommonJS dependencies.
        let source = if kind == ModuleKind::Js && is_likely_commonjs(&raw_source) {
            wrap_cjs_as_esm(&raw_source)
        } else {
            raw_source
        };

        let transformed = self.pipeline.transform(&TransformInput {
            path: canonical.clone(),
            kind,
            source,
            framework: self.config.project.framework,
            is_dev: false,
            minify_css: self.config.css.minify,
        })?;

        if kind.is_script() && self.config.build.splitting {
            for dyn_spec in collect_dynamic_imports(&transformed.code, &canonical)? {
                if let Ok(resolved) = self.resolver.resolve(&dyn_spec, Some(&canonical))
                    && !resolved.is_virtual
                {
                    let dyn_canon = resolved.path.canonicalize().unwrap_or(resolved.path);
                    dynamic_targets.push(dyn_canon);
                }
            }
        }

        // Record symbol-level import usage for Pass 2 tree-shaking.
        if kind.is_script() {
            for (spec, symbols) in collect_imported_symbols(&transformed.code, &canonical)? {
                if let Ok(resolved) = self.resolver.resolve(&spec, Some(&canonical))
                    && !resolved.is_virtual
                {
                    let dep_canon = resolved.path.canonicalize().unwrap_or(resolved.path);
                    usage_map
                        .entry(dep_canon)
                        .and_modify(|existing| existing.merge(symbols.clone()))
                        .or_insert(symbols);
                }
            }
        }

        let mut dep_paths = Vec::new();
        let mut dep_ids = Vec::new();
        for spec in &transformed.imports {
            if let Ok(resolved) = self.resolver.resolve(spec, Some(&canonical))
                && !resolved.is_virtual
            {
                let dep_canon = resolved.path.canonicalize().unwrap_or(resolved.path);
                dep_ids.push(ModuleId::from_specifier(&dep_canon.to_string_lossy()));
                dep_paths.push(dep_canon);
            }
        }

        let mod_id = ModuleId::from_specifier(&canonical.to_string_lossy());
        self.graph
            .upsert_module(mod_id, canonical.clone(), kind, transformed.hash, dep_ids);

        // Depth-first visit dependencies first (post-order).
        for dep in dep_paths {
            self.collect_module(
                &dep,
                out_dir,
                visited,
                collected,
                dynamic_targets,
                usage_map,
                asset_chunks,
            )?;
        }

        let id = self.module_key(&canonical);
        collected.push(CollectedModule {
            id,
            canonical,
            kind,
            code: transformed.code,
            css_exports: transformed.css_exports,
            emitted_asset_url: None,
        });
        Ok(())
    }

    fn emit_imported_asset(
        &self,
        canonical: &Path,
        out_dir: &Path,
        asset_chunks: &mut Vec<Chunk>,
    ) -> ZerhoResult<CollectedModule> {
        let bytes = fs::read(canonical).map_err(|err| ZerhoError::Io {
            path: Some(canonical.to_path_buf()),
            reason: err.to_string(),
        })?;
        let hash = ContentHash::from_bytes(&bytes);
        let stem = canonical
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("asset");
        let ext = canonical
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin");
        let file_name = format!("assets/{stem}-{}.{ext}", hash.short_hex());
        let dest = out_dir.join(&file_name);
        fs::write(&dest, &bytes).map_err(|err| ZerhoError::Io {
            path: Some(dest),
            reason: err.to_string(),
        })?;
        asset_chunks.push(Chunk {
            file_name: file_name.clone(),
            hash,
            bytes: bytes.len(),
            sourcemap_file: None,
        });
        Ok(CollectedModule {
            id: self.module_key(canonical),
            canonical: canonical.to_path_buf(),
            kind: ModuleKind::Asset,
            code: String::new(),
            css_exports: None,
            emitted_asset_url: Some(format!("/{file_name}")),
        })
    }

    fn module_key(&self, canonical: &Path) -> String {
        canonical
            .strip_prefix(&self.root)
            .map_or_else(
                |_| canonical.to_string_lossy().replace('\\', "/"),
                |rel| rel.to_string_lossy().replace('\\', "/"),
            )
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> ZerhoResult<()> {
    fs::create_dir_all(dst).map_err(|err| ZerhoError::Io {
        path: Some(dst.to_path_buf()),
        reason: err.to_string(),
    })?;
    let entries = fs::read_dir(src).map_err(|err| ZerhoError::Io {
        path: Some(src.to_path_buf()),
        reason: err.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| ZerhoError::Io {
            path: Some(src.to_path_buf()),
            reason: err.to_string(),
        })?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|err| ZerhoError::Io {
                path: Some(to),
                reason: err.to_string(),
            })?;
        }
    }
    Ok(())
}

fn emit_css_bundle(
    out_dir: &Path,
    collected: &[CollectedModule],
    chunks: &mut Vec<Chunk>,
) -> ZerhoResult<Option<String>> {
    let mut css_parts = Vec::new();
    for module in collected {
        if module.kind == ModuleKind::Css && !module.code.trim().is_empty() {
            css_parts.push(module.code.as_str());
        }
    }
    if css_parts.is_empty() {
        return Ok(None);
    }
    let combined_css = css_parts.join("\n");
    let css_hash = ContentHash::from_bytes(combined_css.as_bytes());
    let file_name = format!("assets/index-{}.css", css_hash.short_hex());
    let css_path = out_dir.join(&file_name);
    fs::write(&css_path, &combined_css).map_err(|err| ZerhoError::Io {
        path: Some(css_path),
        reason: err.to_string(),
    })?;
    chunks.push(Chunk {
        file_name: file_name.clone(),
        hash: css_hash,
        bytes: combined_css.len(),
        sourcemap_file: None,
    });
    Ok(Some(file_name))
}

fn is_css_module_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.ends_with(".module.css"))
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
        out.push_str("\": function(__zerho_require, __zerho_exports) {\n    __zerho_exports.default = ");
        out.push_str(&json);
        out.push_str(";\n  },\n");
    } else if let Some(asset_url) = &module.emitted_asset_url {
        out.push_str("  \"");
        out.push_str(&module.id);
        out.push_str("\": function(__zerho_require, __zerho_exports) {\n    __zerho_exports.default = \"");
        out.push_str(asset_url);
        out.push_str("\";\n  },\n");
    }
}

fn assemble_js_bundle(modules: &[CollectedModule], entry_id: &str) -> String {
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
           return mod.exports;\n\
         }\n\
         __zerho_require(\"",
    );
    out.push_str(entry_id);
    out.push_str("\");\n})();\n");
    out
}

/// Emits a split async ESM chunk loadable directly via native browser `import("/assets/chunk-<hash>.js")`.
fn assemble_async_esm_chunk(modules: &[CollectedModule], entry_id: &str) -> String {
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
           if (!factory) throw new Error(\"[zerho] missing chunk module: \" + id);\n\
           const mod = { exports: {} };\n\
           __zerho_cache[id] = mod;\n\
           factory(__zerho_require, mod.exports);\n\
           return mod.exports;\n\
         }\n\
         const __zerho_chunk_exports = __zerho_require(\"",
    );
    out.push_str(entry_id);
    out.push_str("\");\nexport default __zerho_chunk_exports.default ?? __zerho_chunk_exports;\n");
    out
}

fn rewrite_html_for_production(
    html: &str,
    entry: &Path,
    js_file: &str,
    css_file: Option<&str>,
) -> String {
    let entry_web = format!("/{}", entry.to_string_lossy().replace('\\', "/"));
    let mut out = html.replace(
        &format!("src=\"{entry_web}\""),
        &format!("src=\"/{js_file}\""),
    );

    if let Some(css) = css_file {
        let link = format!("  <link rel=\"stylesheet\" href=\"/{css}\" />\n");
        if let Some(pos) = out.find("</head>") {
            out.insert_str(pos, &link);
        } else {
            out.push_str(&link);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_html_entry_and_injects_css_link() {
        let html = "<!doctype html><html><head><title>App</title></head><body><script type=\"module\" src=\"/src/main.ts\"></script></body></html>";
        let prod = rewrite_html_for_production(
            html,
            Path::new("src/main.ts"),
            "assets/index-abcd1234.js",
            Some("assets/index-9999aaaa.css"),
        );
        assert!(prod.contains("src=\"/assets/index-abcd1234.js\""));
        assert!(prod.contains("href=\"/assets/index-9999aaaa.css\""));
        assert!(!prod.contains("/src/main.ts"));
    }

    #[test]
    fn bundles_css_modules_and_hashed_svg_asset_imports() {
        let dir = std::env::temp_dir().join("zerho-bundler-css-mod-asset-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src dir");

        fs::write(
            dir.join("src/card.module.css"),
            ".panel { padding: 12px; }",
        )
        .expect("write css module");
        fs::write(
            dir.join("src/icon.svg"),
            "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
        )
        .expect("write svg asset");
        fs::write(
            dir.join("src/main.ts"),
            "import styles from './card.module.css';\n\
             import iconUrl from './icon.svg';\n\
             console.log(styles.panel, iconUrl);\n",
        )
        .expect("write entry");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/main.ts");
        cfg.build.minify = false;

        let mut bundler = Bundler::new(cfg);
        let out = bundler.build().expect("bundle should succeed");

        let has_svg_chunk = out.chunks.iter().any(|c| {
            c.file_name.starts_with("assets/icon-")
                && Path::new(&c.file_name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
        });
        assert!(has_svg_chunk, "imported svg should be emitted as hashed asset");

        let entry_js = fs::read_to_string(out.out_dir.join(&out.chunks[0].file_name))
            .expect("read entry js");
        assert!(entry_js.contains("/assets/icon-"));
        assert!(entry_js.contains("\"panel\":"));

        let _ = fs::remove_dir_all(&dir);
    }
}
