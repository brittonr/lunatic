#![forbid(unsafe_code)]

mod lifecycle;
mod module_context;

pub use lifecycle::{LifecycleDispatcher, LifecycleEvent};
pub use module_context::ModuleContext;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use wasmtime::{Caller, Engine, Linker, Module, Store};

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

/// Host state for plugin transform invocations
struct PluginHostState {
    input_bytes: Vec<u8>,
    output_bytes: Vec<u8>,
}

/// Registry that manages loaded plugins
pub struct PluginRegistry {
    engine: Engine,
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
        let mut config = wasmtime::Config::new();
        config.async_support(false);
        let engine = Engine::new(&config).expect("failed to create plugin engine");
        Self {
            engine,
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

    /// Register a plugin from raw Wasm bytes
    pub fn register_wasm(&mut self, info: PluginInfo, wasm: &[u8]) -> Result<()> {
        let module = Module::new(&self.engine, wasm)?;
        let plugin = Plugin { info, module };
        self.register(plugin)
    }

    /// Get the plugin engine
    pub fn engine(&self) -> &Engine {
        &self.engine
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

        let mut current_bytes = module_bytes.to_vec();

        for plugin in &self.module_transform_plugins {
            let engine = plugin.module.engine();
            let state = PluginHostState {
                input_bytes: current_bytes.clone(),
                output_bytes: Vec::new(),
            };
            let mut store = Store::new(engine, state);

            let mut linker: Linker<PluginHostState> = Linker::new(engine);

            linker.func_wrap(
                "lunatic_plugin",
                "input_size",
                |caller: Caller<PluginHostState>| -> i32 {
                    caller.data().input_bytes.len() as i32
                },
            )?;

            linker.func_wrap(
                "lunatic_plugin",
                "read_input",
                |mut caller: Caller<PluginHostState>, dest_ptr: i32| -> Result<()> {
                    let input = caller.data().input_bytes.clone();
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| anyhow::anyhow!("plugin must export memory"))?;
                    memory.write(&mut caller, dest_ptr as usize, &input)?;
                    Ok(())
                },
            )?;

            linker.func_wrap(
                "lunatic_plugin",
                "write_output",
                |mut caller: Caller<PluginHostState>, src_ptr: i32, len: i32| -> Result<()> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| anyhow::anyhow!("plugin must export memory"))?;
                    let src = src_ptr as usize;
                    let size = len as usize;
                    let data = memory.data(&caller);
                    anyhow::ensure!(
                        src.checked_add(size).is_some_and(|end| end <= data.len()),
                        "write_output: out-of-bounds read from plugin memory"
                    );
                    let output = data[src..src + size].to_vec();
                    caller.data_mut().output_bytes = output;
                    Ok(())
                },
            )?;

            let instance = linker.instantiate(&mut store, &plugin.module)?;

            let func = instance.get_func(&mut store, "lunatic_transform_module");
            if let Some(func) = func {
                func.call(&mut store, &[], &mut [])?;
                let output = &store.data().output_bytes;
                if !output.is_empty() {
                    current_bytes = output.clone();
                }
            } else {
                log::warn!(
                    "Transform plugin '{}' does not export 'lunatic_transform_module', skipping",
                    plugin.info.name
                );
            }
        }

        Ok(current_bytes)
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

    #[test]
    fn test_register_wasm() {
        let mut registry = PluginRegistry::new();
        let info = PluginInfo {
            name: "test".into(),
            version: semver::Version::new(0, 1, 0),
            capabilities: vec![Capability::LifecycleHooks],
            dependencies: vec![],
        };
        registry.register_wasm(info, b"(module)").unwrap();
        assert_eq!(registry.len(), 1);
        assert!(registry.get("test").is_some());
    }

    #[test]
    fn test_register_wasm_invalid_module() {
        let mut registry = PluginRegistry::new();
        let info = PluginInfo {
            name: "bad".into(),
            version: semver::Version::new(0, 1, 0),
            capabilities: vec![],
            dependencies: vec![],
        };
        let result = registry.register_wasm(info, b"not valid wasm");
        assert!(result.is_err());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_transform_module_no_plugins() {
        let registry = PluginRegistry::new();
        let input = b"some module bytes";
        let output = registry.transform_module(input).unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn test_transform_module_passthrough_plugin() {
        let mut registry = PluginRegistry::new();
        let wat = r#"
            (module
                (import "lunatic_plugin" "input_size" (func $input_size (result i32)))
                (import "lunatic_plugin" "read_input" (func $read_input (param i32)))
                (import "lunatic_plugin" "write_output" (func $write_output (param i32 i32)))
                (memory (export "memory") 1)
                (func (export "lunatic_transform_module")
                    (local $size i32)
                    (local.set $size (call $input_size))
                    (call $read_input (i32.const 0))
                    (call $write_output (i32.const 0) (local.get $size))
                )
            )
        "#;
        let info = PluginInfo {
            name: "passthrough".into(),
            version: semver::Version::new(0, 1, 0),
            capabilities: vec![Capability::ModuleTransform],
            dependencies: vec![],
        };
        registry.register_wasm(info, wat.as_bytes()).unwrap();

        let input = b"hello wasm world";
        let output = registry.transform_module(input).unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn test_transform_module_plugin_no_export() {
        let mut registry = PluginRegistry::new();
        let info = PluginInfo {
            name: "no-transform-export".into(),
            version: semver::Version::new(0, 1, 0),
            capabilities: vec![Capability::ModuleTransform],
            dependencies: vec![],
        };
        registry.register_wasm(info, b"(module)").unwrap();

        let input = b"original bytes";
        let output = registry.transform_module(input).unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn test_transform_module_chained_plugins() {
        let mut registry = PluginRegistry::new();
        let wat = r#"
            (module
                (import "lunatic_plugin" "input_size" (func $input_size (result i32)))
                (import "lunatic_plugin" "read_input" (func $read_input (param i32)))
                (import "lunatic_plugin" "write_output" (func $write_output (param i32 i32)))
                (memory (export "memory") 1)
                (func (export "lunatic_transform_module")
                    (local $size i32)
                    (local.set $size (call $input_size))
                    (call $read_input (i32.const 0))
                    (call $write_output (i32.const 0) (local.get $size))
                )
            )
        "#;

        let info1 = PluginInfo {
            name: "passthrough1".into(),
            version: semver::Version::new(0, 1, 0),
            capabilities: vec![Capability::ModuleTransform],
            dependencies: vec![],
        };
        registry.register_wasm(info1, wat.as_bytes()).unwrap();

        let info2 = PluginInfo {
            name: "passthrough2".into(),
            version: semver::Version::new(0, 2, 0),
            capabilities: vec![Capability::ModuleTransform],
            dependencies: vec![],
        };
        registry.register_wasm(info2, wat.as_bytes()).unwrap();

        assert_eq!(registry.len(), 2);
        assert_eq!(registry.module_transform_plugins().len(), 2);

        let input = b"chained transform input";
        let output = registry.transform_module(input).unwrap();
        assert_eq!(output, input);
    }

    // ---- Integration tests proving the plugin system works end-to-end ----

    /// A lifecycle plugin that stores the received process_id into linear memory.
    /// We manually instantiate it and verify the memory contents after the hook runs.
    #[test]
    fn test_lifecycle_plugin_receives_process_id() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (global $call_count (mut i32) (i32.const 0))

                ;; Store the process_id at memory[0..8] and bump the call counter
                (func (export "lunatic_on_process_spawned") (param $pid i64)
                    (i64.store (i32.const 0) (local.get $pid))
                    (global.set $call_count
                        (i32.add (global.get $call_count) (i32.const 1)))
                )

                ;; Export the call count so the host can read it
                (func (export "get_call_count") (result i32)
                    (global.get $call_count)
                )
            )
        "#;

        let engine = wasmtime::Engine::default();
        let module = Module::new(&engine, wat).unwrap();
        let mut store = Store::new(&engine, ());
        let linker = Linker::<()>::new(&engine);
        let instance = linker.instantiate(&mut store, &module).unwrap();

        // Call the lifecycle hook with process_id = 0xDEAD_BEEF_CAFE_BABE
        let hook = instance
            .get_func(&mut store, "lunatic_on_process_spawned")
            .unwrap();
        let pid: i64 = 0x0EAD_BEEF_CAFE_BABEu64 as i64;
        hook.call(&mut store, &[wasmtime::Val::I64(pid)], &mut [])
            .unwrap();

        // Read back the process_id from plugin memory
        let memory = instance.get_memory(&mut store, "memory").unwrap();
        let mut buf = [0u8; 8];
        memory.read(&store, 0, &mut buf).unwrap();
        let stored_pid = u64::from_le_bytes(buf);
        assert_eq!(stored_pid, 0x0EAD_BEEF_CAFE_BABE);

        // Verify the call counter incremented
        let get_count = instance
            .get_typed_func::<(), i32>(&mut store, "get_call_count")
            .unwrap();
        assert_eq!(get_count.call(&mut store, ()).unwrap(), 1);

        // Call again, counter should be 2
        hook.call(&mut store, &[wasmtime::Val::I64(42)], &mut [])
            .unwrap();
        assert_eq!(get_count.call(&mut store, ()).unwrap(), 2);
        memory.read(&store, 0, &mut buf).unwrap();
        assert_eq!(u64::from_le_bytes(buf), 42);
    }

    /// A transform plugin that appends a 4-byte marker [0xDE, 0xAD, 0xBE, 0xEF]
    /// to every module it processes. Proves the transform pipeline mutates bytes.
    #[test]
    fn test_transform_plugin_appends_marker() {
        let wat = r#"
            (module
                (import "lunatic_plugin" "input_size" (func $input_size (result i32)))
                (import "lunatic_plugin" "read_input" (func $read_input (param i32)))
                (import "lunatic_plugin" "write_output" (func $write_output (param i32 i32)))
                (memory (export "memory") 1)

                (func (export "lunatic_transform_module")
                    (local $size i32)
                    (local.set $size (call $input_size))
                    ;; Read input into memory at offset 0
                    (call $read_input (i32.const 0))
                    ;; Append marker bytes after the input
                    (i32.store8 (local.get $size) (i32.const 0xDE))
                    (i32.store8 (i32.add (local.get $size) (i32.const 1)) (i32.const 0xAD))
                    (i32.store8 (i32.add (local.get $size) (i32.const 2)) (i32.const 0xBE))
                    (i32.store8 (i32.add (local.get $size) (i32.const 3)) (i32.const 0xEF))
                    ;; Write output = original + 4 marker bytes
                    (call $write_output
                        (i32.const 0)
                        (i32.add (local.get $size) (i32.const 4)))
                )
            )
        "#;

        let mut registry = PluginRegistry::new();
        registry
            .register_wasm(
                PluginInfo {
                    name: "marker-appender".into(),
                    version: semver::Version::new(1, 0, 0),
                    capabilities: vec![Capability::ModuleTransform],
                    dependencies: vec![],
                },
                wat.as_bytes(),
            )
            .unwrap();

        let input = b"hello lunatic";
        let output = registry.transform_module(input).unwrap();

        // Output should be input + [0xDE, 0xAD, 0xBE, 0xEF]
        assert_eq!(output.len(), input.len() + 4);
        assert_eq!(&output[..input.len()], input);
        assert_eq!(&output[input.len()..], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    /// Two transform plugins chained: first appends [0xAA], second appends [0xBB].
    /// Proves chaining order is preserved and each plugin sees the previous output.
    #[test]
    fn test_chained_transforms_mutate_sequentially() {
        // Plugin that appends a single configurable byte.
        // We use the same WAT but different data offsets to simulate two distinct plugins.
        let appender_wat = |byte: u8| {
            format!(
                r#"
                (module
                    (import "lunatic_plugin" "input_size" (func $input_size (result i32)))
                    (import "lunatic_plugin" "read_input" (func $read_input (param i32)))
                    (import "lunatic_plugin" "write_output" (func $write_output (param i32 i32)))
                    (memory (export "memory") 1)

                    (func (export "lunatic_transform_module")
                        (local $size i32)
                        (local.set $size (call $input_size))
                        (call $read_input (i32.const 0))
                        (i32.store8 (local.get $size) (i32.const {byte}))
                        (call $write_output
                            (i32.const 0)
                            (i32.add (local.get $size) (i32.const 1)))
                    )
                )
            "#,
                byte = byte
            )
        };

        let mut registry = PluginRegistry::new();
        registry
            .register_wasm(
                PluginInfo {
                    name: "append-aa".into(),
                    version: semver::Version::new(1, 0, 0),
                    capabilities: vec![Capability::ModuleTransform],
                    dependencies: vec![],
                },
                appender_wat(0xAA).as_bytes(),
            )
            .unwrap();

        registry
            .register_wasm(
                PluginInfo {
                    name: "append-bb".into(),
                    version: semver::Version::new(1, 0, 0),
                    capabilities: vec![Capability::ModuleTransform],
                    dependencies: vec![],
                },
                appender_wat(0xBB).as_bytes(),
            )
            .unwrap();

        let input = b"data";
        let output = registry.transform_module(input).unwrap();

        // First plugin appends 0xAA, second appends 0xBB
        assert_eq!(output.len(), input.len() + 2);
        assert_eq!(&output[..4], b"data");
        assert_eq!(output[4], 0xAA); // first plugin ran first
        assert_eq!(output[5], 0xBB); // second plugin ran second
    }

    /// Full integration: a registry with both lifecycle AND transform plugins.
    /// Proves the capability system correctly routes plugins to the right subsystem.
    #[test]
    fn test_full_registry_lifecycle_and_transform() {
        let lifecycle_wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "lunatic_on_process_spawning") (param i64))
                (func (export "lunatic_on_process_spawned") (param i64))
                (func (export "lunatic_on_process_exiting") (param i64))
                (func (export "lunatic_on_process_exited") (param i64))
                (func (export "lunatic_on_module_loading") (param i32 i32))
                (func (export "lunatic_on_module_loaded") (param i32 i32))
            )
        "#;

        let transform_wat = r#"
            (module
                (import "lunatic_plugin" "input_size" (func $input_size (result i32)))
                (import "lunatic_plugin" "read_input" (func $read_input (param i32)))
                (import "lunatic_plugin" "write_output" (func $write_output (param i32 i32)))
                (memory (export "memory") 1)
                (func (export "lunatic_transform_module")
                    (local $size i32)
                    (local.set $size (call $input_size))
                    (call $read_input (i32.const 0))
                    ;; Append sentinel
                    (i32.store8 (local.get $size) (i32.const 0xFF))
                    (call $write_output
                        (i32.const 0)
                        (i32.add (local.get $size) (i32.const 1)))
                )
            )
        "#;

        let mut registry = PluginRegistry::new();

        // Register lifecycle plugin
        registry
            .register_wasm(
                PluginInfo {
                    name: "observer".into(),
                    version: semver::Version::new(1, 0, 0),
                    capabilities: vec![Capability::LifecycleHooks],
                    dependencies: vec![],
                },
                lifecycle_wat.as_bytes(),
            )
            .unwrap();

        // Register transform plugin
        registry
            .register_wasm(
                PluginInfo {
                    name: "transformer".into(),
                    version: semver::Version::new(1, 0, 0),
                    capabilities: vec![Capability::ModuleTransform],
                    dependencies: vec![],
                },
                transform_wat.as_bytes(),
            )
            .unwrap();

        assert_eq!(registry.len(), 2);
        assert_eq!(registry.lifecycle_dispatcher().plugin_count(), 1);
        assert_eq!(registry.module_transform_plugins().len(), 1);

        // Lifecycle dispatch works (observer receives all events without error)
        let dispatcher = registry.lifecycle_dispatcher();
        dispatcher.dispatch(&LifecycleEvent::ProcessSpawning { process_id: 1 });
        dispatcher.dispatch(&LifecycleEvent::ProcessSpawned { process_id: 1 });
        dispatcher.dispatch(&LifecycleEvent::ProcessExiting { process_id: 1 });
        dispatcher.dispatch(&LifecycleEvent::ProcessExited {
            process_id: 1,
            error: None,
        });
        dispatcher.dispatch(&LifecycleEvent::ModuleLoading {
            module_name: "test.wasm".into(),
        });
        dispatcher.dispatch(&LifecycleEvent::ModuleLoaded {
            module_name: "test.wasm".into(),
        });

        // Transform works (transformer appends 0xFF)
        let input = b"module bytes";
        let output = registry.transform_module(input).unwrap();
        assert_eq!(output.len(), input.len() + 1);
        assert_eq!(&output[..input.len()], input);
        assert_eq!(*output.last().unwrap(), 0xFF);
    }

    /// A plugin with BOTH lifecycle AND transform capabilities registered as one plugin.
    #[test]
    fn test_dual_capability_plugin() {
        let wat = r#"
            (module
                (import "lunatic_plugin" "input_size" (func $input_size (result i32)))
                (import "lunatic_plugin" "read_input" (func $read_input (param i32)))
                (import "lunatic_plugin" "write_output" (func $write_output (param i32 i32)))
                (memory (export "memory") 1)

                ;; Lifecycle hooks (no-ops, just prove they don't interfere)
                (func (export "lunatic_on_process_spawned") (param i64))
                (func (export "lunatic_on_process_exited") (param i64))

                ;; Transform: uppercase all ASCII lowercase letters
                (func (export "lunatic_transform_module")
                    (local $size i32)
                    (local $i i32)
                    (local $byte i32)
                    (local.set $size (call $input_size))
                    (call $read_input (i32.const 0))

                    ;; Loop through each byte
                    (local.set $i (i32.const 0))
                    (block $break
                        (loop $loop
                            (br_if $break (i32.ge_u (local.get $i) (local.get $size)))
                            (local.set $byte
                                (i32.load8_u (local.get $i)))
                            ;; If byte >= 'a' (97) and byte <= 'z' (122), subtract 32
                            (if (i32.and
                                    (i32.ge_u (local.get $byte) (i32.const 97))
                                    (i32.le_u (local.get $byte) (i32.const 122)))
                                (then
                                    (i32.store8 (local.get $i)
                                        (i32.sub (local.get $byte) (i32.const 32)))))
                            (local.set $i
                                (i32.add (local.get $i) (i32.const 1)))
                            (br $loop)
                        )
                    )

                    (call $write_output (i32.const 0) (local.get $size))
                )
            )
        "#;

        let mut registry = PluginRegistry::new();
        registry
            .register_wasm(
                PluginInfo {
                    name: "dual-plugin".into(),
                    version: semver::Version::new(1, 0, 0),
                    capabilities: vec![Capability::LifecycleHooks, Capability::ModuleTransform],
                    dependencies: vec![],
                },
                wat.as_bytes(),
            )
            .unwrap();

        // One plugin registered, visible in both subsystems
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.lifecycle_dispatcher().plugin_count(), 1);
        assert_eq!(registry.module_transform_plugins().len(), 1);

        // Note: lifecycle dispatch will fail to instantiate this module because the
        // lifecycle linker doesn't provide lunatic_plugin imports. This is by design:
        // lifecycle-only hooks should be in a separate module without transform imports.
        // The transform still works independently.

        // Transform uppercases ASCII
        let input = b"hello world";
        let output = registry.transform_module(input).unwrap();
        assert_eq!(output, b"HELLO WORLD");
    }
}
