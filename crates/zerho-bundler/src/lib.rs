mod assemble;
pub mod check;
mod html;
pub mod images;
pub mod security;
pub mod stats;
mod targets;

use assemble::{
    assemble_async_esm_chunk, assemble_js_bundle, assemble_shared_esm_chunk, build_hoist_inputs,
    emit_post_order, is_side_effect_free, shake_modules,
};
pub use check::{CheckIssue, CheckReport, CheckSeverity, run_project_check};
use html::{copy_dir_recursive, emit_css_bundle, is_css_module_path, rewrite_html_for_production};
use rayon::prelude::*;
use security::{
    build_csp_manifest, compress_brotli, compute_gzip_size, compute_sri_sha384, enforce_budgets,
    scan_for_leaked_secrets,
};
use stats::{ModuleStat, emit_bundle_stats_html};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use zerho_config::{SourceMapMode, ZerhoConfig, build_import_meta_env_json, load_env_vars};
use zerho_core::{ContentHash, ModuleId, ModuleKind, ZerhoError, ZerhoResult};
use zerho_graph::{CachedArtifact, DiskCache, ModuleGraph};
use zerho_plugin::{DefinePlugin, PluginRegistry, register_wasm_plugins};
use zerho_resolver::{AssetQuery, OxcResolver, Resolver};
use zerho_transform::{
    ImportedSymbols, TransformInput, TransformPipeline, collect_dynamic_imports,
    collect_imported_symbols, collect_web_worker_urls, encode_base64, finalize_js_bundle_with_map,
    inline_css_imports, is_likely_commonjs, lower_esm_for_bundle, optimize_svg,
    parse_barrel_exports, rewrite_barrel_imports, rewrite_cjs_requires_for_env, rewrite_css_urls,
    rewrite_dynamic_imports, rewrite_web_worker_urls, try_scope_hoist_bundle, wrap_cjs_as_esm,
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
pub(crate) struct CollectedModule {
    pub(crate) id: String,
    pub(crate) canonical: PathBuf,
    pub(crate) kind: ModuleKind,
    pub(crate) code: String,
    pub(crate) css_exports: Option<BTreeMap<String, String>>,
    pub(crate) emitted_asset_url: Option<String>,
    pub(crate) resolved_imports: BTreeMap<String, Option<String>>,
}

#[derive(Debug)]
struct LoadedNode {
    canonical: PathBuf,
    module: CollectedModule,
    hash: Option<ContentHash>,
    dep_paths: Vec<PathBuf>,
    dep_ids: Vec<ModuleId>,
    dynamic_deps: Vec<PathBuf>,
    worker_deps: Vec<PathBuf>,
    imported_symbols: Vec<(PathBuf, ImportedSymbols)>,
    asset_chunks: Vec<Chunk>,
    synthetic_modules: Vec<CollectedModule>,
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
        let mut env_json = build_import_meta_env_json("production", false, &env_vars);
        if config.build.ssr && env_json.ends_with('}') {
            env_json.pop();
            env_json.push_str(",\"SSR\":true}");
        }
        let env_hash = ContentHash::from_bytes(format!("{env_json}:production").as_bytes());
        let mut defines = BTreeMap::new();
        defines.insert("import.meta.env".to_owned(), env_json);
        defines.insert(
            "process.env.NODE_ENV".to_owned(),
            "\"production\"".to_owned(),
        );
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

        if self.config.build.ssr {
            return self.build_ssr();
        }

        if let Some(lib_entry) = self.config.build.lib.entry.clone() {
            return self.build_library(&lib_entry);
        }

        if !self.config.build.entries.is_empty() {
            return self.build_mpa();
        }

        let entry_path = self.root.join(&self.config.project.entry);
        let entry_canon = entry_path.canonicalize().map_err(|err| ZerhoError::Io {
            path: Some(entry_path.clone()),
            reason: format!("entry file not found: {err}"),
        })?;

        let out_dir = self.prepare_out_dir()?;

        let mut collected = Vec::new();
        let mut dynamic_targets: Vec<PathBuf> = Vec::new();
        let mut worker_targets: Vec<PathBuf> = Vec::new();
        let mut usage_map: HashMap<PathBuf, ImportedSymbols> = HashMap::new();
        let mut asset_chunks: Vec<Chunk> = Vec::new();
        usage_map.insert(entry_canon.clone(), ImportedSymbols::All);

        let barrel_imports_bypassed = self.collect_module_graph(
            &entry_canon,
            &out_dir,
            &mut collected,
            &mut dynamic_targets,
            &mut worker_targets,
            &mut usage_map,
            &mut asset_chunks,
        )?;

        let mut tree_shaken_exports = shake_modules(&mut collected, &usage_map)?;

        let mut chunks = Vec::new();
        let split_res = self.emit_split_chunks(&out_dir, dynamic_targets, &mut chunks)?;
        tree_shaken_exports += split_res.tree_shaken_exports;
        let worker_url_map = self.emit_worker_chunks(&out_dir, worker_targets, &mut chunks)?;
        chunks.extend(asset_chunks);

        // Rewrite dynamic `import(...)` and Web Worker `new URL(...)` specifiers in parallel.
        if !split_res.dynamic_url_map.is_empty() || !worker_url_map.is_empty() {
            collected
                .par_iter_mut()
                .try_for_each(|module| -> ZerhoResult<()> {
                    if module.kind.is_script() {
                        let importer = module.canonical.clone();
                        if !split_res.dynamic_url_map.is_empty() {
                            module.code =
                                rewrite_dynamic_imports(&module.code, &importer, |spec| {
                                    let resolved =
                                        self.resolver.resolve(spec, Some(&importer)).ok()?;
                                    let canon =
                                        resolved.path.canonicalize().unwrap_or(resolved.path);
                                    split_res.dynamic_url_map.get(&canon).cloned()
                                })?;
                        }
                        if !worker_url_map.is_empty() {
                            module.code =
                                rewrite_web_worker_urls(&module.code, &importer, |spec| {
                                    let resolved =
                                        self.resolver.resolve(spec, Some(&importer)).ok()?;
                                    let canon =
                                        resolved.path.canonicalize().unwrap_or(resolved.path);
                                    worker_url_map.get(&canon).cloned()
                                })?;
                        }
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

    pub(crate) fn prepare_out_dir(&self) -> ZerhoResult<PathBuf> {
        let out_dir = if self.config.build.out_dir.is_absolute() {
            self.config.build.out_dir.clone()
        } else {
            self.root.join(&self.config.build.out_dir)
        };
        if self.config.build.empty_out_dir && out_dir.exists() && out_dir != self.root {
            fs::remove_dir_all(&out_dir).map_err(|err| ZerhoError::Io {
                path: Some(out_dir.clone()),
                reason: format!("failed to empty out_dir before build: {err}"),
            })?;
        }
        let assets_dir = out_dir.join("assets");
        fs::create_dir_all(&assets_dir).map_err(|err| ZerhoError::Io {
            path: Some(assets_dir),
            reason: err.to_string(),
        })?;
        Ok(out_dir)
    }

    pub(crate) fn lower_esm_modules(&self, collected: &mut [CollectedModule]) -> ZerhoResult<()> {
        collected
            .par_iter_mut()
            .try_for_each(|module| -> ZerhoResult<()> {
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
            let mut nested_workers = Vec::new();
            let mut chunk_usage = HashMap::new();
            chunk_usage.insert(target.clone(), ImportedSymbols::All);

            self.collect_module_graph(
                &target,
                out_dir,
                &mut chunk_modules,
                &mut nested_dynamic,
                &mut nested_workers,
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
            let url = self.config.build.asset_url(&emitted.file_name);
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
            dynamic_url_map.insert(target, self.config.build.asset_url(&emitted.file_name));
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

    fn emit_worker_chunks(
        &mut self,
        out_dir: &Path,
        worker_targets: Vec<PathBuf>,
        chunks: &mut Vec<Chunk>,
    ) -> ZerhoResult<HashMap<PathBuf, String>> {
        let mut worker_url_map = HashMap::new();
        let mut processed = HashSet::new();
        for target in worker_targets {
            if !processed.insert(target.clone()) {
                continue;
            }
            let mut worker_mods = Vec::new();
            let mut nested_dyn = Vec::new();
            let mut nested_workers = Vec::new();
            let mut usage = HashMap::new();
            usage.insert(target.clone(), ImportedSymbols::All);

            self.collect_module_graph(
                &target,
                out_dir,
                &mut worker_mods,
                &mut nested_dyn,
                &mut nested_workers,
                &mut usage,
                chunks,
            )?;
            let _ = shake_modules(&mut worker_mods, &usage)?;
            self.lower_esm_modules(&mut worker_mods)?;

            let target_id = self.module_key(&target);
            let raw_worker = assemble_js_bundle(&worker_mods, &target_id);
            let emitted = self.emit_js_chunk(out_dir, "worker", &raw_worker)?;
            worker_url_map.insert(target, self.config.build.asset_url(&emitted.file_name));
            chunks.push(emitted);
        }
        Ok(worker_url_map)
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
            let base = self.config.build.normalized_base();
            let prod_html = rewrite_html_for_production(
                &html,
                &self.config.project.entry,
                &base,
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
            self.config
                .project
                .entry
                .to_string_lossy()
                .replace('\\', "/"),
            self.config.build.sourcemap.as_str(),
        );
        let manifest_path = out_dir.join("manifest.json");
        fs::write(&manifest_path, manifest).map_err(|err| ZerhoError::Io {
            path: Some(manifest_path),
            reason: err.to_string(),
        })
    }

    pub(crate) fn emit_js_chunk(
        &self,
        out_dir: &Path,
        prefix: &str,
        raw_js: &str,
    ) -> ZerhoResult<Chunk> {
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn collect_module_graph(
        &mut self,
        entry: &Path,
        out_dir: &Path,
        collected: &mut Vec<CollectedModule>,
        dynamic_targets: &mut Vec<PathBuf>,
        worker_targets: &mut Vec<PathBuf>,
        usage_map: &mut HashMap<PathBuf, ImportedSymbols>,
        asset_chunks: &mut Vec<Chunk>,
    ) -> ZerhoResult<usize> {
        let entry_canon = entry.canonicalize().unwrap_or_else(|_| entry.to_path_buf());
        let mut discovered: HashSet<PathBuf> = HashSet::new();
        let mut frontier: Vec<PathBuf> = vec![entry_canon.clone()];
        discovered.insert(entry_canon.clone());

        let mut loaded_map: HashMap<PathBuf, (CollectedModule, Vec<PathBuf>)> = HashMap::new();
        let mut synthetic_seen: HashSet<String> = HashSet::new();
        let mut total_barrel_bypassed = 0usize;

        while !frontier.is_empty() {
            let batch: Vec<LoadedNode> = frontier
                .par_iter()
                .map(|canonical| self.load_and_transform_module(canonical, out_dir))
                .collect::<ZerhoResult<Vec<LoadedNode>>>()?;

            let mut next_frontier = Vec::new();
            for node in batch {
                total_barrel_bypassed += node.barrel_bypassed;
                asset_chunks.extend(node.asset_chunks);
                for syn in node.synthetic_modules {
                    if synthetic_seen.insert(syn.id.clone()) {
                        collected.push(syn);
                    }
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
                worker_targets.extend(node.worker_deps);
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
                worker_deps: Vec::new(),
                imported_symbols: Vec::new(),
                asset_chunks: vec![chunk],
                synthetic_modules: Vec::new(),
                barrel_bypassed: 0,
            });
        }

        let raw_source = fs::read_to_string(canonical).map_err(|err| ZerhoError::Io {
            path: Some(canonical.to_path_buf()),
            reason: err.to_string(),
        })?;

        let mut extra_asset_chunks = Vec::new();
        let mut cjs_dep_paths: Vec<PathBuf> = Vec::new();

        // Shared-registry CommonJS -> ESM wrapping (preserving singleton instances across
        // packages like react / react-dom / scheduler) and CSS @import + url(...) rewriting.
        let source = if kind == ModuleKind::Js && is_likely_commonjs(&raw_source) {
            if let Ok(rewritten) =
                rewrite_cjs_requires_for_env(&raw_source, canonical, true, |spec| {
                    let dep = self.resolver.resolve(spec, Some(canonical)).ok()?;
                    if dep.is_virtual {
                        return None;
                    }
                    let dep_canon = dep.path.canonicalize().unwrap_or(dep.path);
                    let key = self.module_key(&dep_canon);
                    cjs_dep_paths.push(dep_canon);
                    Some(key)
                })
            {
                format!(
                    "const __zerho_cjs_module = {{ exports: {{}} }};\n\
                     (function(require, module, exports) {{\n\
                     {rewritten}\n\
                     }})((id) => {{ const m = __zerho_require(id); return m && typeof m === \"object\" && \"default\" in m ? m.default : m; }}, __zerho_cjs_module, __zerho_cjs_module.exports);\n\
                     const __zerho_cjs_entry = __zerho_cjs_module.exports;\n\
                     export default __zerho_cjs_entry;\n"
                )
            } else {
                wrap_cjs_as_esm(&raw_source)
            }
        } else if kind == ModuleKind::Css {
            let inlined = inline_css_imports(&raw_source, canonical, |spec, importer| {
                let dep = self.resolver.resolve(spec, Some(importer)).ok()?;
                if dep.is_virtual {
                    return None;
                }
                let dep_canon = dep.path.canonicalize().unwrap_or(dep.path);
                let css = fs::read_to_string(&dep_canon).ok()?;
                Some((dep_canon, css))
            });
            rewrite_css_urls(&inlined, |url_ref| {
                let clean_ref = url_ref.split(['?', '#']).next().unwrap_or(url_ref);
                let target_path = if let Some(stripped) = clean_ref.strip_prefix('/') {
                    self.root.join(stripped)
                } else {
                    canonical.parent()?.join(clean_ref)
                };
                let target_canon = target_path.canonicalize().ok()?;
                let (asset_mod, asset_chunk) =
                    self.emit_imported_asset(&target_canon, out_dir).ok()?;
                extra_asset_chunks.push(asset_chunk);
                asset_mod.emitted_asset_url
            })
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
                            let resolved =
                                self.resolver.resolve(barrel_spec, Some(canonical)).ok()?;
                            if resolved.is_virtual || !resolved.kind.is_script() {
                                return None;
                            }
                            let barrel_canon =
                                resolved.path.canonicalize().unwrap_or(resolved.path);
                            let src = fs::read_to_string(&barrel_canon).ok()?;
                            let map = parse_barrel_exports(
                                &src,
                                &barrel_canon,
                                is_side_effect_free(&barrel_canon),
                            )?;
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
                    let leaf_canon = leaf_resolved
                        .path
                        .canonicalize()
                        .unwrap_or(leaf_resolved.path);
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

        let mut worker_deps = Vec::new();
        if kind.is_script() {
            for worker_spec in collect_web_worker_urls(&code, canonical)? {
                if let Ok(resolved) = self.resolver.resolve(&worker_spec, Some(canonical))
                    && !resolved.is_virtual
                {
                    let worker_canon = resolved.path.canonicalize().unwrap_or(resolved.path);
                    worker_deps.push(worker_canon);
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
        let mut synthetic_modules = Vec::new();
        for cjs_dep in cjs_dep_paths {
            dep_ids.push(ModuleId::from_specifier(&cjs_dep.to_string_lossy()));
            imported_symbols.push((cjs_dep.clone(), ImportedSymbols::All));
            dep_paths.push(cjs_dep);
        }
        for spec in &imports {
            if let Ok(resolved) = self.resolver.resolve(spec, Some(canonical)) {
                if resolved.is_virtual {
                    resolved_imports.insert(spec.clone(), None);
                } else {
                    let dep_canon = resolved.path.canonicalize().unwrap_or(resolved.path);
                    match resolved.query {
                        AssetQuery::Raw => {
                            let raw_text =
                                fs::read_to_string(&dep_canon).map_err(|err| ZerhoError::Io {
                                    path: Some(dep_canon.clone()),
                                    reason: err.to_string(),
                                })?;
                            let raw_id = format!("{}?raw", self.module_key(&dep_canon));
                            synthetic_modules.push(CollectedModule {
                                id: raw_id.clone(),
                                canonical: dep_canon,
                                kind: ModuleKind::Json,
                                code: format!(
                                    "export default \"{}\";\n",
                                    escape_js_string_literal(&raw_text)
                                ),
                                css_exports: None,
                                emitted_asset_url: None,
                                resolved_imports: BTreeMap::new(),
                            });
                            resolved_imports.insert(spec.clone(), Some(raw_id));
                        }
                        AssetQuery::Url => {
                            let (mut asset_mod, asset_chunk) =
                                self.emit_imported_asset(&dep_canon, out_dir)?;
                            let url_id = format!("{}?url", self.module_key(&dep_canon));
                            asset_mod.id.clone_from(&url_id);
                            extra_asset_chunks.push(asset_chunk);
                            synthetic_modules.push(asset_mod);
                            resolved_imports.insert(spec.clone(), Some(url_id));
                        }
                        AssetQuery::None => {
                            dep_ids.push(ModuleId::from_specifier(&dep_canon.to_string_lossy()));
                            dep_paths.push(dep_canon.clone());
                            if resolved.kind == ModuleKind::Css && !is_css_module_path(&dep_canon) {
                                resolved_imports.insert(spec.clone(), None);
                            } else {
                                resolved_imports
                                    .insert(spec.clone(), Some(self.module_key(&dep_canon)));
                            }
                        }
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
            worker_deps,
            imported_symbols,
            asset_chunks: extra_asset_chunks,
            synthetic_modules,
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
            emitted_asset_url: Some(self.config.build.asset_url(&file_name)),
            resolved_imports: BTreeMap::new(),
        };
        Ok((module, chunk))
    }

    pub(crate) fn module_key(&self, canonical: &Path) -> String {
        canonical.strip_prefix(&self.root).map_or_else(
            |_| canonical.to_string_lossy().replace('\\', "/"),
            |rel| rel.to_string_lossy().replace('\\', "/"),
        )
    }
}

fn escape_js_string_literal(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
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
            "/",
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
        assert!(
            has_svg_chunk,
            "imported svg should be emitted as hashed asset"
        );

        let entry_js =
            fs::read_to_string(out.out_dir.join(&out.chunks[0].file_name)).expect("read entry js");
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
        let out_a = Bundler::new(cfg.clone())
            .build()
            .expect("first parallel build");
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

        let out = Bundler::new(cfg)
            .build()
            .expect("split build should succeed");
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
            let content =
                fs::read_to_string(out.out_dir.join(&chunk.file_name)).expect("read dynamic chunk");
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

        fs::write(
            dir.join("src/ui/Button.ts"),
            "export const Button = 'BTN';\n",
        )
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

        let out = Bundler::new(cfg)
            .build()
            .expect("barrel build should succeed");
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

        let out = Bundler::new(cfg)
            .build()
            .expect("library build should succeed");
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
        fs::write(
            dir.join("src/main.ts"),
            "export const ok = 1;\nconsole.log(ok);\n",
        )
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

    #[test]
    fn inlines_css_imports_and_hashes_css_url_assets() {
        let dir = std::env::temp_dir().join("zerho-bundler-css-import-url-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src");

        fs::write(dir.join("src/tokens.css"), ":root { --accent: #f97316; }\n")
            .expect("write tokens.css");
        fs::write(
            dir.join("src/bg.svg"),
            "<!-- comment --><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
        )
        .expect("write bg.svg");
        fs::write(dir.join("src/font.woff2"), b"WOFF2_FAKE_FONT_BYTES").expect("write font.woff2");
        fs::write(
            dir.join("src/styles.css"),
            "@import \"./tokens.css\";\n\
             @font-face { font-family: 'RH'; src: url('./font.woff2') format('woff2'); }\n\
             body { color: var(--accent); background: url('./bg.svg'); }\n",
        )
        .expect("write styles.css");
        fs::write(
            dir.join("src/main.ts"),
            "import './styles.css';\nconsole.log('styled');\n",
        )
        .expect("write main.ts");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/main.ts");

        let out = Bundler::new(cfg)
            .build()
            .expect("css asset build should succeed");
        let css_chunk = out
            .chunks
            .iter()
            .find(|c| {
                Path::new(&c.file_name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("css"))
            })
            .expect("css bundle should be emitted");
        let css_out =
            fs::read_to_string(out.out_dir.join(&css_chunk.file_name)).expect("read emitted css");
        assert!(css_out.contains("--accent:#f97316") || css_out.contains("--accent: #f97316"));
        assert!(css_out.contains("/assets/font-"));
        assert!(css_out.contains("/assets/bg-"));
        assert!(!css_out.contains("@import"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn builds_multi_page_app_with_shared_chunk_deduplication() {
        let dir = std::env::temp_dir().join("zerho-bundler-mpa-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src");

        fs::write(
            dir.join("src/common.ts"),
            "export const BRAND_MARKER = 'ZERHO_MPA_SHARED_MARKER';\n",
        )
        .expect("write common.ts");
        fs::write(
            dir.join("src/index.ts"),
            "import { BRAND_MARKER } from './common';\nconsole.log('home', BRAND_MARKER);\n",
        )
        .expect("write index.ts");
        fs::write(
            dir.join("src/admin.ts"),
            "import { BRAND_MARKER } from './common';\nconsole.log('admin', BRAND_MARKER);\n",
        )
        .expect("write admin.ts");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.build.minify = false;
        cfg.build
            .entries
            .insert("index".to_owned(), PathBuf::from("src/index.ts"));
        cfg.build
            .entries
            .insert("admin".to_owned(), PathBuf::from("src/admin.ts"));

        let out = Bundler::new(cfg).build().expect("MPA build should succeed");
        assert!(out.out_dir.join("index.html").is_file());
        assert!(out.out_dir.join("admin.html").is_file());
        let shared = out
            .chunks
            .iter()
            .find(|c| c.file_name.starts_with("assets/shared-"))
            .expect("MPA should extract shared chunk");
        let shared_js =
            fs::read_to_string(out.out_dir.join(&shared.file_name)).expect("read shared chunk");
        assert!(shared_js.contains("ZERHO_MPA_SHARED_MARKER"));

        let admin_html =
            fs::read_to_string(out.out_dir.join("admin.html")).expect("read admin.html");
        assert!(admin_html.contains(&format!(
            "<link rel=\"modulepreload\" href=\"/{}\"",
            shared.file_name
        )));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundles_web_workers_and_wasm_imports_into_hashed_chunks() {
        let dir = std::env::temp_dir().join("zerho-bundler-worker-wasm-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src");

        fs::write(
            dir.join("src/compute.worker.ts"),
            "self.onmessage = (e: MessageEvent) => self.postMessage(e.data);\n",
        )
        .expect("write worker");
        fs::write(dir.join("src/algo.wasm"), b"\0asm\x01\x00\x00\x00").expect("write wasm");
        fs::write(
            dir.join("src/main.ts"),
            "import initWasm from './algo.wasm';\n\
             const w = new Worker(new URL('./compute.worker.ts', import.meta.url), { type: 'module' });\n\
             console.log(w, initWasm);\n",
        )
        .expect("write main.ts");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/main.ts");
        cfg.build.minify = false;

        let out = Bundler::new(cfg)
            .build()
            .expect("worker + wasm build should succeed");
        let worker_chunk = out
            .chunks
            .iter()
            .find(|c| c.file_name.starts_with("assets/worker-"))
            .expect("dedicated worker chunk should be emitted");
        let wasm_chunk = out
            .chunks
            .iter()
            .find(|c| {
                Path::new(&c.file_name)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("wasm"))
            })
            .expect("hashed wasm chunk should be emitted");

        let entry_js =
            fs::read_to_string(out.out_dir.join(&out.chunks[0].file_name)).expect("read entry js");
        assert!(entry_js.contains(&format!("/{}", worker_chunk.file_name)));
        assert!(entry_js.contains("WebAssembly.instantiateStreaming"));
        assert!(entry_js.contains(&format!("/{}", wasm_chunk.file_name)));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn builds_dom_free_ssr_bundle_for_edge_runtimes() {
        let dir = std::env::temp_dir().join("zerho-bundler-ssr-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src");

        fs::write(
            dir.join("src/entry-server.ts"),
            "export function render(url: string) { return { url, ssr: import.meta.env.SSR }; }\n",
        )
        .expect("write entry-server.ts");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/entry-server.ts");
        cfg.build.ssr = true;
        cfg.build.minify = false;

        let out = Bundler::new(cfg).build().expect("SSR build should succeed");
        let server_mjs = out.out_dir.join("server.mjs");
        assert!(server_mjs.is_file());
        assert!(!out.out_dir.join("index.html").exists());

        let code = fs::read_to_string(&server_mjs).expect("read server.mjs");
        assert!(code.contains("\"SSR\": true") || code.contains("\"SSR\":true"));
        assert!(code.contains("export default"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn respects_build_base_and_empties_stale_out_dir() {
        let dir = std::env::temp_dir().join("zerho-bundler-base-and-clean-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src dir");
        fs::create_dir_all(dir.join("dist/assets")).expect("create stale dist dir");
        let stale_file = dir.join("dist/assets/index-stale000.js");
        fs::write(&stale_file, "// stale artifact").expect("write stale artifact");

        fs::write(
            dir.join("index.html"),
            "<!doctype html><html><head><title>Subpath</title></head><body><script type=\"module\" src=\"/src/main.ts\"></script></body></html>",
        )
        .expect("write index.html");
        fs::write(dir.join("src/styles.css"), "body { color: #f97316; }\n").expect("write css");
        fs::write(
            dir.join("src/lazy.ts"),
            "export default function lazy() { return 'ok'; }\n",
        )
        .expect("write lazy.ts");
        fs::write(
            dir.join("src/main.ts"),
            "import './styles.css';\nexport async function load() { return import('./lazy'); }\nconsole.log(load);\n",
        )
        .expect("write main.ts");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/main.ts");
        cfg.build.base = "/demo".to_owned();
        cfg.build.empty_out_dir = true;
        cfg.build.minify = false;

        let out = Bundler::new(cfg)
            .build()
            .expect("subpath build should succeed");
        assert!(
            !stale_file.exists(),
            "empty_out_dir = true must remove stale artifacts"
        );

        let html = fs::read_to_string(out.out_dir.join("index.html")).expect("read index.html");
        assert!(
            html.contains("src=\"/demo/assets/index-"),
            "index.html script should use /demo/ base: {html}"
        );
        assert!(
            html.contains("href=\"/demo/assets/index-"),
            "index.html stylesheet should use /demo/ base: {html}"
        );

        let entry_js =
            fs::read_to_string(out.out_dir.join(&out.chunks[0].file_name)).expect("read entry js");
        assert!(
            entry_js.contains("/demo/assets/chunk-"),
            "dynamic import should use /demo/ base: {entry_js}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tree_shakes_side_effect_free_package_modules() {
        let dir = std::env::temp_dir().join("zerho-bundler-side-effects-false-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src");
        fs::create_dir_all(dir.join("node_modules/pure-icons")).expect("create pkg");

        fs::write(
            dir.join("node_modules/pure-icons/package.json"),
            "{\"name\":\"pure-icons\",\"version\":\"1.0.0\",\"type\":\"module\",\"sideEffects\":false,\"exports\":{\".\":\"./index.js\",\"./unused\":\"./unused.js\"}}",
        )
        .expect("write package.json");
        fs::write(
            dir.join("node_modules/pure-icons/index.js"),
            "export const usedIcon = () => 'USED_ICON';\n",
        )
        .expect("write index.js");
        fs::write(
            dir.join("node_modules/pure-icons/unused.js"),
            "const DEAD_SIDE_EFFECT_TABLE = 'SHOULD_BE_DROPPED';\nconsole.log(DEAD_SIDE_EFFECT_TABLE);\nexport const unusedHelper = () => DEAD_SIDE_EFFECT_TABLE;\n",
        )
        .expect("write unused.js");
        fs::write(
            dir.join("src/main.ts"),
            "import { usedIcon } from 'pure-icons';\nimport { nonExistent } from 'pure-icons/unused';\nconsole.log(usedIcon(), typeof nonExistent);\n",
        )
        .expect("write main.ts");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/main.ts");
        cfg.build.minify = false;

        let out = Bundler::new(cfg)
            .build()
            .expect("sideEffects: false build should succeed");
        let js =
            fs::read_to_string(out.out_dir.join(&out.chunks[0].file_name)).expect("read entry js");
        assert!(js.contains("USED_ICON"), "used icon must be preserved");
        assert!(
            !js.contains("SHOULD_BE_DROPPED"),
            "dead top-level code in sideEffects:false module should be pruned: {js}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundles_vite_raw_and_url_query_imports() {
        let dir = std::env::temp_dir().join("zerho-bundler-raw-url-query-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("create src");

        fs::write(dir.join("src/query.sql"), "SELECT * FROM telemetry;\n")
            .expect("write query.sql");
        fs::write(
            dir.join("src/main.ts"),
            "import sqlText from './query.sql?raw';\n\
             import sqlUrl from './query.sql?url';\n\
             console.log(sqlText, sqlUrl);\n",
        )
        .expect("write main.ts");

        let mut cfg = ZerhoConfig::default();
        cfg.project.root = dir.clone();
        cfg.project.entry = PathBuf::from("src/main.ts");
        cfg.build.base = "/app/".to_owned();
        cfg.build.minify = false;

        let out = Bundler::new(cfg)
            .build()
            .expect("?raw and ?url build should succeed");
        let js =
            fs::read_to_string(out.out_dir.join(&out.chunks[0].file_name)).expect("read entry js");
        assert!(
            js.contains("SELECT * FROM telemetry;"),
            "?raw file contents should be inlined: {js}"
        );
        assert!(
            js.contains("/app/assets/query-"),
            "?url should emit hashed asset URL with build.base: {js}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
