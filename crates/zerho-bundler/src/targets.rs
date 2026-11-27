use crate::assemble::{
    assemble_async_esm_chunk, assemble_js_bundle, assemble_library_bundle,
    assemble_shared_esm_chunk, shake_modules,
};
use crate::html::{emit_css_bundle, rewrite_html_for_production};
use crate::security::{
    build_csp_manifest, compress_brotli, compute_gzip_size, compute_sri_sha384, enforce_budgets,
    scan_for_leaked_secrets,
};
use crate::{BundleOutput, Bundler, Chunk, CollectedModule};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use zerho_config::SourceMapMode;
use zerho_core::{ContentHash, ModuleKind, ZerhoError, ZerhoResult};
use zerho_transform::{ImportedSymbols, finalize_js_bundle_with_map};

impl Bundler {
    /// Builds a DOM-free Edge/Server ESM bundle (`dist/server.mjs`) targeting Cloudflare Workers,
    /// Deno, Node.js, and Bun (`import.meta.env.SSR = true`, no `index.html` emitted).
    pub(crate) fn build_ssr(&mut self) -> ZerhoResult<BundleOutput> {
        let entry_path = self.root.join(&self.config.project.entry);
        let entry_canon = entry_path.canonicalize().map_err(|err| ZerhoError::Io {
            path: Some(entry_path),
            reason: format!("SSR entry file not found: {err}"),
        })?;

        let out_dir = self.prepare_out_dir()?;

        let mut collected = Vec::new();
        let mut dynamic_targets = Vec::new();
        let mut worker_targets = Vec::new();
        let mut usage_map = HashMap::new();
        let mut asset_chunks = Vec::new();
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
        let tree_shaken_exports = shake_modules(&mut collected, &usage_map)?;
        self.lower_esm_modules(&mut collected)?;

        let entry_id = self.module_key(&entry_canon);
        let raw_ssr = assemble_library_bundle(&collected, &entry_id, true);
        let ssr_chunk = self.emit_named_js_file(&out_dir, "server.mjs", &raw_ssr)?;

        let manifest = format!(
            "{{\n  \"mode\": \"ssr\",\n  \"entry\": \"{}\",\n  \"file\": \"server.mjs\",\n  \"modules\": {},\n  \"tree_shaken_exports\": {tree_shaken_exports}\n}}\n",
            self.config
                .project
                .entry
                .to_string_lossy()
                .replace('\\', "/"),
            collected.len()
        );
        let manifest_path = out_dir.join("manifest.json");
        fs::write(&manifest_path, manifest).map_err(|err| ZerhoError::Io {
            path: Some(manifest_path),
            reason: err.to_string(),
        })?;

        let mut chunks = vec![ssr_chunk];
        chunks.extend(asset_chunks);
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

    #[allow(clippy::too_many_lines)]
    pub(crate) fn build_mpa(&mut self) -> ZerhoResult<BundleOutput> {
        let out_dir = self.prepare_out_dir()?;
        let base = self.config.build.normalized_base();

        let entries: Vec<(String, PathBuf)> = self
            .config
            .build
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let mut pending: Vec<(String, PathBuf, PathBuf, Vec<CollectedModule>)> = Vec::new();
        let mut chunks = Vec::new();
        let mut total_shaken = 0usize;
        let mut total_barrels = 0usize;
        let mut unique_modules = HashSet::new();

        for (name, rel_entry) in entries {
            let entry_path = self.root.join(&rel_entry);
            let entry_canon = entry_path.canonicalize().map_err(|err| ZerhoError::Io {
                path: Some(entry_path),
                reason: format!("MPA entry `{name}` not found: {err}"),
            })?;

            let mut collected = Vec::new();
            let mut dynamic_targets = Vec::new();
            let mut worker_targets = Vec::new();
            let mut usage_map = HashMap::new();
            let mut asset_chunks = Vec::new();
            usage_map.insert(entry_canon.clone(), ImportedSymbols::All);

            total_barrels += self.collect_module_graph(
                &entry_canon,
                &out_dir,
                &mut collected,
                &mut dynamic_targets,
                &mut worker_targets,
                &mut usage_map,
                &mut asset_chunks,
            )?;
            total_shaken += shake_modules(&mut collected, &usage_map)?;
            self.lower_esm_modules(&mut collected)?;
            for m in &collected {
                unique_modules.insert(m.id.clone());
            }
            chunks.extend(asset_chunks);
            pending.push((name, rel_entry, entry_canon, collected));
        }

        // Deduplicate modules shared across >= 2 MPA entry points into `assets/shared-<hash>.js`.
        let mut id_counts: HashMap<String, usize> = HashMap::new();
        for (_, _, _, mods) in &pending {
            let mut seen = HashSet::new();
            for m in mods {
                if (m.kind.is_script() || m.kind == ModuleKind::Json || m.css_exports.is_some())
                    && seen.insert(m.id.clone())
                {
                    *id_counts.entry(m.id.clone()).or_insert(0) += 1;
                }
            }
        }
        let shared_ids: HashSet<String> = id_counts
            .into_iter()
            .filter_map(|(id, count)| (count >= 2).then_some(id))
            .collect();

        let mut preload_chunks = Vec::new();
        let shared_url = if shared_ids.is_empty() {
            None
        } else {
            let mut shared_mods = Vec::new();
            let mut seen = HashSet::new();
            for (_, _, _, mods) in &pending {
                for m in mods {
                    if shared_ids.contains(&m.id) && seen.insert(m.id.clone()) {
                        shared_mods.push(m.clone());
                    }
                }
            }
            let raw_shared = assemble_shared_esm_chunk(&shared_mods);
            let emitted = self.emit_js_chunk(&out_dir, "shared", &raw_shared)?;
            let url = self.config.build.asset_url(&emitted.file_name);
            preload_chunks.push((emitted.file_name.clone(), emitted.integrity.clone()));
            chunks.push(emitted);
            Some(url)
        };

        let mut manifest_entries = Vec::new();
        for (name, rel_entry, entry_canon, mut mods) in pending {
            let uses_shared =
                shared_url.is_some() && mods.iter().any(|m| shared_ids.contains(&m.id));
            if uses_shared {
                mods.retain(|m| !shared_ids.contains(&m.id));
            }
            let entry_id = self.module_key(&entry_canon);
            let raw_js = if uses_shared {
                assemble_async_esm_chunk(&mods, &entry_id, shared_url.as_deref())
            } else {
                assemble_js_bundle(&mods, &entry_id)
            };
            let js_chunk = self.emit_js_chunk(&out_dir, &name, &raw_js)?;
            let css_emit = emit_css_bundle(
                &out_dir,
                &mods,
                self.config.security.sri,
                self.config.build.brotli,
                &mut chunks,
            )?;

            let html_name = format!("{name}.html");
            let src_html = self.root.join(&html_name);
            let base_html = if src_html.is_file() {
                fs::read_to_string(&src_html).map_err(|err| ZerhoError::Io {
                    path: Some(src_html),
                    reason: err.to_string(),
                })?
            } else {
                format!(
                    "<!doctype html><html><head><meta charset=\"utf-8\" /><title>{name}</title></head><body><div id=\"app\"></div><script type=\"module\" src=\"/{}\"></script></body></html>\n",
                    rel_entry.to_string_lossy().replace('\\', "/")
                )
            };
            let prod_html = rewrite_html_for_production(
                &base_html,
                &rel_entry,
                &base,
                &js_chunk.file_name,
                js_chunk.integrity.as_deref(),
                css_emit.as_ref().map(|(f, _)| f.as_str()),
                css_emit.as_ref().and_then(|(_, sri)| sri.as_deref()),
                if uses_shared { &preload_chunks } else { &[] },
            );
            let out_html = out_dir.join(&html_name);
            fs::write(&out_html, prod_html).map_err(|err| ZerhoError::Io {
                path: Some(out_html),
                reason: err.to_string(),
            })?;

            manifest_entries.push(format!("  \"{name}\": \"{}\"", js_chunk.file_name));
            chunks.push(js_chunk);
        }

        enforce_budgets(&chunks, &self.config.build.budgets)?;
        if self.config.security.csp {
            let csp_path = out_dir.join("csp.json");
            fs::write(&csp_path, build_csp_manifest(&chunks)).map_err(|err| ZerhoError::Io {
                path: Some(csp_path),
                reason: err.to_string(),
            })?;
        }
        let manifest = format!(
            "{{\n  \"mode\": \"mpa\",\n  \"entries\": {{\n{}\n  }}\n}}\n",
            manifest_entries.join(",\n")
        );
        let manifest_path = out_dir.join("manifest.json");
        fs::write(&manifest_path, manifest).map_err(|err| ZerhoError::Io {
            path: Some(manifest_path),
            reason: err.to_string(),
        })?;

        let cache_stats = self.cache.stats();
        Ok(BundleOutput {
            out_dir,
            chunks,
            module_count: unique_modules.len(),
            tree_shaken_exports: total_shaken,
            barrel_imports_bypassed: total_barrels,
            cache_hits: cache_stats.hits,
            cache_misses: cache_stats.misses,
            scope_hoisted: false,
            stats_file: None,
        })
    }

    pub(crate) fn build_library(&mut self, lib_entry: &Path) -> ZerhoResult<BundleOutput> {
        let entry_path = self.root.join(lib_entry);
        let entry_canon = entry_path.canonicalize().map_err(|err| ZerhoError::Io {
            path: Some(entry_path),
            reason: format!("library entry file not found: {err}"),
        })?;

        let out_dir = self.prepare_out_dir()?;

        let mut collected = Vec::new();
        let mut dynamic_targets = Vec::new();
        let mut worker_targets = Vec::new();
        let mut usage_map = HashMap::new();
        let mut asset_chunks = Vec::new();
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
}
