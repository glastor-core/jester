mod scaffold;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use scaffold::{Template, scaffold_project};
use zerho_bundler::Bundler;
use zerho_config::ZerhoConfig;
use zerho_core::{ModuleKind, ZerhoError, ZerhoResult};
use zerho_graph::{CachedArtifact, DiskCache};
use zerho_server::{DevServer, PreviewServer};
use zerho_transform::{TransformInput, TransformPipeline};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &[String]) -> ZerhoResult<()> {
    let Some(cmd) = args.first().map(String::as_str) else {
        print_help();
        return Ok(());
    };

    match cmd {
        "-h" | "--help" | "help" => {
            print_help();
            Ok(())
        }
        "-V" | "--version" | "version" => {
            println!("zerho {VERSION}");
            Ok(())
        }
        "schema" => {
            println!("{}", ZerhoConfig::json_schema());
            Ok(())
        }
        "wit" => {
            println!("{}", zerho_plugin::wit_contract());
            Ok(())
        }
        "init" => cmd_init(&args[1..]),
        "dev" => cmd_dev(&args[1..]).await,
        "build" => cmd_build(&args[1..]),
        "preview" => cmd_preview(&args[1..]).await,
        "bench" => cmd_bench(&args[1..]),
        other => Err(ZerhoError::Config(format!(
            "unknown command `{other}` (run `zerho --help` for usage)"
        ))),
    }
}

fn cmd_init(flags: &[String]) -> ZerhoResult<()> {
    let template = match find_flag_value(flags, "--template") {
        Some(raw) => Template::parse(raw)?,
        None => Template::default(),
    };
    let target_dir = find_positional_arg(flags)
        .map_or_else(|| PathBuf::from("."), PathBuf::from);

    let created = scaffold_project(&target_dir, template)?;
    println!(
        "zerho init complete ({} template, {} files) -> {}",
        template.as_str(),
        created.len(),
        target_dir.display()
    );
    if target_dir.as_os_str() != "." {
        println!("  cd {}", target_dir.display());
    }
    println!("  zerho dev");
    Ok(())
}

async fn cmd_dev(flags: &[String]) -> ZerhoResult<()> {
    let mut config = load_or_default(find_flag_value(flags, "--config"))?;
    if let Some(port_raw) = find_flag_value(flags, "--port") {
        config.server.port = port_raw.parse::<u16>().map_err(|_| {
            ZerhoError::Config(format!("invalid `--port` value `{port_raw}`"))
        })?;
    }
    let server = DevServer::new(config);
    server.serve().await
}

fn cmd_build(flags: &[String]) -> ZerhoResult<()> {
    let config = load_or_default(find_flag_value(flags, "--config"))?;
    let mut bundler = Bundler::new(config);
    let output = bundler.build()?;
    println!(
        "zerho build complete ({} modules, {} dead exports shaken) -> {}",
        output.module_count,
        output.tree_shaken_exports,
        output.out_dir.display()
    );
    for chunk in output.chunks {
        let raw_kb = format!("{}.{:02} KB", chunk.bytes / 1024, (chunk.bytes % 1024) * 100 / 1024);
        let gz_kb = format!(
            "{}.{:02} KB",
            chunk.gzip_bytes / 1024,
            (chunk.gzip_bytes % 1024) * 100 / 1024
        );
        let sri_tag = chunk
            .integrity
            .as_ref()
            .map_or_else(String::new, |s| format!("  [{}..]", &s[..15.min(s.len())]));
        match chunk.sourcemap_file {
            Some(map) => {
                println!("  {}  {raw_kb} | gzip: {gz_kb}  (+ {map}){sri_tag}", chunk.file_name);
            }
            None => println!("  {}  {raw_kb} | gzip: {gz_kb}{sri_tag}", chunk.file_name),
        }
    }
    Ok(())
}

async fn cmd_preview(flags: &[String]) -> ZerhoResult<()> {
    let mut config = load_or_default(find_flag_value(flags, "--config"))?;
    if let Some(port_raw) = find_flag_value(flags, "--port") {
        config.server.port = port_raw.parse::<u16>().map_err(|_| {
            ZerhoError::Config(format!("invalid `--port` value `{port_raw}`"))
        })?;
    }
    let server = PreviewServer::new(config);
    server.serve().await
}

fn cmd_bench(flags: &[String]) -> ZerhoResult<()> {
    let config = load_or_default(find_flag_value(flags, "--config"))?;
    let iterations: usize = find_flag_value(flags, "--iterations")
        .map_or(Ok(10), |raw| {
            raw.parse::<usize>()
                .map_err(|_| ZerhoError::Config(format!("invalid `--iterations` value `{raw}`")))
        })?
        .max(1);

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
    let cold_duration = t_cold.elapsed();

    // 2. Persistent DiskCache hit benchmark (blake3)
    let cache_dir = root.join(&config.cache.dir);
    let cache = DiskCache::new(cache_dir, true);
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
    let avg_cache = t_cache.elapsed() / u32::try_from(iterations).unwrap_or(1);

    // 3. Full production build benchmark (Oxc + LightningCSS + Bundler + Disk I/O)
    let mut build_times = Vec::with_capacity(iterations);
    let mut module_count = 0;
    for _ in 0..iterations {
        let mut bundler = Bundler::new(config.clone());
        let start = Instant::now();
        let out = bundler.build()?;
        build_times.push(start.elapsed());
        module_count = out.module_count;
    }
    build_times.sort();
    let min_build = build_times[0];
    let total_build: Duration = build_times.iter().copied().sum();
    let avg_build = total_build / u32::try_from(iterations).unwrap_or(1);
    let p95_idx = ((iterations * 95) / 100).min(iterations - 1);
    let p95_build = build_times[p95_idx];

    println!("zerho benchmark — {} ({} iterations)", config.project.name, iterations);
    println!("  cold transform (oxc):          {:.2} ms", cold_duration.as_secs_f64() * 1000.0);
    println!("  warm disk cache hit (blake3):  {:.3} ms", avg_cache.as_secs_f64() * 1000.0);
    println!(
        "  production build ({module_count} mods):     min {:.2} ms | avg {:.2} ms | p95 {:.2} ms",
        min_build.as_secs_f64() * 1000.0,
        avg_build.as_secs_f64() * 1000.0,
        p95_build.as_secs_f64() * 1000.0,
    );
    Ok(())
}

fn load_or_default(config_path: Option<&str>) -> ZerhoResult<ZerhoConfig> {
    let path = PathBuf::from(config_path.unwrap_or("zerho.toml"));
    if path.exists() || config_path.is_some() {
        let mut config = ZerhoConfig::load(&path)?;
        // Resolve relative `project.root` against the directory containing `zerho.toml`.
        if config.project.root.is_relative()
            && let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            config.project.root = parent.join(&config.project.root);
        }
        Ok(config)
    } else {
        Ok(ZerhoConfig::default())
    }
}

fn find_positional_arg(flags: &[String]) -> Option<&str> {
    let mut skip_next = false;
    for arg in flags {
        if skip_next {
            skip_next = false;
            continue;
        }
        if matches!(arg.as_str(), "--template" | "--config" | "--port" | "--iterations") {
            skip_next = true;
            continue;
        }
        if !arg.starts_with('-') {
            return Some(arg.as_str());
        }
    }
    None
}

fn find_flag_value<'a>(flags: &'a [String], name: &str) -> Option<&'a str> {
    let mut iter = flags.iter();
    while let Some(arg) = iter.next() {
        if arg == name {
            return iter.next().map(String::as_str);
        }
        if let Some(val) = arg.strip_prefix(&format!("{name}=")) {
            return Some(val);
        }
    }
    None
}

fn print_help() {
    println!(
        "zerho {VERSION} — Zero-Node, 100% Rust frontend dev server & bundler\n\n\
         USAGE:\n\
           zerho <COMMAND> [OPTIONS]\n\n\
         COMMANDS:\n\
           dev       Start the native dev server with HMR and /__inspect UI\n\
           build     Build production bundle (minified JS/CSS + manifest.json)\n\
           preview   Preview the production build locally\n\
           bench     Run reproducible transform, cache, and build benchmarks\n\
           init [DIR]  Scaffold a Zero-Node project (--template basic-ts | react-tsx)\n\
           schema      Print the JSON Schema for zerho.toml\n\
           wit         Print the WASM Component Model plugin contract (zerho:plugin@0.1.0)\n\
           help        Show this help message\n\n\
         OPTIONS:\n\
           --template <NAME>     Template for `zerho init` (basic-ts | react-tsx)\n\
           --config <PATH>       Path to zerho.toml (default: ./zerho.toml)\n\
           --port <PORT>         Override dev/preview server port\n\
           --iterations <N>      Number of iterations for `zerho bench` (default: 10)\n\
           -V, --version         Print version"
    );
}
