pub mod wasm;

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use zerho_core::{ModuleKind, ZerhoResult};

pub use wasm::{CompiledWasmPlugin, WasmPluginDescriptor, WasmPluginHost, wit_contract};

/// Context passed to plugins during resolve/load/transform hooks.
#[derive(Debug, Clone)]
pub struct PluginContext {
    pub is_dev: bool,
}

/// Result returned when a plugin transforms module source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginTransformResult {
    pub code: String,
    pub sourcemap: Option<String>,
}

/// Native Rust plugin interface (mirrors `zerho:plugin@0.1.0` WIT hooks).
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;

    fn resolve_id(&self, _specifier: &str, _importer: Option<&Path>) -> ZerhoResult<Option<String>> {
        Ok(None)
    }

    fn load(&self, _id: &str, _ctx: &PluginContext) -> ZerhoResult<Option<String>> {
        Ok(None)
    }

    fn transform(
        &self,
        _code: &str,
        _id: &str,
        _kind: ModuleKind,
        _ctx: &PluginContext,
    ) -> ZerhoResult<Option<PluginTransformResult>> {
        Ok(None)
    }
}

/// Built-in zero-overhead compile-time constant replacement plugin (e.g. `import.meta.env.MODE`).
#[derive(Debug, Clone, Default)]
pub struct DefinePlugin {
    replacements: BTreeMap<String, String>,
}

impl DefinePlugin {
    pub fn new(replacements: BTreeMap<String, String>) -> Self {
        Self { replacements }
    }
}

impl Plugin for DefinePlugin {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "zerho:define"
    }

    fn transform(
        &self,
        code: &str,
        _id: &str,
        kind: ModuleKind,
        _ctx: &PluginContext,
    ) -> ZerhoResult<Option<PluginTransformResult>> {
        if !kind.is_script() || self.replacements.is_empty() {
            return Ok(None);
        }
        let mut updated = code.to_owned();
        let mut changed = false;
        for (key, value) in &self.replacements {
            if updated.contains(key) {
                updated = updated.replace(key, value);
                changed = true;
            }
        }
        if changed {
            Ok(Some(PluginTransformResult { code: updated, sourcemap: None }))
        } else {
            Ok(None)
        }
    }
}

/// Ordered registry of native and WASM plugins.
#[derive(Default)]
pub struct PluginRegistry {
    plugins: Vec<Box<dyn Plugin>>,
    wasm_descriptors: Vec<WasmPluginDescriptor>,
}

impl fmt::Debug for PluginRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.plugins.iter().map(|p| p.name()).collect();
        f.debug_struct("PluginRegistry")
            .field("plugins", &names)
            .field("wasm_plugins", &self.wasm_descriptors.len())
            .finish()
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, plugin: impl Plugin + 'static) {
        self.plugins.push(Box::new(plugin));
    }

    pub fn register_wasm(&mut self, descriptor: WasmPluginDescriptor) {
        self.wasm_descriptors.push(descriptor);
    }

    pub fn len(&self) -> usize {
        self.plugins.len() + self.wasm_descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty() && self.wasm_descriptors.is_empty()
    }

    pub fn run_transform(
        &self,
        initial_code: String,
        id: &str,
        kind: ModuleKind,
        ctx: &PluginContext,
    ) -> ZerhoResult<PluginTransformResult> {
        let mut current = PluginTransformResult { code: initial_code, sourcemap: None };
        for plugin in &self.plugins {
            if let Some(updated) = plugin.transform(&current.code, id, kind, ctx)? {
                current = updated;
            }
        }
        Ok(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn define_plugin_replaces_compile_time_constants() {
        let mut defs = BTreeMap::new();
        defs.insert("__DEV__".to_owned(), "true".to_owned());
        let mut registry = PluginRegistry::new();
        registry.register(DefinePlugin::new(defs));

        let res = registry
            .run_transform(
                "if (__DEV__) console.log('dev');".to_owned(),
                "src/main.ts",
                ModuleKind::Ts,
                &PluginContext { is_dev: true },
            )
            .expect("plugin transform should succeed");
        assert_eq!(res.code, "if (true) console.log('dev');");
    }
}
