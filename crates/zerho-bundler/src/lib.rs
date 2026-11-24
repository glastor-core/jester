pub mod check;
pub mod images;
pub mod security;
pub mod stats;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use rayon::prelude::*;
pub use check::{CheckIssue, CheckReport, CheckSeverity, run_project_check};
use security::{
    build_csp_manifest, compress_brotli, compute_gzip_size, compute_sri_sha384, enforce_budgets,
    scan_for_leaked_secrets,
};
use stats::{ModuleStat, emit_bundle_stats_html};
use zerho_config::{SourceMapMode, ZerhoConfig, build_import_meta_env_json, load_env_vars};
use zerho_core::{ContentHash, ModuleId, ModuleKind, ZerhoError, ZerhoResult};
use zerho_graph::{CachedArtifact, DiskCache, ModuleGraph};
use zerho_plugin::{DefinePlugin, PluginRegistry, register_wasm_plugins};
use zerho_resolver::{OxcResolver, Resolver};
use zerho_transform::{
    HoistedModuleInput, ImportedSymbols, TransformInput, TransformPipeline,
    collect_dynamic_imports, collect_imported_symbols, encode_base64, finalize_js_bundle_with_map,
    is_likely_commonjs, lower_esm_for_bundle, optimize_svg, parse_pure_barrel,
    prune_unused_exports, rewrite_barrel_imports, rewrite_dynamic_imports,
    serialize_css_exports_json, try_scope_hoist_bundle, wrap_cjs_as_esm,
};

/// Emitted production chunk metadata (including gzip/brotli sizes and optional SHA-384 SRI digest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub file_name: String,
    pub hash: ContentHash,
    pub bytes: usize,
    pub gzip_bytes: usize,
    pub brotli_bytes: usize,
    pub brotli_file: Option<String>,
    pub sourcemap_file: Option<String>,
    pub integrity: Option<String>,
}

/// Result of a production build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleOutput {
    pub out_dir: PathBuf,
    pub chunks: Vec<Chunk>,
    pub module_count: usize,
    pub tree_shaken_exports: usize,
    pub barrel_imports_bypassed: usize,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub scope_hoisted: bool,
    pub stats_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct CollectedModule {
    id: String,
    canonical: PathBuf,
    kind: ModuleKind,
    code: String,
    css_exports: Option<BTreeMap<String, String>>,
    emitted_asset_url: Option<String>,
    resolved_imports: BTreeMap<String, Option<String>>,
}

#[derive(Debug)]
struct LoadedNode {
    canonical: PathBuf,
    module: CollectedModule,
    hash: Option<ContentHash>,
    dep_paths: Vec<PathBuf>,
    dep_ids: Vec<ModuleId>,
    dynamic_deps: Vec<PathBuf>,
    imported_symbols: Vec<(PathBuf, ImportedSymbols)>,
    asset_chunk: Option<Chunk>,
    barrel_bypassed: usize,
}

struct SplitChunksResult {
    split_chunk_files: Vec<String>,
    preload_chunks: Vec<(String, Option<String>)>,
    dynamic_url_map: HashMap<PathBuf, String>,
    tree_shaken_exports: usize,
}

/// Production bundler powered by `oxc_resolver`, `oxc`, `oxc_minifier`, `lightningcss`,
/// `rayon`, `redb`, `brotli`, and `blake3`, supporting incremental build caching,
/// Scope Hoisting, Shared Chunks + `modulepreload`, CJS→ESM, and Source Maps v3.
#[derive(Debug)]
pub struct Bundler {
    config: ZerhoConfig,
    root: PathBuf,
    env_hash: ContentHash,
    graph: ModuleGraph,
    cache: DiskCache,
    resolver: OxcResolver,
    pipeline: TransformPipeline,
    plugin_init_error: Option<String>,
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
        let env_hash = ContentHash::from_bytes(env_json.as_bytes());
        let mut defines = BTreeMap::new();
        defines.insert("import.meta.env".to_owned(), env_json);
        let mut registry = PluginRegistry::new();
        registry.register(DefinePlugin::new(defines));

        let plugin_init_error = register_wasm_plugins(
            &mut registry,
            &root,
            &config.plugins.wasm,
            config.plugins.fuel_limit,
        )
        .err()
        .map(|err| err.to_string());

        let cache_dir = if config.cache.dir.is_absolute() {
            config.cache.dir.clone()
        } else {
            root.join(&config.cache.dir)
        };
        let cache = DiskCache::with_remote(
            cache_dir,
            config.cache.enabled,
            config.cache.remote_url.as_deref(),
            config.cache.remote_token.as_deref(),
        );

        Self {
            config,
            root,
            env_hash,
            graph: ModuleGraph::new(),
            cache,
            resolver,
            pipeline: TransformPipeline::new(registry),
            plugin_init_error,
        }
    }

    pub fn graph(&self) -> &ModuleGraph {
        &self.graph
    }

    /// Walks the dependency graph from `project.entry`, emits main + shared + split dynamic chunks,
    /// Brotli `.br` assets, source maps, minified CSS, `index.html` (with SRI + `modulepreload`),
    /// `manifest.json`, and `csp.json` into `build.out_dir`.
    #[allow(clippy::too_many_lines)]
    pub fn build(&mut self) -> ZerhoResult<BundleOutput> {
        self.config.validate()?;
        if let Some(reason) = &self.plugin_init_error {
            return Err(ZerhoError::Plugin {
                plugin: "wasm".to_owned(),
                reason: reason.clone(),
            });
        }

        if let Some(lib_entry) = self.config.build.lib.entry.clone() {
            return self.build_library(&lib_entry);
        }

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

        let mut collected = Vec::new();
        let mut dynamic_targets: Vec<PathBuf> = Vec::new();
        let mut usage_map: HashMap<PathBuf, ImportedSymbols> = HashMap::new();
        let mut asset_chunks: Vec<Chunk> = Vec::new();
        usage_map.insert(entry_canon.clone(), ImportedSymbols::All);

        let barrel_imports_bypassed = self.collect_module_graph(
            &entry_canon,
            &out_dir,
            &mut collected,
            &mut dynamic_targets,
            &mut usage_map,
            &mut asset_chunks,
        )?;

        let mut tree_shaken_exports = shake_modules(&mut collected, &usage_map)?;

        let mut chunks = Vec::new();
        let split_res = self.emit_split_chunks(&out_dir, dynamic_targets, &mut chunks)?;
        tree_shaken_exports += split_res.tree_shaken_exports;
        chunks.extend(asset_chunks);

        // Rewrite dynamic `import(...)` specifiers in entry modules in parallel.
        if !split_res.dynamic_url_map.is_empty() {
            collected.par_iter_mut().try_for_each(|module| -> ZerhoResult<()> {
                if module.kind.is_script() {
                    let importer = module.canonical.clone();
                    module.code = rewrite_dynamic_imports(&module.code, &importer, |spec| {
                        let resolved = self.resolver.resolve(spec, Some(&importer)).ok()?;
                        let canon = resolved.path.canonicalize().unwrap_or(resolved.path);
                        split_res.dynamic_url_map.get(&canon).cloned()
                    })?;
                }
                Ok(())
            })?;
        }

        // 2. Assemble via Scope Hoisting (default for acyclic ESM) or fall back to registry mode.
        let entry_id = self.module_key(&entry_canon);
        let (raw_js, scope_hoisted) = if self.config.build.scope_hoisting {
            let hoist_inputs = build_hoist_inputs(&collected);
            if let Some(hoisted) = try_scope_hoist_bundle(&hoist_inputs) {
                (hoisted, true)
            } else {
                self.lower_esm_modules(&mut collected)?;
                (assemble_js_bundle(&collected, &entry_id), false)
            }
        } else {
            self.lower_esm_modules(&mut collected)?;
            (assemble_js_bundle(&collected, &entry_id), false)
        };

        let entry_chunk = self.emit_js_chunk(&out_dir, "index", &raw_js)?;
        let js_file = entry_chunk.file_name.clone();
        let js_sri = entry_chunk.integrity.clone();
        chunks.insert(0, entry_chunk);

        // 3. Emit CSS, copy/optimize public/ static assets, enforce budgets, emit HTML/manifests.
        let css_emit = emit_css_bundle(
            &out_dir,
            &collected,
            self.config.security.sri,
            self.config.build.brotli,
            &mut chunks,
        )?;
        let public_dir = self.root.join(&self.config.project.public_dir);
        if public_dir.is_dir() {
            copy_dir_recursive(&public_dir, &out_dir, self.config.assets.optimize_svg)?;
        }

        enforce_budgets(&chunks, &self.config.build.budgets)?;

        if self.config.security.csp {
            let csp_path = out_dir.join("csp.json");
            fs::write(&csp_path, build_csp_manifest(&chunks)).map_err(|err| ZerhoError::Io {
                path: Some(csp_path),
                reason: err.to_string(),
            })?;
        }

        self.emit_html_and_manifest(
            &out_dir,
            &js_file,
            js_sri.as_deref(),
            css_emit.as_ref().map(|(f, _)| f.as_str()),
            css_emit.as_ref().and_then(|(_, sri)| sri.as_deref()),
            &split_res.split_chunk_files,
            &split_res.preload_chunks,
            collected.len(),
            tree_shaken_exports,
            scope_hoisted,
        )?;

        let stats_file = if self.config.build.analyze {
            let mod_stats: Vec<ModuleStat> = collected
                .iter()
                .map(|m| ModuleStat {
                    id: m.id.clone(),
                    raw_bytes: m.code.len(),
                    kind: format!("{:?}", m.kind).to_ascii_lowercase(),
                })
                .collect();
            Some(emit_bundle_stats_html(
                &out_dir,
                &self.config.project.name,
                &chunks,
                &mod_stats,
                tree_shaken_exports,
                barrel_imports_bypassed,
                scope_hoisted,
            )?)
        } else {
            None
        };

        let cache_stats = self.cache.stats();
        Ok(BundleOutput {
            out_dir,
            chunks,
            module_count: collected.len(),
            tree_shaken_exports,
            barrel_imports_bypassed,
            cache_hits: cache_stats.hits,
            cache_misses: cache_stats.misses,
            scope_hoisted,
            stats_file,
        })
    }

    fn lower_esm_modules(&self, collected: &mut [CollectedModule]) -> ZerhoResult<()> {
        collected.par_iter_mut().try_for_each(|module| -> ZerhoResult<()> {
            if !module.kind.is_script() {
                return Ok(());
            }
            let importer = module.canonical.clone();
            module.code = lower_esm_for_bundle(&module.code, &importer, |spec| {
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
            Ok(())
        })
    }

    fn emit_split_chunks(
        &mut self,
        out_dir: &Path,
        dynamic_targets: Vec<PathBuf>,
        chunks: &mut Vec<Chunk>,
    ) -> ZerhoResult<SplitChunksResult> {
        let mut split_chunk_files = Vec::new();
        let mut preload_chunks = Vec::new();
        let mut dynamic_url_map = HashMap::new();
        let mut total_shaken = 0;

        if !self.config.build.splitting || dynamic_targets.is_empty() {
            return Ok(SplitChunksResult {
                split_chunk_files,
                preload_chunks,
                dynamic_url_map,
                tree_shaken_exports: 0,
            });
        }

        // 1. Collect & tree-shake every dynamic target subgraph first to detect shared modules.
        let mut processed_dynamic = HashSet::new();
        let mut pending: Vec<(PathBuf, Vec<CollectedModule>)> = Vec::new();
        for target in dynamic_targets {
            if !processed_dynamic.insert(target.clone()) {
                continue;
            }
            let mut chunk_modules = Vec::new();
            let mut nested_dynamic = Vec::new();
            let mut chunk_usage = HashMap::new();
            chunk_usage.insert(target.clone(), ImportedSymbols::All);

            self.collect_module_graph(
                &target,
                out_dir,
                &mut chunk_modules,
                &mut nested_dynamic,
                &mut chunk_usage,
                chunks,
            )?;
            total_shaken += shake_modules(&mut chunk_modules, &chunk_usage)?;
            self.lower_esm_modules(&mut chunk_modules)?;
            pending.push((target, chunk_modules));
        }

        // 2. Extract modules referenced by >= 2 dynamic chunks into a deduplicated shared chunk.
        let mut id_counts: HashMap<String, usize> = HashMap::new();
        for (_, mods) in &pending {
            let mut seen_in_chunk = HashSet::new();
            for m in mods {
                if (m.kind.is_script() || m.kind == ModuleKind::Json || m.css_exports.is_some())
                    && seen_in_chunk.insert(m.id.clone())
                {
                    *id_counts.entry(m.id.clone()).or_insert(0) += 1;
                }
            }
        }

        let shared_ids: HashSet<String> = id_counts
            .into_iter()
            .filter_map(|(id, count)| (count >= 2).then_some(id))
            .collect();

        let shared_chunk_url = if shared_ids.is_empty() {
            None
        } else {
            let mut shared_modules = Vec::new();
            let mut seen = HashSet::new();
            for (_, mods) in &pending {
                for m in mods {
                    if shared_ids.contains(&m.id) && seen.insert(m.id.clone()) {
                        shared_modules.push(m.clone());
                    }
                }
            }
            let raw_shared = assemble_shared_esm_chunk(&shared_modules);
            let emitted = self.emit_js_chunk(out_dir, "shared", &raw_shared)?;
            let url = format!("/{}", emitted.file_name);
            preload_chunks.push((emitted.file_name.clone(), emitted.integrity.clone()));
            split_chunk_files.push(emitted.file_name.clone());
            chunks.push(emitted);
            Some(url)
        };

        // 3. Emit each dynamic async chunk, importing shared factories instead of duplicating them.
        for (target, mut chunk_modules) in pending {
            let uses_shared = shared_chunk_url.is_some()
                && chunk_modules.iter().any(|m| shared_ids.contains(&m.id));
            if uses_shared {
                chunk_modules.retain(|m| !shared_ids.contains(&m.id));
            }
            let target_id = self.module_key(&target);
            let raw_chunk = assemble_async_esm_chunk(
                &chunk_modules,
                &target_id,
                if uses_shared {
                    shared_chunk_url.as_deref()
                } else {
                    None
                },
            );
            let emitted = self.emit_js_chunk(out_dir, "chunk", &raw_chunk)?;
            dynamic_url_map.insert(target, format!("/{}", emitted.file_name));
            split_chunk_files.push(emitted.file_name.clone());
            chunks.push(emitted);
        }

        Ok(SplitChunksResult {
            split_chunk_files,
            preload_chunks,
            dynamic_url_map,
            tree_shaken_exports: total_shaken,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_html_and_manifest(
        &self,
        out_dir: &Path,
        js_file: &str,
        js_sri: Option<&str>,
        css_file: Option<&str>,
        css_sri: Option<&str>,
        split_chunk_files: &[String],
        preload_chunks: &[(String, Option<String>)],
        module_count: usize,
        tree_shaken_exports: usize,
        scope_hoisted: bool,
    ) -> ZerhoResult<()> {
        let src_html = self.root.join("index.html");
        if src_html.is_file() {
            let html = fs::read_to_string(&src_html).map_err(|err| ZerhoError::Io {
                path: Some(src_html),
                reason: err.to_string(),
            })?;
            let prod_html = rewrite_html_for_production(
                &html,
                &self.config.project.entry,
                js_file,
                js_sri,
                css_file,
                css_sri,
                preload_chunks,
            );
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
        let preload_json = format!(
            "[{}]",
            preload_chunks
                .iter()
                .map(|(f, _)| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let integrity_json = js_sri.map_or_else(|| "null".to_owned(), |s| format!("\"{s}\""));
        let manifest = format!(
            "{{\n  \"entry\": \"{}\",\n  \"file\": \"{js_file}\",\n  \"integrity\": {integrity_json},\n  \"css\": {css_json},\n  \"chunks\": {split_json},\n  \"preload\": {preload_json},\n  \"sourcemap\": \"{}\",\n  \"scope_hoisted\": {scope_hoisted},\n  \"modules\": {module_count},\n  \"tree_shaken_exports\": {tree_shaken_exports}\n}}\n",
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
        let (mut final_js, map_json) = finalize_js_bundle_with_map(
            raw_js,
            &format!("{prefix}.js"),
            self.config.build.minify,
            emit_map,
        )?;

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

        if self.config.security.secret_scan {
            scan_for_leaked_secrets(&file_name, &final_js)?;
        }

        let integrity = self
            .config
            .security
            .sri
            .then(|| compute_sri_sha384(final_js.as_bytes()));
        let gzip_bytes = compute_gzip_size(final_js.as_bytes());
        let br_buf = compress_brotli(final_js.as_bytes());
        let brotli_bytes = br_buf.len();

        let js_path = out_dir.join(&file_name);
        fs::write(&js_path, &final_js).map_err(|err| ZerhoError::Io {
            path: Some(js_path),
            reason: err.to_string(),
        })?;

        let brotli_file = if self.config.build.brotli {
            let br_name = format!("{file_name}.br");
            let br_path = out_dir.join(&br_name);
            fs::write(&br_path, &br_buf).map_err(|err| ZerhoError::Io {
                path: Some(br_path),
                reason: err.to_string(),
            })?;
            Some(br_name)
        } else {
            None
        };

        Ok(Chunk {
            file_name,
            hash: js_hash,
            bytes: final_js.len(),
            gzip_bytes,
            brotli_bytes,
            brotli_file,
            sourcemap_file,
            integrity,
        })
    }

    /// Discovers and transforms the module subgraph in parallel BFS waves via `rayon`
    /// backed by incremental `redb` + remote team cache, then emits modules in deterministic
    /// post-order DFS.
    fn collect_module_graph(
        &mut self,
        entry: &Path,
        out_dir: &Path,
        collected: &mut Vec<CollectedModule>,
        dynamic_targets: &mut Vec<PathBuf>,
        usage_map: &mut HashMap<PathBuf, ImportedSymbols>,
        asset_chunks: &mut Vec<Chunk>,
    ) -> ZerhoResult<usize> {
        let entry_canon = entry.canonicalize().unwrap_or_else(|_| entry.to_path_buf());
        let mut discovered: HashSet<PathBuf> = HashSet::new();
        let mut frontier: Vec<PathBuf> = vec![entry_canon.clone()];
        discovered.insert(entry_canon.clone());

        let mut loaded_map: HashMap<PathBuf, (CollectedModule, Vec<PathBuf>)> = HashMap::new();
        let mut total_barrel_bypassed = 0usize;

        while !frontier.is_empty() {
            let batch: Vec<LoadedNode> = frontier
                .par_iter()
                .map(|canonical| self.load_and_transform_module(canonical, out_dir))
                .collect::<ZerhoResult<Vec<LoadedNode>>>()?;

            let mut next_frontier = Vec::new();
            for node in batch {
                total_barrel_bypassed += node.barrel_bypassed;
                if let Some(chunk) = node.asset_chunk {
                    asset_chunks.push(chunk);
                }
                if let Some(hash) = node.hash {
                    let mod_id = ModuleId::from_specifier(&node.canonical.to_string_lossy());
                    self.graph.upsert_module(
                        mod_id,
                        node.canonical.clone(),
                        node.module.kind,
                        hash,
                        node.dep_ids,
                    );
                }
                dynamic_targets.extend(node.dynamic_deps);
                for (dep_canon, symbols) in node.imported_symbols {
                    usage_map
                        .entry(dep_canon)
                        .and_modify(|existing| existing.merge(symbols.clone()))
                        .or_insert(symbols);
                }
                for dep in &node.dep_paths {
                    if discovered.insert(dep.clone()) {
                        next_frontier.push(dep.clone());
                    }
                }
                loaded_map.insert(node.canonical, (node.module, node.dep_paths));
            }
            frontier = next_frontier;
        }

        let mut dfs_visited = HashSet::new();
        emit_post_order(&entry_canon, &mut dfs_visited, &mut loaded_map, collected);
        Ok(total_barrel_bypassed)
    }

    #[allow(clippy::too_many_lines)]
    fn load_and_transform_module(
        &self,
        canonical: &Path,
        out_dir: &Path,
    ) -> ZerhoResult<LoadedNode> {
        let kind = ModuleKind::from_path(canonical);
        if kind == ModuleKind::Asset {
            let (module, chunk) = self.emit_imported_asset(canonical, out_dir)?;
            return Ok(LoadedNode {
                canonical: canonical.to_path_buf(),
                module,
                hash: None,
                dep_paths: Vec::new(),
                dep_ids: Vec::new(),
                dynamic_deps: Vec::new(),
                imported_symbols: Vec::new(),
                asset_chunk: Some(chunk),
                barrel_bypassed: 0,
            });
        }

        let raw_source = fs::read_to_string(canonical).map_err(|err| ZerhoError::Io {
            path: Some(canonical.to_path_buf()),
            reason: err.to_string(),
        })?;

        // Automatic CJS -> ESM pre-bundling for CommonJS dependencies.
        let source = if kind == ModuleKind::Js && is_likely_commonjs(&raw_source) {
            wrap_cjs_as_esm(&raw_source)
        } else {
            raw_source
        };

        let cacheable = kind.is_script() || kind == ModuleKind::Json;
        let (mut code, mut imports, css_exports, hash) = if cacheable {
            let kind_tag = format!("prod-{kind:?}-{}", self.env_hash.short_hex());
            let key = DiskCache::compute_key(
                &source,
                &kind_tag,
                self.config.project.framework.as_str(),
                false,
                self.config.css.minify,
            );
            if let Some(hit) = self.cache.get(key) {
                (hit.code, hit.imports, None, hit.output_hash)
            } else {
                self.cache.record_miss();
                let transformed = self.pipeline.transform(&TransformInput {
                    path: canonical.to_path_buf(),
                    kind,
                    source,
                    framework: self.config.project.framework,
                    is_dev: false,
                    minify_css: self.config.css.minify,
                })?;
                let _ = self.cache.put(
                    key,
                    &CachedArtifact {
                        code: transformed.code.clone(),
                        imports: transformed.imports.clone(),
                        output_hash: transformed.hash,
                    },
                );
                (
                    transformed.code,
                    transformed.imports,
                    transformed.css_exports,
                    transformed.hash,
                )
            }
        } else {
            let transformed = self.pipeline.transform(&TransformInput {
                path: canonical.to_path_buf(),
                kind,
                source,
                framework: self.config.project.framework,
                is_dev: false,
                minify_css: self.config.css.minify,
            })?;
            (
                transformed.code,
                transformed.imports,
                transformed.css_exports,
                transformed.hash,
            )
        };

        // Barrel File Optimizer: rewrite named imports targeting pure re-export barrels directly to leaf modules.
        let mut barrel_bypassed = 0usize;
        if kind.is_script() {
            type BarrelEntry = Option<(PathBuf, BTreeMap<String, zerho_transform::BarrelExport>)>;
            let mut barrel_cache: HashMap<String, BarrelEntry> = HashMap::new();
            let (rewritten, bypassed) =
                rewrite_barrel_imports(&code, canonical, |barrel_spec, sym| {
                    let entry = barrel_cache
                        .entry(barrel_spec.to_owned())
                        .or_insert_with(|| {
                            let resolved = self.resolver.resolve(barrel_spec, Some(canonical)).ok()?;
                            if resolved.is_virtual || !resolved.kind.is_script() {
                                return None;
                            }
                            let barrel_canon = resolved.path.canonicalize().unwrap_or(resolved.path);
                            let src = fs::read_to_string(&barrel_canon).ok()?;
                            let map = parse_pure_barrel(&src, &barrel_canon)?;
                            Some((barrel_canon, map))
                        });
                    let (barrel_canon, map) = entry.as_ref()?;
                    let target = map.get(sym)?;
                    let leaf_resolved = self
                        .resolver
                        .resolve(&target.source_specifier, Some(barrel_canon))
                        .ok()?;
                    if leaf_resolved.is_virtual {
                        return None;
                    }
                    let leaf_canon = leaf_resolved.path.canonicalize().unwrap_or(leaf_resolved.path);
                    let leaf_web = format!("/{}", self.module_key(&leaf_canon));
                    Some((leaf_web, target.original_name.clone()))
                })?;
            if bypassed > 0 {
                code = rewritten;
                barrel_bypassed = bypassed;
                imports = collect_imported_symbols(&code, canonical)?
                    .into_iter()
                    .map(|(spec, _)| spec)
                    .collect();
            }
        }

        let mut dynamic_deps = Vec::new();
        if kind.is_script() && self.config.build.splitting {
            for dyn_spec in collect_dynamic_imports(&code, canonical)? {
                if let Ok(resolved) = self.resolver.resolve(&dyn_spec, Some(canonical))
                    && !resolved.is_virtual
                {
                    let dyn_canon = resolved.path.canonicalize().unwrap_or(resolved.path);
                    dynamic_deps.push(dyn_canon);
                }
            }
        }

        let mut imported_symbols = Vec::new();
        if kind.is_script() {
            for (spec, symbols) in collect_imported_symbols(&code, canonical)? {
                if let Ok(resolved) = self.resolver.resolve(&spec, Some(canonical))
                    && !resolved.is_virtual
                {
                    let dep_canon = resolved.path.canonicalize().unwrap_or(resolved.path);
                    imported_symbols.push((dep_canon, symbols));
                }
            }
        }

        let mut dep_paths = Vec::new();
        let mut dep_ids = Vec::new();
        let mut resolved_imports = BTreeMap::new();
        for spec in &imports {
            if let Ok(resolved) = self.resolver.resolve(spec, Some(canonical)) {
                if resolved.is_virtual {
                    resolved_imports.insert(spec.clone(), None);
                } else {
                    let dep_canon = resolved.path.canonicalize().unwrap_or(resolved.path);
                    dep_ids.push(ModuleId::from_specifier(&dep_canon.to_string_lossy()));
                    dep_paths.push(dep_canon.clone());
                    if resolved.kind == ModuleKind::Css && !is_css_module_path(&dep_canon) {
                        resolved_imports.insert(spec.clone(), None);
                    } else {
                        resolved_imports.insert(spec.clone(), Some(self.module_key(&dep_canon)));
                    }
                }
            }
        }

        let id = self.module_key(canonical);
        Ok(LoadedNode {
            canonical: canonical.to_path_buf(),
            module: CollectedModule {
                id,
                canonical: canonical.to_path_buf(),
                kind,
                code,
                css_exports,
                emitted_asset_url: None,
                resolved_imports,
            },
            hash: Some(hash),
            dep_paths,
            dep_ids,
            dynamic_deps,
            imported_symbols,
            asset_chunk: None,
            barrel_bypassed,
        })
    }

    fn emit_imported_asset(
        &self,
        canonical: &Path,
        out_dir: &Path,
    ) -> ZerhoResult<(CollectedModule, Chunk)> {
        let mut bytes = fs::read(canonical).map_err(|err| ZerhoError::Io {
            path: Some(canonical.to_path_buf()),
            reason: err.to_string(),
        })?;
        let mut ext = canonical
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
            .to_owned();
        if self.config.assets.optimize_svg
            && ext.eq_ignore_ascii_case("svg")
            && let Ok(text) = std::str::from_utf8(&bytes)
        {
            bytes = optimize_svg(text).into_bytes();
        } else if self.config.assets.optimize_images
            && (ext.eq_ignore_ascii_case("png")
                || ext.eq_ignore_ascii_case("jpg")
                || ext.eq_ignore_ascii_case("jpeg"))
            && let Some(webp) = images::optimize_image_to_webp(&bytes)
        {
            bytes = webp;
            "webp".clone_into(&mut ext);
        }
        let hash = ContentHash::from_bytes(&bytes);
        let stem = canonical
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("asset");
        let file_name = format!("assets/{stem}-{}.{ext}", hash.short_hex());
        let dest = out_dir.join(&file_name);
        fs::write(&dest, &bytes).map_err(|err| ZerhoError::Io {
            path: Some(dest),
            reason: err.to_string(),
        })?;
        let gzip_bytes = compute_gzip_size(&bytes);
        let brotli_bytes = compress_brotli(&bytes).len();
        let chunk = Chunk {
            file_name: file_name.clone(),
            hash,
            bytes: bytes.len(),
            gzip_bytes,
            brotli_bytes,
            brotli_file: None,
            sourcemap_file: None,
            integrity: None,
        };
        let module = CollectedModule {
            id: self.module_key(canonical),
            canonical: canonical.to_path_buf(),
            kind: ModuleKind::Asset,
            code: String::new(),
            css_exports: None,
            emitted_asset_url: Some(format!("/{file_name}")),
            resolved_imports: BTreeMap::new(),
        };
        Ok((module, chunk))
    }

    fn build_library(&mut self, lib_entry: &Path) -> ZerhoResult<BundleOutput> {
        let entry_path = self.root.join(lib_entry);
        let entry_canon = entry_path.canonicalize().map_err(|err| ZerhoError::Io {
            path: Some(entry_path),
            reason: format!("library entry file not found: {err}"),
        })?;

        let out_dir = if self.config.build.out_dir.is_absolute() {
            self.config.build.out_dir.clone()
        } else {
            self.root.join(&self.config.build.out_dir)
        };
        fs::create_dir_all(&out_dir).map_err(|err| ZerhoError::Io {
            path: Some(out_dir.clone()),
            reason: err.to_string(),
        })?;
        fs::create_dir_all(out_dir.join("assets")).map_err(|err| ZerhoError::Io {
            path: Some(out_dir.join("assets")),
            reason: err.to_string(),
        })?;

        let mut collected = Vec::new();
        let mut dynamic_targets = Vec::new();
        let mut usage_map = HashMap::new();
        let mut asset_chunks = Vec::new();
        usage_map.insert(entry_canon.clone(), ImportedSymbols::All);

        let barrel_imports_bypassed = self.collect_module_graph(
            &entry_canon,
            &out_dir,
            &mut collected,
            &mut dynamic_targets,
            &mut usage_map,
            &mut asset_chunks,
        )?;
        let tree_shaken_exports = shake_modules(&mut collected, &usage_map)?;
        self.lower_esm_modules(&mut collected)?;

        let entry_id = self.module_key(&entry_canon);
        let lib_name = self
            .config
            .build
            .lib
            .name
            .clone()
            .unwrap_or_else(|| self.config.project.name.clone());

        let mut chunks = Vec::new();
        for fmt in &self.config.build.lib.formats {
            let (file_name, raw_code) = match fmt.as_str() {
                "es" | "esm" => (
                    format!("{lib_name}.mjs"),
                    assemble_library_bundle(&collected, &entry_id, true),
                ),
                "cjs" => (
                    format!("{lib_name}.cjs"),
                    assemble_library_bundle(&collected, &entry_id, false),
                ),
                other => {
                    return Err(ZerhoError::Config(format!(
                        "unsupported `build.lib.formats` value `{other}` (expected: es, cjs)"
                    )));
                }
            };
            let chunk = self.emit_named_js_file(&out_dir, &file_name, &raw_code)?;
            chunks.push(chunk);
        }
        chunks.extend(asset_chunks);

        enforce_budgets(&chunks, &self.config.build.budgets)?;
        let cache_stats = self.cache.stats();
        Ok(BundleOutput {
            out_dir,
            chunks,
            module_count: collected.len(),
            tree_shaken_exports,
            barrel_imports_bypassed,
            cache_hits: cache_stats.hits,
            cache_misses: cache_stats.misses,
            scope_hoisted: false,
            stats_file: None,
        })
    }

    fn emit_named_js_file(
        &self,
        out_dir: &Path,
        file_name: &str,
        raw_js: &str,
    ) -> ZerhoResult<Chunk> {
        let emit_map = self.config.build.sourcemap != SourceMapMode::None;
        let (mut final_js, map_json) =
            finalize_js_bundle_with_map(raw_js, file_name, self.config.build.minify, emit_map)?;
        let hash = ContentHash::from_bytes(final_js.as_bytes());
        let mut sourcemap_file = None;
        if let Some(map) = map_json
            && self.config.build.sourcemap == SourceMapMode::Linked
        {
            let map_file = format!("{file_name}.map");
            let map_path = out_dir.join(&map_file);
            fs::write(&map_path, map).map_err(|err| ZerhoError::Io {
                path: Some(map_path),
                reason: err.to_string(),
            })?;
            let _ = write!(final_js, "\n//# sourceMappingURL={map_file}\n");
            sourcemap_file = Some(map_file);
        }
        if self.config.security.secret_scan {
            scan_for_leaked_secrets(file_name, &final_js)?;
        }
        let integrity = self
            .config
            .security
            .sri
            .then(|| compute_sri_sha384(final_js.as_bytes()));
        let gzip_bytes = compute_gzip_size(final_js.as_bytes());
        let br_buf = compress_brotli(final_js.as_bytes());
        let brotli_bytes = br_buf.len();

        let js_path = out_dir.join(file_name);
        fs::write(&js_path, &final_js).map_err(|err| ZerhoError::Io {
            path: Some(js_path),
            reason: err.to_string(),
        })?;
        let brotli_file = if self.config.build.brotli {
            let br_name = format!("{file_name}.br");
            let br_path = out_dir.join(&br_name);
            fs::write(&br_path, &br_buf).map_err(|err| ZerhoError::Io {
                path: Some(br_path),
                reason: err.to_string(),
            })?;
            Some(br_name)
        } else {
            None
        };

        Ok(Chunk {
            file_name: file_name.to_owned(),
            hash,
            bytes: final_js.len(),
            gzip_bytes,
            brotli_bytes,
            brotli_file,
            sourcemap_file,
            integrity,
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

fn shake_modules(
    collected: &mut [CollectedModule],
    usage_map: &HashMap<PathBuf, ImportedSymbols>,
) -> ZerhoResult<usize> {
    let counts = collected
        .par_iter_mut()
        .map(|module| -> ZerhoResult<usize> {
            if !module.kind.is_script() {
                return Ok(0);
            }
            let used = usage_map
                .get(&module.canonical)
                .unwrap_or(&ImportedSymbols::All);
            let shaken = prune_unused_exports(&module.code, &module.canonical, used)?;
            module.code = shaken.code;
            Ok(shaken.removed_exports.len())
        })
        .collect::<ZerhoResult<Vec<usize>>>()?;
    Ok(counts.into_iter().sum())
}

fn build_hoist_inputs(collected: &[CollectedModule]) -> Vec<HoistedModuleInput> {
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
            out.push(HoistedModuleInput {
                id: module.id.clone(),
                code: String::new(),
                synthetic_default_expr: Some(format!("\"{asset_url}\"")),
                resolved_imports: BTreeMap::new(),
            });
        }
    }
    out
}

fn emit_post_order(
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

fn copy_dir_recursive(src: &Path, dst: &Path, optimize_svgs: bool) -> ZerhoResult<()> {
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
            copy_dir_recursive(&from, &to, optimize_svgs)?;
        } else if optimize_svgs
            && from.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg"))
            && let Ok(raw) = fs::read_to_string(&from)
        {
            fs::write(&to, optimize_svg(&raw)).map_err(|err| ZerhoError::Io {
                path: Some(to),
                reason: err.to_string(),
            })?;
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
    emit_sri: bool,
    emit_brotli: bool,
    chunks: &mut Vec<Chunk>,
) -> ZerhoResult<Option<(String, Option<String>)>> {
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
    let integrity = emit_sri.then(|| compute_sri_sha384(combined_css.as_bytes()));
    let gzip_bytes = compute_gzip_size(combined_css.as_bytes());
    let br_buf = compress_brotli(combined_css.as_bytes());
    let brotli_bytes = br_buf.len();
    let brotli_file = if emit_brotli {
        let br_name = format!("{file_name}.br");
        let br_path = out_dir.join(&br_name);
        fs::write(&br_path, &br_buf).map_err(|err| ZerhoError::Io {
            path: Some(br_path),
            reason: err.to_string(),
        })?;
        Some(br_name)
    } else {
        None
    };
    chunks.push(Chunk {
        file_name: file_name.clone(),
        hash: css_hash,
        bytes: combined_css.len(),
        gzip_bytes,
        brotli_bytes,
        brotli_file,
        sourcemap_file: None,
        integrity: integrity.clone(),
    });
    Ok(Some((file_name, integrity)))
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
        out.push_str(
            "\": function(__zerho_require, __zerho_exports) {\n    __zerho_exports.default = ",
        );
        out.push_str(&json);
        out.push_str(";\n  },\n");
    } else if let Some(asset_url) = &module.emitted_asset_url {
        out.push_str("  \"");
        out.push_str(&module.id);
        out.push_str(
            "\": function(__zerho_require, __zerho_exports) {\n    __zerho_exports.default = \"",
        );
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

/// Emits a reusable Library Mode bundle in either ESM (`.mjs`) or `CommonJS` (`.cjs`) format.
fn assemble_library_bundle(modules: &[CollectedModule], entry_id: &str, is_esm: bool) -> String {
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
fn assemble_shared_esm_chunk(modules: &[CollectedModule]) -> String {
    let mut out = String::from("export const __zerho_shared = {\n");
    for module in modules {
        push_module_factory(&mut out, module);
    }
    out.push_str("};\n");
    out
}

/// Emits a split async ESM chunk loadable directly via native browser `import("/assets/chunk-<hash>.js")`.
fn assemble_async_esm_chunk(
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
    js_sri: Option<&str>,
    css_file: Option<&str>,
    css_sri: Option<&str>,
    preload_chunks: &[(String, Option<String>)],
) -> String {
    let entry_web = format!("/{}", entry.to_string_lossy().replace('\\', "/"));
    let script_attr = match js_sri {
        Some(sri) => format!("src=\"/{js_file}\" integrity=\"{sri}\" crossorigin=\"anonymous\""),
        None => format!("src=\"/{js_file}\""),
    };
    let mut out = html.replace(&format!("src=\"{entry_web}\""), &script_attr);

    let mut head_inject = String::new();
    for (preload_file, preload_sri) in preload_chunks {
        match preload_sri {
            Some(sri) => {
                let _ = writeln!(
                    head_inject,
                    "  <link rel=\"modulepreload\" href=\"/{preload_file}\" integrity=\"{sri}\" crossorigin=\"anonymous\" />"
                );
            }
            None => {
                let _ = writeln!(
                    head_inject,
                    "  <link rel=\"modulepreload\" href=\"/{preload_file}\" crossorigin=\"anonymous\" />"
                );
            }
        }
    }

    if let Some(css) = css_file {
        match css_sri {
            Some(sri) => {
                let _ = writeln!(
                    head_inject,
                    "  <link rel=\"stylesheet\" href=\"/{css}\" integrity=\"{sri}\" crossorigin=\"anonymous\" />"
                );
            }
            None => {
                let _ = writeln!(head_inject, "  <link rel=\"stylesheet\" href=\"/{css}\" />");
            }
        }
    }

    if !head_inject.is_empty() {
        if let Some(pos) = out.find("</head>") {
            out.insert_str(pos, &head_inject);
        } else {
            out.push_str(&head_inject);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_html_entry_and_injects_css_and_modulepreload_with_sri() {
        let html = "<!doctype html><html><head><title>App</title></head><body><script type=\"module\" src=\"/src/main.ts\"></script></body></html>";
        let preload = vec![(
            "assets/shared-deadbeef.js".to_owned(),
            Some("sha384-sharedHash".to_owned()),
        )];
        let prod = rewrite_html_for_production(
            html,
            Path::new("src/main.ts"),
            "assets/index-abcd1234.js",
            Some("sha384-jsHash"),
            Some("assets/index-9999aaaa.css"),
            Some("sha384-cssHash"),
            &preload,
        );
        assert!(prod.contains(
            "src=\"/assets/index-abcd1234.js\" integrity=\"sha384-jsHash\" crossorigin=\"anonymous\""
        ));
        assert!(prod.contains(
            "<link rel=\"modulepreload\" href=\"/assets/shared-deadbeef.js\" integrity=\"sha384-sharedHash\" crossorigin=\"anonymous\" />"
        ));
        assert!(prod.contains(
            "href=\"/assets/index-9999aaaa.css\" integrity=\"sha384-cssHash\" crossorigin=\"anonymous\""
        ));
        assert!(!prod.contains("/src/main.ts"));
    }

    #[test]
    fn bundles_css_modules_and_hashed_svg_asset_imports() {
        let dir = std::env::temp_dir().join("zerho-bundler-css-mod-asset-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src dir");

        fs::write(dir.join("src/card.module.css"), ".panel { padding: 12px; }")
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

    #[test]
    fn parallel_bundler_is_deterministic_with_scope_hoisting_redb_cache_and_brotli() {
        let dir = std::env::temp_dir().join("zerho-bundler-v05-parallel-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src dir");

        let mut entry_src = String::new();
        for i in 0..12 {
            fs::write(
                dir.join(format!("src/mod_{i}.ts")),
                format!(
                    "export const used_{i}: number = {i};\nexport const dead_{i}: number = 999;\n"
                ),
            )
            .expect("write leaf module");
            let _ = writeln!(entry_src, "import {{ used_{i} }} from './mod_{i}';");
        }
        entry_src.push_str("if (false) { console.log('DEAD_BRANCH_SHOULD_FOLD'); }\n");
        entry_src.push_str("console.log(");
        for i in 0..12 {
            if i > 0 {
                entry_src.push_str(", ");
            }
            let _ = write!(entry_src, "used_{i}");
        }
        entry_src.push_str(");\n");
        fs::write(dir.join("src/main.ts"), entry_src).expect("write entry");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/main.ts");
        cfg.build.minify = true;
        cfg.build.brotli = true;
        cfg.build.scope_hoisting = true;

        // First build populates incremental redb cache.
        let out_a = Bundler::new(cfg.clone()).build().expect("first parallel build");
        assert_eq!(out_a.module_count, 13);
        assert_eq!(out_a.tree_shaken_exports, 12);
        assert!(out_a.scope_hoisted, "acyclic ESM should be scope-hoisted");
        assert_eq!(out_a.cache_misses, 13);

        // Verify scope hoisting, constant folding/DCE, and .br pre-compression.
        let entry_js = fs::read_to_string(out_a.out_dir.join(&out_a.chunks[0].file_name))
            .expect("read entry js");
        assert!(!entry_js.contains("__zerho_require"));
        assert!(!entry_js.contains("DEAD_BRANCH_SHOULD_FOLD"));
        let br_file = out_a.chunks[0]
            .brotli_file
            .as_ref()
            .expect("brotli file should be emitted");
        assert!(out_a.out_dir.join(br_file).is_file());

        // Second build hits redb cache for all 13 modules and produces identical hash.
        let out_b = Bundler::new(cfg).build().expect("second cached build");
        assert_eq!(out_b.cache_hits, 13);
        assert_eq!(out_b.cache_misses, 0);
        assert_eq!(out_a.chunks[0].hash, out_b.chunks[0].hash);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extracts_shared_chunks_and_injects_modulepreload() {
        let dir = std::env::temp_dir().join("zerho-bundler-shared-chunks-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src dir");

        fs::write(
            dir.join("index.html"),
            "<!doctype html><html><head><title>Split</title></head><body><script type=\"module\" src=\"/src/main.ts\"></script></body></html>",
        )
        .expect("write html");
        fs::write(
            dir.join("src/shared.ts"),
            "export const SHARED_TOKEN = 'ZERHO_SHARED_UTIL_UNIQUE_MARKER';\n",
        )
        .expect("write shared");
        fs::write(
            dir.join("src/lazyA.ts"),
            "import { SHARED_TOKEN } from './shared';\nexport default function a() { return SHARED_TOKEN + ':A'; }\n",
        )
        .expect("write lazyA");
        fs::write(
            dir.join("src/lazyB.ts"),
            "import { SHARED_TOKEN } from './shared';\nexport default function b() { return SHARED_TOKEN + ':B'; }\n",
        )
        .expect("write lazyB");
        fs::write(
            dir.join("src/main.ts"),
            "export async function run() { const a = await import('./lazyA'); const b = await import('./lazyB'); console.log(a, b); }\nrun();\n",
        )
        .expect("write entry");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/main.ts");
        cfg.build.minify = false;

        let out = Bundler::new(cfg).build().expect("split build should succeed");
        let shared_chunk = out
            .chunks
            .iter()
            .find(|c| c.file_name.starts_with("assets/shared-"))
            .expect("shared chunk should be emitted");

        let shared_js = fs::read_to_string(out.out_dir.join(&shared_chunk.file_name))
            .expect("read shared chunk");
        assert!(shared_js.contains("ZERHO_SHARED_UTIL_UNIQUE_MARKER"));

        let dynamic_chunks: Vec<&Chunk> = out
            .chunks
            .iter()
            .filter(|c| c.file_name.starts_with("assets/chunk-"))
            .collect();
        assert_eq!(dynamic_chunks.len(), 2);
        for chunk in dynamic_chunks {
            let content = fs::read_to_string(out.out_dir.join(&chunk.file_name))
                .expect("read dynamic chunk");
            assert!(
                !content.contains("ZERHO_SHARED_UTIL_UNIQUE_MARKER"),
                "shared module must not be duplicated in dynamic chunks"
            );
            assert!(content.contains(&format!("/{}", shared_chunk.file_name)));
        }

        let prod_html = fs::read_to_string(out.out_dir.join("index.html")).expect("read html");
        assert!(prod_html.contains(&format!(
            "<link rel=\"modulepreload\" href=\"/{}\"",
            shared_chunk.file_name
        )));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn barrel_optimizer_skips_unused_modules_and_emits_stats_html() {
        let dir = std::env::temp_dir().join("zerho-bundler-barrel-stats-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/ui")).expect("create src/ui");

        fs::write(dir.join("src/ui/Button.ts"), "export const Button = 'BTN';\n")
            .expect("write Button");
        for i in 0..4 {
            fs::write(
                dir.join(format!("src/ui/Unused{i}.ts")),
                format!("export const Unused{i} = 'HEAVY_{i}';\n"),
            )
            .expect("write Unused");
        }
        fs::write(
            dir.join("src/ui/index.ts"),
            "export { Button } from './Button';\n\
             export { Unused0 } from './Unused0';\n\
             export { Unused1 } from './Unused1';\n\
             export { Unused2 } from './Unused2';\n\
             export { Unused3 } from './Unused3';\n",
        )
        .expect("write barrel");
        fs::write(
            dir.join("src/main.ts"),
            "import { Button } from './ui';\nconsole.log(Button);\n",
        )
        .expect("write entry");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/main.ts");
        cfg.build.analyze = true;

        let out = Bundler::new(cfg).build().expect("barrel build should succeed");
        assert_eq!(out.barrel_imports_bypassed, 1);
        // Only main.ts + Button.ts should be loaded (barrel + 4 unused files completely skipped!).
        assert_eq!(out.module_count, 2);

        let stats_path = out.stats_file.expect("stats.html should be emitted");
        assert!(stats_path.is_file());
        let stats_html = fs::read_to_string(&stats_path).expect("read stats.html");
        assert!(stats_html.contains("Barrels Bypassed"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn builds_library_mode_esm_and_cjs_bundles() {
        let dir = std::env::temp_dir().join("zerho-bundler-lib-mode-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src");

        fs::write(
            dir.join("src/math.ts"),
            "export const double = (n: number) => n * 2;\n",
        )
        .expect("write math");
        fs::write(
            dir.join("src/index.ts"),
            "import { double } from './math';\nexport function quad(n: number) { return double(double(n)); }\n",
        )
        .expect("write lib entry");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.name = "my-math-lib".to_owned();
        cfg.build.lib.entry = Some(PathBuf::from("src/index.ts"));
        cfg.build.lib.formats = vec!["es".to_owned(), "cjs".to_owned()];

        let out = Bundler::new(cfg).build().expect("library build should succeed");
        assert!(out.out_dir.join("my-math-lib.mjs").is_file());
        assert!(out.out_dir.join("my-math-lib.cjs").is_file());
        assert!(out.out_dir.join("my-math-lib.mjs.br").is_file());
        assert!(out.out_dir.join("my-math-lib.cjs.br").is_file());

        let esm = fs::read_to_string(out.out_dir.join("my-math-lib.mjs")).expect("read mjs");
        let cjs = fs::read_to_string(out.out_dir.join("my-math-lib.cjs")).expect("read cjs");
        assert!(esm.contains("export"));
        assert!(cjs.contains("module.exports"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_declarative_wasm_plugin_from_zerho_config() {
        const MINIMAL_ABI_WASM: &[u8] = &[
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7f, 0x03, 0x02, 0x01, 0x00, 0x07, 0x15, 0x01, 0x11, b'z', b'e', b'r', b'h', b'o',
            b'_', b'a', b'b', b'i', b'_', b'v', b'e', b'r', b's', b'i', b'o', b'n', 0x00, 0x00,
            0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x01, 0x0b,
        ];

        let dir = std::env::temp_dir().join("zerho-bundler-wasm-plugin-config-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src");
        fs::create_dir_all(dir.join("plugins")).expect("create plugins");
        fs::write(dir.join("plugins/demo.wasm"), MINIMAL_ABI_WASM).expect("write wasm");
        fs::write(dir.join("src/main.ts"), "export const ok = 1;\nconsole.log(ok);\n")
            .expect("write entry");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/main.ts");
        cfg.plugins.wasm = vec![PathBuf::from("plugins/demo.wasm")];
        cfg.plugins.fuel_limit = 500_000;

        let out = Bundler::new(cfg.clone())
            .build()
            .expect("valid wasm plugin should load and build");
        assert_eq!(out.module_count, 1);

        // Invalid wasm binary must fail build cleanly.
        fs::write(dir.join("plugins/bad.wasm"), b"not-a-wasm").expect("write bad wasm");
        let mut bad_cfg = cfg;
        bad_cfg.plugins.wasm = vec![PathBuf::from("plugins/bad.wasm")];
        assert!(Bundler::new(bad_cfg).build().is_err());

        let _ = fs::remove_dir_all(&dir);
    }
}
