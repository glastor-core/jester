mod bench;
mod scaffold;

use std::env;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use bench::execute_bench;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use scaffold::{Template, scaffold_project};
use zerho_bundler::{Bundler, run_project_check};
use zerho_config::ZerhoConfig;
use zerho_core::{ZerhoError, ZerhoResult};
use zerho_server::{DevServer, PreviewServer};

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
        "check" => cmd_check(&args[1..]),
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
    let mut config = load_or_default(find_flag_value(flags, "--config"))?;
    if flags.iter().any(|f| f == "--analyze") {
        config.build.analyze = true;
    }
    let watch = flags.iter().any(|f| f == "--watch");
    run_single_build(&config)?;
    if watch {
        run_build_watch(&config)?;
    }
    Ok(())
}

fn run_single_build(config: &ZerhoConfig) -> ZerhoResult<()> {
    let mut bundler = Bundler::new(config.clone());
    let output = bundler.build()?;
    let mode_tag = if output.scope_hoisted {
        "scope-hoisted"
    } else {
        "registry"
    };
    let barrel_tag = if output.barrel_imports_bypassed > 0 {
        format!(", {} barrels bypassed", output.barrel_imports_bypassed)
    } else {
        String::new()
    };
    println!(
        "zerho build complete ({} modules, {} dead exports shaken{barrel_tag}, cache: {} hits/{} misses, {mode_tag}) -> {}",
        output.module_count,
        output.tree_shaken_exports,
        output.cache_hits,
        output.cache_misses,
        output.out_dir.display()
    );
    for chunk in output.chunks {
        let raw_kb = format!("{}.{:02} KB", chunk.bytes / 1024, (chunk.bytes % 1024) * 100 / 1024);
        let gz_kb = format!(
            "{}.{:02} KB",
            chunk.gzip_bytes / 1024,
            (chunk.gzip_bytes % 1024) * 100 / 1024
        );
        let br_kb = format!(
            "{}.{:02} KB",
            chunk.brotli_bytes / 1024,
            (chunk.brotli_bytes % 1024) * 100 / 1024
        );
        let sri_tag = chunk
            .integrity
            .as_ref()
            .map_or_else(String::new, |s| format!("  [{}..]", &s[..15.min(s.len())]));
        match chunk.sourcemap_file {
            Some(map) => {
                println!(
                    "  {}  {raw_kb} | gzip: {gz_kb} | br: {br_kb}  (+ {map}){sri_tag}",
                    chunk.file_name
                );
            }
            None => println!(
                "  {}  {raw_kb} | gzip: {gz_kb} | br: {br_kb}{sri_tag}",
                chunk.file_name
            ),
        }
    }
    if let Some(stats_path) = output.stats_file {
        println!("  bundle analyzer -> {}", stats_path.display());
    }
    Ok(())
}

fn run_build_watch(config: &ZerhoConfig) -> ZerhoResult<()> {
    let root = config
        .project
        .root
        .canonicalize()
        .unwrap_or_else(|_| config.project.root.clone());
    let (tx, rx) = mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        let Ok(event) = res else { return };
        if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
            return;
        }
        if event.paths.iter().any(|p| !is_watch_ignored(p) && !p.is_dir()) {
            let _ = tx.send(());
        }
    })
    .map_err(|err| ZerhoError::Io {
        path: Some(root.clone()),
        reason: format!("failed to start build watcher: {err}"),
    })?;

    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|err| ZerhoError::Io {
            path: Some(root.clone()),
            reason: format!("failed to watch `{}`: {err}", root.display()),
        })?;

    println!("zerho build --watch listening for changes in {} (Ctrl+C to stop)...", root.display());
    while rx.recv().is_ok() {
        // Coalesce rapid editor save bursts.
        while rx.recv_timeout(Duration::from_millis(60)).is_ok() {}
        let t0 = Instant::now();
        match run_single_build(config) {
            Ok(()) => println!("  rebuilt in {} ms", t0.elapsed().as_millis()),
            Err(err) => eprintln!("  rebuild error: {err}"),
        }
    }
    Ok(())
}

fn is_watch_ignored(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c,
            Component::Normal(name)
                if name == ".git" || name == "target" || name == "dist" || name == ".zerho" || name == "node_modules"
        )
    })
}

fn cmd_check(flags: &[String]) -> ZerhoResult<()> {
    let config = load_or_default(find_flag_value(flags, "--config"))?;
    let strict = flags.iter().any(|f| f == "--strict");
    let report = run_project_check(&config)?;

    for issue in &report.issues {
        println!(
            "  [{}] ({}) {} — {}",
            issue.severity.as_str(),
            issue.code,
            issue.file,
            issue.message
        );
    }

    let errors = report.error_count();
    let warnings = report.warning_count();
    println!(
        "zerho check finished in {} ms ({} modules, {} cycles, {} errors, {} warnings)",
        report.elapsed_ms,
        report.modules_checked,
        report.cycles.len(),
        errors,
        warnings
    );

    if errors > 0 || (strict && warnings > 0) {
        return Err(ZerhoError::Config(format!(
            "zerho check failed with {errors} error(s) and {warnings} warning(s)"
        )));
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
    let synthetic: Option<usize> = match find_flag_value(flags, "--synthetic") {
        Some(raw) => Some(raw.parse::<usize>().map_err(|_| {
            ZerhoError::Config(format!("invalid `--synthetic` module count `{raw}`"))
        })?),
        None => None,
    };
    let json_output = flags.iter().any(|f| f == "--json");

    let result = execute_bench(&config, iterations, synthetic)?;
    if json_output {
        println!("{}", result.to_json());
    } else {
        result.print_human();
    }
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
        if matches!(
            arg.as_str(),
            "--template" | "--config" | "--port" | "--iterations" | "--synthetic"
        ) {
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
           build     Build production bundle (scope-hoisted JS/CSS, Brotli, SRI, manifest)\n\
           check     Audit module graph for broken imports, cycles, missing .env, and secrets\n\
           preview   Preview the production build locally (serves pre-compressed .br)\n\
           bench     Run reproducible transform, cache, and build benchmarks\n\
           init [DIR]  Scaffold a Zero-Node project (--template basic-ts | react-tsx)\n\
           schema      Print the JSON Schema for zerho.toml\n\
           wit         Print the WASM Component Model plugin contract (zerho:plugin@0.1.0)\n\
           help        Show this help message\n\n\
         OPTIONS:\n\
           --template <NAME>     Template for `zerho init` (basic-ts | react-tsx)\n\
           --config <PATH>       Path to zerho.toml (default: ./zerho.toml)\n\
           --port <PORT>         Override dev/preview server port\n\
           --analyze             Emit interactive dist/stats.html Bundle Analyzer on build\n\
           --watch               Watch source files and rebuild incrementally on `zerho build`\n\
           --strict              Fail `zerho check` on warnings as well as errors\n\
           --iterations <N>      Number of iterations for `zerho bench` (default: 10)\n\
           --synthetic <N>       Generate & benchmark a deterministic N-module TS graph\n\
           --json                Emit machine-readable JSON output for `zerho bench`\n\
           -V, --version         Print version"
    );
}
