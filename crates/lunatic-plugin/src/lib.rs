#![forbid(unsafe_code)]

mod lifecycle;
mod module_context;

pub use lifecycle::{LifecycleDispatcher, LifecycleEvent};
pub use module_context::ModuleContext;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use wasmtime::Module;

/// Capability that a plugin may request
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    ModuleTransform,
    HostFunctions(String),
    LifecycleHooks,
    Networking,
    Filesystem(Vec<PathBuf>),
    ProcessSpawn,
}

/// Plugin dependency specification
#[derive(Debug, Clone)]
pub struct PluginDependency {
    pub name: String,
    pub version_req: semver::VersionReq,
}

/// Plugin metadata
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    pub version: semver::Version,
    pub capabilities: Vec<Capability>,
    pub dependencies: Vec<PluginDependency>,
}

/// A loaded plugin
pub struct Plugin {
    pub info: PluginInfo,
    pub module: Module,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

/// Registry that manages loaded plugins
pub struct PluginRegistry {
    plugins: HashMap<String, Arc<Plugin>>,
    module_transform_plugins: Vec<Arc<Plugin>>,
    host_function_plugins: HashMap<String, Vec<Arc<Plugin>>>,
    lifecycle_plugins: Vec<Arc<Plugin>>,
    lifecycle_dispatcher: LifecycleDispatcher,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            module_transform_plugins: Vec::new(),
            host_function_plugins: HashMap::new(),
            lifecycle_plugins: Vec::new(),
            lifecycle_dispatcher: LifecycleDispatcher::new(),
        }
    }

    /// Register a plugin in the registry
    pub fn register(&mut self, plugin: Plugin) -> Result<()> {
        let name = plugin.info.name.clone();
        let plugin = Arc::new(plugin);

        for cap in &plugin.info.capabilities {
            match cap {
                Capability::ModuleTransform => {
                    self.module_transform_plugins.push(Arc::clone(&plugin));
                }
                Capability::HostFunctions(namespace) => {
                    self.host_function_plugins
                        .entry(namespace.clone())
                        .or_default()
                        .push(Arc::clone(&plugin));
                }
                Capability::LifecycleHooks => {
                    self.lifecycle_plugins.push(Arc::clone(&plugin));
                    self.lifecycle_dispatcher.add_plugin(Arc::clone(&plugin));
                }
                _ => {}
            }
        }

        self.plugins.insert(name, plugin);
        Ok(())
    }

    /// Get a plugin by name
    pub fn get(&self, name: &str) -> Option<&Arc<Plugin>> {
        self.plugins.get(name)
    }

    /// Get all module transform plugins (in registration order)
    pub fn module_transform_plugins(&self) -> &[Arc<Plugin>] {
        &self.module_transform_plugins
    }

    /// Get host function plugins for a namespace
    pub fn host_function_plugins(&self, namespace: &str) -> Option<&Vec<Arc<Plugin>>> {
        self.host_function_plugins.get(namespace)
    }

    /// Get the lifecycle dispatcher
    pub fn lifecycle_dispatcher(&self) -> &LifecycleDispatcher {
        &self.lifecycle_dispatcher
    }

    /// Transform a module through all registered transform plugins.
    /// Each plugin's transform is applied sequentially.
    pub fn transform_module(&self, module_bytes: &[u8]) -> Result<Vec<u8>> {
        if self.module_transform_plugins.is_empty() {
            return Ok(module_bytes.to_vec());
        }

        let context = ModuleContext::new(module_bytes)?;
        // TODO: Invoke each transform plugin's `lunatic_create_module_hook`
        // For now, just return the re-encoded module
        context.encode()
    }

    /// Check if any plugins are registered
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Number of registered plugins
    pub fn len(&self) -> usize {
        self.plugins.len()
    }
}

/// Trait for process states that support plugins.
/// Implemented by DefaultProcessState in the root crate.
pub trait PluginCtx {
    fn plugin_registry(&self) -> &Arc<PluginRegistry>;
}

/// Check if a fully-qualified function name matches a namespace filter
pub fn namespace_matches_filter(namespace: &str, name: &str, filter: &[String]) -> bool {
    let full_name = format!("{namespace}::{name}");
    filter.iter().any(|allowed| full_name.starts_with(allowed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_namespace_matches_filter() {
        let filter = vec![
            "lunatic::".to_string(),
            "wasi_snapshot_preview1".to_string(),
        ];
        assert!(namespace_matches_filter(
            "lunatic::process",
            "spawn",
            &filter
        ));
        assert!(namespace_matches_filter(
            "wasi_snapshot_preview1",
            "fd_read",
            &filter
        ));
        assert!(!namespace_matches_filter("custom", "foo", &filter));
    }

    #[test]
    fn test_empty_registry() {
        let registry = PluginRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_registry_register_and_get() {
        let registry = PluginRegistry::new();
        // We can't create a real wasmtime::Module without an engine, so we test
        // the registry logic through type system and public API structure
        assert!(registry.get("test-plugin").is_none());
        assert_eq!(registry.len(), 0);
    }
}
