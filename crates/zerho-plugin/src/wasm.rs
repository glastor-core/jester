use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wasmtime::{Config, Engine, Instance, Module, Store};
use zerho_core::{ContentHash, ModuleKind, ZerhoError, ZerhoResult};

use crate::{Plugin, PluginContext, PluginRegistry, PluginTransformResult};

const WASM_MAGIC: &[u8; 4] = b"\0asm";
const DEFAULT_FUEL_LIMIT: u64 = 1_000_000;

/// Metadata for a validated WebAssembly plugin (`zerho:plugin@0.1.0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmPluginDescriptor {
    pub name: String,
    pub path: PathBuf,
    pub hash: ContentHash,
    pub byte_len: usize,
    pub sandboxed: bool,
}

impl WasmPluginDescriptor {
    /// Reads and validates a `.wasm` plugin binary (magic header + version check).
    pub fn from_file(name: impl Into<String>, path: impl AsRef<Path>) -> ZerhoResult<Self> {
        let path = path.as_ref().to_path_buf();
        let bytes = fs::read(&path).map_err(|err| ZerhoError::Io {
            path: Some(path.clone()),
            reason: err.to_string(),
        })?;
        Self::from_bytes(name, path, &bytes)
    }

    /// Validates raw WebAssembly binary bytes against the WASM binary header.
    pub fn from_bytes(
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        bytes: &[u8],
    ) -> ZerhoResult<Self> {
        let name = name.into();
        if bytes.len() < 8 || &bytes[..4] != WASM_MAGIC {
            return Err(ZerhoError::Plugin {
                plugin: name,
                reason: "invalid WebAssembly binary (missing `\\0asm` header)".to_owned(),
            });
        }
        Ok(Self {
            name,
            path: path.into(),
            hash: ContentHash::from_bytes(bytes),
            byte_len: bytes.len(),
            sandboxed: true,
        })
    }
}

/// Fuel-metered, sandboxed WebAssembly plugin runtime powered by `wasmtime`.
pub struct WasmPluginHost {
    engine: Engine,
    fuel_limit: u64,
}

impl std::fmt::Debug for WasmPluginHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPluginHost")
            .field("fuel_limit", &self.fuel_limit)
            .finish_non_exhaustive()
    }
}

impl WasmPluginHost {
    /// Creates a new sandboxed `wasmtime` engine with fuel metering enabled to prevent infinite loops.
    pub fn new() -> ZerhoResult<Self> {
        Self::with_fuel(DEFAULT_FUEL_LIMIT)
    }

    /// Creates a new sandboxed `wasmtime` engine with a custom fuel budget per invocation.
    pub fn with_fuel(fuel_limit: u64) -> ZerhoResult<Self> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|err| ZerhoError::Plugin {
            plugin: "wasmtime".to_owned(),
            reason: err.to_string(),
        })?;
        Ok(Self {
            engine,
            fuel_limit: fuel_limit.max(1),
        })
    }

    /// Reads, validates, and compiles a `.wasm` plugin file from disk.
    pub fn load_file(&self, path: &Path) -> ZerhoResult<CompiledWasmPlugin> {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("wasm-plugin")
            .to_owned();
        let bytes = fs::read(path).map_err(|err| ZerhoError::Io {
            path: Some(path.to_path_buf()),
            reason: err.to_string(),
        })?;
        self.load_bytes(name, path, &bytes)
    }

    /// Compiles and validates a `.wasm` plugin binary into a reusable [`CompiledWasmPlugin`].
    pub fn load_bytes(
        &self,
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        bytes: &[u8],
    ) -> ZerhoResult<CompiledWasmPlugin> {
        let name = name.into();
        let descriptor = WasmPluginDescriptor::from_bytes(&name, path, bytes)?;
        let module = Module::new(&self.engine, bytes).map_err(|err| ZerhoError::Plugin {
            plugin: name.clone(),
            reason: format!("wasm compilation failed: {err}"),
        })?;
        Ok(CompiledWasmPlugin { descriptor, module })
    }

    /// Executes a zero-argument `() -> i32` exported hook (e.g. `zerho_abi_version`) inside a fuel-capped sandbox.
    pub fn call_i32_export(
        &self,
        plugin: &CompiledWasmPlugin,
        export_name: &str,
    ) -> ZerhoResult<i32> {
        let mut store = Store::new(&self.engine, ());
        store
            .set_fuel(self.fuel_limit)
            .map_err(|err| ZerhoError::Plugin {
                plugin: plugin.descriptor.name.clone(),
                reason: err.to_string(),
            })?;

        let instance =
            Instance::new(&mut store, &plugin.module, &[]).map_err(|err| ZerhoError::Plugin {
                plugin: plugin.descriptor.name.clone(),
                reason: format!("wasm instantiation failed: {err}"),
            })?;

        let func = instance
            .get_typed_func::<(), i32>(&mut store, export_name)
            .map_err(|err| ZerhoError::Plugin {
                plugin: plugin.descriptor.name.clone(),
                reason: format!("missing export `{export_name}`: {err}"),
            })?;

        func.call(&mut store, ()).map_err(|err| ZerhoError::Plugin {
            plugin: plugin.descriptor.name.clone(),
            reason: format!("wasm execution trapped: {err}"),
        })
    }

    /// Passes a UTF-8 string into guest linear memory via `zerho_alloc(len) -> ptr`, invokes
    /// `<hook_name>(ptr, len) -> i64` (returning packed `(out_ptr << 32) | out_len`), and reads
    /// back the transformed UTF-8 string inside a fuel-capped sandbox.
    ///
    /// Returns `Ok(None)` if the guest does not export `memory`/`zerho_alloc`/`<hook_name>` or returns `0`.
    pub fn call_string_hook(
        &self,
        plugin: &CompiledWasmPlugin,
        hook_name: &str,
        input: &str,
    ) -> ZerhoResult<Option<String>> {
        let mut store = Store::new(&self.engine, ());
        store
            .set_fuel(self.fuel_limit)
            .map_err(|err| ZerhoError::Plugin {
                plugin: plugin.descriptor.name.clone(),
                reason: err.to_string(),
            })?;

        let instance =
            Instance::new(&mut store, &plugin.module, &[]).map_err(|err| ZerhoError::Plugin {
                plugin: plugin.descriptor.name.clone(),
                reason: format!("wasm instantiation failed: {err}"),
            })?;

        let Some(memory) = instance.get_memory(&mut store, "memory") else {
            return Ok(None);
        };
        let Ok(alloc_fn) = instance.get_typed_func::<i32, i32>(&mut store, "zerho_alloc") else {
            return Ok(None);
        };
        let Ok(hook_fn) = instance.get_typed_func::<(i32, i32), i64>(&mut store, hook_name) else {
            return Ok(None);
        };

        let in_bytes = input.as_bytes();
        let in_len = i32::try_from(in_bytes.len()).map_err(|_| ZerhoError::Plugin {
            plugin: plugin.descriptor.name.clone(),
            reason: "input exceeds i32 length for WASM linear memory".to_owned(),
        })?;
        let in_ptr = alloc_fn
            .call(&mut store, in_len)
            .map_err(|err| ZerhoError::Plugin {
                plugin: plugin.descriptor.name.clone(),
                reason: format!("zerho_alloc trapped: {err}"),
            })?;
        let in_offset = usize::try_from(in_ptr).map_err(|_| ZerhoError::Plugin {
            plugin: plugin.descriptor.name.clone(),
            reason: "zerho_alloc returned negative pointer".to_owned(),
        })?;
        memory
            .write(&mut store, in_offset, in_bytes)
            .map_err(|err| ZerhoError::Plugin {
                plugin: plugin.descriptor.name.clone(),
                reason: format!("wasm memory write failed: {err}"),
            })?;

        let packed =
            hook_fn
                .call(&mut store, (in_ptr, in_len))
                .map_err(|err| ZerhoError::Plugin {
                    plugin: plugin.descriptor.name.clone(),
                    reason: format!("`{hook_name}` trapped: {err}"),
                })?;
        if packed == 0 {
            return Ok(None);
        }

        let packed_u64 = packed.cast_unsigned();
        let out_ptr = usize::try_from(packed_u64 >> 32).unwrap_or(0);
        let out_len = usize::try_from(packed_u64 & 0xFFFF_FFFF).unwrap_or(0);
        if out_len == 0 {
            return Ok(None);
        }

        let mut out_buf = vec![0u8; out_len];
        memory
            .read(&store, out_ptr, &mut out_buf)
            .map_err(|err| ZerhoError::Plugin {
                plugin: plugin.descriptor.name.clone(),
                reason: format!("wasm memory read failed: {err}"),
            })?;

        let decoded = String::from_utf8(out_buf).map_err(|err| ZerhoError::Plugin {
            plugin: plugin.descriptor.name.clone(),
            reason: format!("`{hook_name}` returned invalid UTF-8: {err}"),
        })?;
        Ok(Some(decoded))
    }
}

/// A compiled, ready-to-instantiate WebAssembly plugin module.
pub struct CompiledWasmPlugin {
    pub descriptor: WasmPluginDescriptor,
    module: Module,
}

impl std::fmt::Debug for CompiledWasmPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledWasmPlugin")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

/// Adapter bridging a compiled `wasmtime` plugin into [`PluginRegistry`].
#[derive(Debug)]
pub struct WasmPluginAdapter {
    host: Arc<WasmPluginHost>,
    compiled: CompiledWasmPlugin,
}

impl WasmPluginAdapter {
    pub fn new(host: Arc<WasmPluginHost>, compiled: CompiledWasmPlugin) -> ZerhoResult<Self> {
        // Verify optional `zerho_abi_version` export in the fuel-capped sandbox on load.
        let _ = host.call_i32_export(&compiled, "zerho_abi_version");
        Ok(Self { host, compiled })
    }
}

impl Plugin for WasmPluginAdapter {
    fn name(&self) -> &str {
        &self.compiled.descriptor.name
    }

    fn resolve_id(&self, specifier: &str, _importer: Option<&Path>) -> ZerhoResult<Option<String>> {
        self.host
            .call_string_hook(&self.compiled, "zerho_resolve_id", specifier)
    }

    fn load(&self, id: &str, _ctx: &PluginContext) -> ZerhoResult<Option<String>> {
        self.host.call_string_hook(&self.compiled, "zerho_load", id)
    }

    fn transform(
        &self,
        code: &str,
        _id: &str,
        _kind: ModuleKind,
        _ctx: &PluginContext,
    ) -> ZerhoResult<Option<PluginTransformResult>> {
        let Some(transformed) =
            self.host
                .call_string_hook(&self.compiled, "zerho_transform", code)?
        else {
            return Ok(None);
        };
        Ok(Some(PluginTransformResult {
            code: transformed,
            sourcemap: None,
        }))
    }
}

/// Loads all configured `.wasm` plugins relative to `root` and registers them into `registry`.
pub fn register_wasm_plugins(
    registry: &mut PluginRegistry,
    root: &Path,
    wasm_paths: &[PathBuf],
    fuel_limit: u64,
) -> ZerhoResult<usize> {
    if wasm_paths.is_empty() {
        return Ok(0);
    }
    let host = Arc::new(WasmPluginHost::with_fuel(fuel_limit)?);
    let mut loaded = 0usize;
    for rel in wasm_paths {
        let full = if rel.is_absolute() {
            rel.clone()
        } else {
            root.join(rel)
        };
        let compiled = host.load_file(&full)?;
        registry.register(WasmPluginAdapter::new(Arc::clone(&host), compiled)?);
        loaded += 1;
    }
    Ok(loaded)
}

/// Returns the canonical WIT interface definition (`zerho:plugin@0.1.0`).
pub const fn wit_contract() -> &'static str {
    include_str!("../wit/zerho-plugin.wit")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid WebAssembly module exporting `(func (export "zerho_abi_version") (result i32) (i32.const 1))`.
    const MINIMAL_ABI_WASM: &[u8] = &[
        0x00, 0x61, 0x73, 0x6d, // \0asm
        0x01, 0x00, 0x00, 0x00, // version 1
        // Type section: 1 func () -> i32
        0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, // Function section: func 0 has type 0
        0x03, 0x02, 0x01, 0x00, // Export section: export "zerho_abi_version" -> func 0
        0x07, 0x15, 0x01, 0x11, b'z', b'e', b'r', b'h', b'o', b'_', b'a', b'b', b'i', b'_', b'v',
        b'e', b'r', b's', b'i', b'o', b'n', 0x00, 0x00,
        // Code section: i32.const 1; end
        0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x01, 0x0b,
    ];

    #[test]
    fn executes_real_wasm_plugin_in_wasmtime_sandbox() {
        let host = WasmPluginHost::new().expect("wasmtime host should initialize");
        let plugin = host
            .load_bytes("demo-wasm", "plugins/demo.wasm", MINIMAL_ABI_WASM)
            .expect("wasm module should compile");

        let abi_version = host
            .call_i32_export(&plugin, "zerho_abi_version")
            .expect("wasm export should execute");
        assert_eq!(abi_version, 1);
    }

    #[test]
    fn rejects_invalid_wasm_header() {
        let invalid = b"not-wasm-bytes";
        assert!(WasmPluginDescriptor::from_bytes("bad", "bad.wasm", invalid).is_err());
    }

    #[test]
    fn embeds_wit_contract() {
        assert!(wit_contract().contains("package zerho:plugin@0.1.0;"));
        assert!(wit_contract().contains("world zerho-plugin"));
    }

    #[test]
    fn passes_and_returns_utf8_string_across_wasm_linear_memory() {
        #[rustfmt::skip]
        const STRING_TRANSFORM_WASM: &[u8] = &[
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
            // Type section: (i32)->i32 and (i32, i32)->i64 (12 = 0x0c bytes)
            0x01, 0x0c, 0x02,
            0x60, 0x01, 0x7f, 0x01, 0x7f,
            0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7e,
            // Function section
            0x03, 0x03, 0x02, 0x00, 0x01,
            // Memory section (1 page)
            0x05, 0x03, 0x01, 0x00, 0x01,
            // Export section (42 = 0x2a bytes)
            0x07, 0x2a, 0x03,
            0x06, b'm', b'e', b'm', b'o', b'r', b'y', 0x02, 0x00,
            0x0b, b'z', b'e', b'r', b'h', b'o', b'_', b'a', b'l', b'l', b'o', b'c', 0x00, 0x00,
            0x0f, b'z', b'e', b'r', b'h', b'o', b'_', b't', b'r', b'a', b'n', b's', b'f', b'o', b'r', b'm', 0x00, 0x01,
            // Code section (27 = 0x1b bytes)
            0x0a, 0x1b, 0x02,
            // Body 0 (zerho_alloc): return 16
            0x04, 0x00, 0x41, 0x10, 0x0b,
            // Body 1 (zerho_transform): mem[ptr] = 'Z' (90 = 0xda 0x00), return (ptr << 32) | len
            0x14, 0x00,
            0x20, 0x00, 0x41, 0xda, 0x00, 0x3a, 0x00, 0x00,
            0x20, 0x00, 0xad, 0x42, 0x20, 0x86,
            0x20, 0x01, 0xad, 0x84,
            0x0b,
        ];

        let host = WasmPluginHost::new().expect("wasmtime host should initialize");
        let compiled = host
            .load_bytes(
                "string-plugin",
                "plugins/string.wasm",
                STRING_TRANSFORM_WASM,
            )
            .expect("string transform wasm should compile");
        let out = host
            .call_string_hook(&compiled, "zerho_transform", "abc")
            .expect("string hook should succeed");
        assert_eq!(out.as_deref(), Some("Zbc"));
    }
}
