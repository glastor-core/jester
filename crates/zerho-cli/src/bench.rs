use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use zerho_bundler::Bundler;
use zerho_config::ZerhoConfig;
use zerho_core::{ModuleKind, ZerhoError, ZerhoResult};
use zerho_graph::{CachedArtifact, DiskCache};
use zerho_transform::{TransformInput, TransformPipeline};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Structured benchmark output for both human-readable tables and `--json` export.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct BenchResult {
    pub project: String,
    pub iterations: usize,
    pub module_count: usize,
    pub tree_shaken_exports: usize,
    pub scope_hoisted: bool,
    pub minify: bool,
    pub brotli: bool,
    pub sri: bool,
    pub profile: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    pub cpu_threads: usize,
    pub cold_transform_ms: f64,
    pub warm_redb_cache_ms: f64,
    pub cold_build_ms: f64,
    pub warm_build_min_ms: f64,
    pub warm_build_avg_ms: f64,
    pub warm_build_p95_ms: f64,
    pub entry_raw_bytes: usize,
    pub entry_gzip_bytes: usize,
    pub entry_brotli_bytes: usize,
}

impl BenchResult {
    pub fn to_json(&self) -> String {
        let throughput = if self.warm_build_min_ms > 0.0 {
            #[allow(clippy::cast_precision_loss)]
            {
                (self.module_count as f64) / (self.warm_build_min_ms / 1000.0)
            }
        } else {
            0.0
        };
        format!(
            "{{\n\
             \x20 \"environment\": {{\n\
             \x20   \"zerho_version\": \"{VERSION}\",\n\
             \x20   \"profile\": \"{}\",\n\
             \x20   \"os\": \"{}\",\n\
             \x20   \"arch\": \"{}\",\n\
             \x20   \"cpu_threads\": {}\n\
             \x20 }},\n\
             \x20 \"fixture\": {{\n\
             \x20   \"project\": \"{}\",\n\
             \x20   \"iterations\": {},\n\
             \x20   \"modules\": {},\n\
             \x20   \"tree_shaken_exports\": {},\n\
             \x20   \"scope_hoisted\": {},\n\
             \x20   \"minify\": {},\n\
             \x20   \"brotli\": {},\n\
             \x20   \"sri\": {}\n\
             \x20 }},\n\
             \x20 \"metrics\": {{\n\
             \x20   \"cold_transform_ms\": {:.3},\n\
             \x20   \"warm_redb_cache_ms\": {:.4},\n\
             \x20   \"cold_build_ms\": {:.2},\n\
             \x20   \"warm_build_min_ms\": {:.2},\n\
             \x20   \"warm_build_avg_ms\": {:.2},\n\
             \x20   \"warm_build_p95_ms\": {:.2},\n\
             \x20   \"throughput_mods_per_sec\": {:.0},\n\
             \x20   \"entry_raw_bytes\": {},\n\
             \x20   \"entry_gzip_bytes\": {},\n\
             \x20   \"entry_brotli_bytes\": {}\n\
             \x20 }}\n\
             }}",
            self.profile,
            self.os,
            self.arch,
            self.cpu_threads,
            self.project.replace('"', "\\\""),
            self.iterations,
            self.module_count,
            self.tree_shaken_exports,
            self.scope_hoisted,
            self.minify,
            self.brotli,
            self.sri,
            self.cold_transform_ms,
            self.warm_redb_cache_ms,
            self.cold_build_ms,
            self.warm_build_min_ms,
            self.warm_build_avg_ms,
            self.warm_build_p95_ms,
            throughput,
            self.entry_raw_bytes,
            self.entry_gzip_bytes,
            self.entry_brotli_bytes,
        )
    }

    pub fn print_human(&self) {
        let throughput = if self.warm_build_min_ms > 0.0 {
            #[allow(clippy::cast_precision_loss)]
            {
                (self.module_count as f64) / (self.warm_build_min_ms / 1000.0)
            }
        } else {
            0.0
        };
        println!(
            "zerho benchmark — {} ({} iterations)\n\
             \x20 environment:            zerho {VERSION} ({}) | {}/{} | {} logical threads\n\
             \x20 pipeline flags:         scope_hoisting={} minify={} brotli={} sri={}\n\
             \x20 cold transform (oxc):   {:.3} ms\n\
             \x20 warm redb cache hit:    {:.4} ms\n\
             \x20 cold build ({} mods):   {:.2} ms\n\
             \x20 warm build ({} mods):   min {:.2} ms | avg {:.2} ms | p95 {:.2} ms ({:.0} mods/s)\n\
             \x20 entry chunk size:       {} B raw | {} B gzip | {} B brotli ({} dead exports shaken)",
            self.project,
            self.iterations,
            self.profile,
            self.os,
            self.arch,
            self.cpu_threads,
            self.scope_hoisted,
            self.minify,
            self.brotli,
            self.sri,
            self.cold_transform_ms,
            self.warm_redb_cache_ms,
            self.module_count,
            self.cold_build_ms,
            self.module_count,
            self.warm_build_min_ms,
            self.warm_build_avg_ms,
            self.warm_build_p95_ms,
            throughput,
            self.entry_raw_bytes,
            self.entry_gzip_bytes,
            self.entry_brotli_bytes,
            self.tree_shaken_exports,
        );
    }
}

/// Runs reproducible benchmarks against either a real project config or a deterministic
/// synthetic N-module TypeScript project (`--synthetic N`).
pub fn execute_bench(
    base_config: &ZerhoConfig,
    iterations: usize,
    synthetic_modules: Option<usize>,
) -> ZerhoResult<BenchResult> {
    let mut temp_dir_to_clean: Option<PathBuf> = None;
    let config = if let Some(count) = synthetic_modules {
        let count = count.max(2);
        let dir = std::env::temp_dir().join(format!("zerho-synthetic-bench-{count}"));
        generate_synthetic_project(&dir, count)?;
        temp_dir_to_clean = Some(dir.clone());
        let mut cfg = ZerhoConfig::default();
        cfg.project.name = format!("synthetic-{count}-modules");
        cfg.project.root = dir;
        cfg.project.entry = PathBuf::from("src/main.ts");
        cfg.build.sourcemap = zerho_config::SourceMapMode::None;
        cfg
    } else {
        base_config.clone()
    };

    let result = measure_project(&config, iterations);
    if let Some(dir) = temp_dir_to_clean {
        let _ = fs::remove_dir_all(dir);
    }
    result
}

fn measure_project(config: &ZerhoConfig, iterations: usize) -> ZerhoResult<BenchResult> {
    let root = config
        .project
        .root
        .canonicalize()
        .unwrap_or_else(|_| config.project.root.clone());
    let entry_path = root.join(&config.project.entry);
    let entry_source = fs::read_to_string(&entry_path).map_err(|err| ZerhoError::Io {
        path: Some(entry_path.clone()),
        reason: err.to_string(),
    })?;
    let kind = ModuleKind::from_path(&entry_path);

    // 1. Cold transform benchmark (Oxc)
    let pipeline = TransformPipeline::default();
    let t_cold = Instant::now();
    let transformed = pipeline.transform(&TransformInput {
        path: entry_path,
        kind,
        source: entry_source.clone(),
        framework: config.project.framework,
        is_dev: true,
        minify_css: false,
    })?;
    let cold_transform_ms = t_cold.elapsed().as_secs_f64() * 1000.0;

    // 2. Warm embedded redb DiskCache lookup benchmark
    let cache_dir = if config.cache.dir.is_absolute() {
        config.cache.dir.clone()
    } else {
        root.join(&config.cache.dir)
    };
    let _ = fs::remove_dir_all(&cache_dir);
    let warm_redb_cache_ms = {
        let cache = DiskCache::new(&cache_dir, true);
        let cache_key = DiskCache::compute_key(
            &entry_source,
            "bench",
            config.project.framework.as_str(),
            true,
            false,
        );
        cache.put(
            cache_key,
            &CachedArtifact {
                code: transformed.code,
                imports: transformed.imports,
                output_hash: transformed.hash,
            },
        )?;
        let t_cache = Instant::now();
        for _ in 0..iterations {
            let _ = cache.get(cache_key);
        }
        (t_cache.elapsed().as_secs_f64() * 1000.0)
            / f64::from(u32::try_from(iterations).unwrap_or(1))
    };

    // 3. Cold production build (fresh cache cleared)
    let _ = fs::remove_dir_all(&cache_dir);
    let t_cold_build = Instant::now();
    let cold_out = Bundler::new(config.clone()).build()?;
    let cold_build_ms = t_cold_build.elapsed().as_secs_f64() * 1000.0;

    // 4. Warm incremental production builds (redb cache hot)
    let mut warm_times = Vec::with_capacity(iterations);
    let mut last_out = cold_out;
    for _ in 0..iterations {
        let mut bundler = Bundler::new(config.clone());
        let start = Instant::now();
        last_out = bundler.build()?;
        warm_times.push(start.elapsed());
    }
    warm_times.sort();
    let min_build = warm_times[0];
    let total_build: Duration = warm_times.iter().copied().sum();
    let avg_build = total_build / u32::try_from(iterations).unwrap_or(1);
    let p95_idx = ((iterations * 95) / 100).min(iterations.saturating_sub(1));
    let p95_build = warm_times[p95_idx];

    let entry_chunk = last_out.chunks.first();
    Ok(BenchResult {
        project: config.project.name.clone(),
        iterations,
        module_count: last_out.module_count,
        tree_shaken_exports: last_out.tree_shaken_exports,
        scope_hoisted: last_out.scope_hoisted,
        minify: config.build.minify,
        brotli: config.build.brotli,
        sri: config.security.sri,
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        cpu_threads: std::thread::available_parallelism().map_or(1, usize::from),
        cold_transform_ms,
        warm_redb_cache_ms,
        cold_build_ms,
        warm_build_min_ms: min_build.as_secs_f64() * 1000.0,
        warm_build_avg_ms: avg_build.as_secs_f64() * 1000.0,
        warm_build_p95_ms: p95_build.as_secs_f64() * 1000.0,
        entry_raw_bytes: entry_chunk.map_or(0, |c| c.bytes),
        entry_gzip_bytes: entry_chunk.map_or(0, |c| c.gzip_bytes),
        entry_brotli_bytes: entry_chunk.map_or(0, |c| c.brotli_bytes),
    })
}

fn generate_synthetic_project(dir: &Path, total_modules: usize) -> ZerhoResult<()> {
    let _ = fs::remove_dir_all(dir);
    let src_dir = dir.join("src");
    fs::create_dir_all(&src_dir).map_err(|err| ZerhoError::Io {
        path: Some(src_dir.clone()),
        reason: err.to_string(),
    })?;

    fs::write(
        dir.join("index.html"),
        "<!doctype html><html><head><title>Synthetic Bench</title></head><body><script type=\"module\" src=\"/src/main.ts\"></script></body></html>",
    )
    .map_err(|err| ZerhoError::Io {
        path: Some(dir.join("index.html")),
        reason: err.to_string(),
    })?;

    let leaf_count = total_modules.saturating_sub(1);
    let mut entry_src = String::with_capacity(leaf_count * 48 + 128);
    for i in 0..leaf_count {
        let mod_path = src_dir.join(format!("mod_{i}.ts"));
        let mod_code = format!(
            "export const value_{i}: number = {};\n\
             export function compute_{i}(x: number): number {{ return x + value_{i}; }}\n\
             export const dead_export_{i}: string = \"unused_symbol_{i}\";\n",
            i + 1
        );
        fs::write(&mod_path, mod_code).map_err(|err| ZerhoError::Io {
            path: Some(mod_path),
            reason: err.to_string(),
        })?;
        let _ = writeln!(entry_src, "import {{ compute_{i} }} from './mod_{i}';");
    }

    entry_src.push_str("let total = 0;\n");
    for i in 0..leaf_count {
        let _ = writeln!(entry_src, "total = compute_{i}(total);");
    }
    entry_src.push_str("console.log(total);\n");

    let entry_path = src_dir.join("main.ts");
    fs::write(&entry_path, entry_src).map_err(|err| ZerhoError::Io {
        path: Some(entry_path),
        reason: err.to_string(),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_benchmark_runs_and_emits_structured_json() {
        let result = execute_bench(&ZerhoConfig::default(), 2, Some(15))
            .expect("synthetic benchmark should succeed");
        assert_eq!(result.module_count, 15);
        assert_eq!(result.tree_shaken_exports, 14);
        assert!(result.scope_hoisted);

        let json = result.to_json();
        assert!(json.contains("\"environment\""));
        assert!(json.contains("\"synthetic-15-modules\""));
        assert!(json.contains("\"cold_build_ms\""));
        assert!(json.contains("\"warm_build_min_ms\""));
        assert!(json.contains("\"throughput_mods_per_sec\""));
    }
}
