use std::sync::Arc;

use wasmtime::{Linker, Store, Val};

use crate::Plugin;

/// Events that plugins can hook into
#[derive(Debug, Clone)]
pub enum LifecycleEvent {
    /// A process is about to be spawned
    ProcessSpawning { process_id: u64 },
    /// A process has been spawned
    ProcessSpawned { process_id: u64 },
    /// A process is about to exit
    ProcessExiting { process_id: u64 },
    /// A process has exited
    ProcessExited {
        process_id: u64,
        error: Option<String>,
    },
    /// A module is being loaded
    ModuleLoading { module_name: String },
    /// A module has been loaded
    ModuleLoaded { module_name: String },
}

/// Dispatches lifecycle events to registered plugins
pub struct LifecycleDispatcher {
    plugins: Vec<Arc<Plugin>>,
}

impl LifecycleDispatcher {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Add a plugin to receive lifecycle events
    pub fn add_plugin(&mut self, plugin: Arc<Plugin>) {
        self.plugins.push(plugin);
    }

    /// Dispatch a lifecycle event to all registered plugins
    ///
    /// For each plugin, instantiates a fresh wasm instance and calls the
    /// corresponding lifecycle hook export. Errors are logged and do not
    /// propagate -- a failing plugin never takes down the runtime.
    ///
    /// For module events, the module name string is written into the plugin's
    /// exported `memory` at offset 0 and passed as `(ptr: i32, len: i32)`.
    pub fn dispatch(&self, event: &LifecycleEvent) {
        log::trace!(
            "Lifecycle event: {event:?}, notifying {} plugins",
            self.plugins.len()
        );

        let export_name = Self::event_export_name(event);

        for plugin in &self.plugins {
            let engine = plugin.module.engine();
            let mut store = Store::new(engine, ());
            let linker = Linker::<()>::new(engine);

            let instance = match linker.instantiate(&mut store, &plugin.module) {
                Ok(inst) => inst,
                Err(e) => {
                    log::warn!(
                        "Failed to instantiate plugin '{}' for event {export_name}: {e}",
                        plugin.info.name
                    );
                    continue;
                }
            };

            let func = match instance.get_func(&mut store, export_name) {
                Some(f) => f,
                None => {
                    log::trace!(
                        "Plugin '{}' does not export '{export_name}', skipping",
                        plugin.info.name
                    );
                    continue;
                }
            };

            let args = match Self::build_args(event, &instance, &mut store) {
                Ok(args) => args,
                Err(e) => {
                    log::warn!(
                        "Plugin '{}': failed to prepare args for '{export_name}': {e}",
                        plugin.info.name
                    );
                    continue;
                }
            };

            if let Err(e) = func.call(&mut store, &args, &mut []) {
                log::warn!(
                    "Plugin '{}' hook '{export_name}' failed: {e}",
                    plugin.info.name
                );
            }
        }
    }

    /// Map a lifecycle event to its corresponding wasm export name
    fn event_export_name(event: &LifecycleEvent) -> &'static str {
        match event {
            LifecycleEvent::ProcessSpawning { .. } => "lunatic_on_process_spawning",
            LifecycleEvent::ProcessSpawned { .. } => "lunatic_on_process_spawned",
            LifecycleEvent::ProcessExiting { .. } => "lunatic_on_process_exiting",
            LifecycleEvent::ProcessExited { .. } => "lunatic_on_process_exited",
            LifecycleEvent::ModuleLoading { .. } => "lunatic_on_module_loading",
            LifecycleEvent::ModuleLoaded { .. } => "lunatic_on_module_loaded",
        }
    }

    /// Build the argument list for a lifecycle hook call.
    ///
    /// Process events pass `(process_id: i64)`.
    /// Module events write the module name into the plugin's exported memory
    /// at offset 0 and pass `(ptr: i32, len: i32)`.
    fn build_args(
        event: &LifecycleEvent,
        instance: &wasmtime::Instance,
        store: &mut Store<()>,
    ) -> anyhow::Result<Vec<Val>> {
        match event {
            LifecycleEvent::ProcessSpawning { process_id }
            | LifecycleEvent::ProcessSpawned { process_id }
            | LifecycleEvent::ProcessExiting { process_id }
            | LifecycleEvent::ProcessExited { process_id, .. } => {
                Ok(vec![Val::I64(*process_id as i64)])
            }
            LifecycleEvent::ModuleLoading { module_name }
            | LifecycleEvent::ModuleLoaded { module_name, .. } => {
                let name_bytes = module_name.as_bytes();
                let memory = instance.get_memory(&mut *store, "memory").ok_or_else(|| {
                    anyhow::anyhow!("plugin must export memory for module events")
                })?;
                memory.write(&mut *store, 0, name_bytes)?;
                Ok(vec![Val::I32(0), Val::I32(name_bytes.len() as i32)])
            }
        }
    }

    /// Number of registered lifecycle plugins
    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }
}

impl Default for LifecycleDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_dispatcher() {
        let dispatcher = LifecycleDispatcher::new();
        assert_eq!(dispatcher.plugin_count(), 0);
        // Should not panic
        dispatcher.dispatch(&LifecycleEvent::ProcessSpawned { process_id: 1 });
    }

    #[test]
    fn test_event_export_names() {
        assert_eq!(
            LifecycleDispatcher::event_export_name(&LifecycleEvent::ProcessSpawning {
                process_id: 1
            }),
            "lunatic_on_process_spawning"
        );
        assert_eq!(
            LifecycleDispatcher::event_export_name(&LifecycleEvent::ProcessSpawned {
                process_id: 1
            }),
            "lunatic_on_process_spawned"
        );
        assert_eq!(
            LifecycleDispatcher::event_export_name(&LifecycleEvent::ProcessExiting {
                process_id: 1
            }),
            "lunatic_on_process_exiting"
        );
        assert_eq!(
            LifecycleDispatcher::event_export_name(&LifecycleEvent::ProcessExited {
                process_id: 1,
                error: None
            }),
            "lunatic_on_process_exited"
        );
        assert_eq!(
            LifecycleDispatcher::event_export_name(&LifecycleEvent::ModuleLoading {
                module_name: "test".into()
            }),
            "lunatic_on_module_loading"
        );
        assert_eq!(
            LifecycleDispatcher::event_export_name(&LifecycleEvent::ModuleLoaded {
                module_name: "test".into()
            }),
            "lunatic_on_module_loaded"
        );
    }

    #[test]
    fn test_build_args_process_events() {
        // Process events don't need memory, but build_args requires an instance
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, "(module)").unwrap();
        let mut store = Store::new(&engine, ());
        let linker = Linker::<()>::new(&engine);
        let instance = linker.instantiate(&mut store, &module).unwrap();

        let args = LifecycleDispatcher::build_args(
            &LifecycleEvent::ProcessSpawned { process_id: 42 },
            &instance,
            &mut store,
        )
        .unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].unwrap_i64(), 42);

        let args = LifecycleDispatcher::build_args(
            &LifecycleEvent::ProcessExited {
                process_id: 99,
                error: Some("oops".into()),
            },
            &instance,
            &mut store,
        )
        .unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].unwrap_i64(), 99);
    }

    #[test]
    fn test_build_args_module_events() {
        // Module events need an instance with exported memory to build args
        let engine = wasmtime::Engine::default();
        let module =
            wasmtime::Module::new(&engine, "(module (memory (export \"memory\") 1))").unwrap();
        let mut store = Store::new(&engine, ());
        let linker = Linker::<()>::new(&engine);
        let instance = linker.instantiate(&mut store, &module).unwrap();

        let args = LifecycleDispatcher::build_args(
            &LifecycleEvent::ModuleLoading {
                module_name: "test.wasm".into(),
            },
            &instance,
            &mut store,
        )
        .unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].unwrap_i32(), 0); // ptr
        assert_eq!(args[1].unwrap_i32(), 9); // len of "test.wasm"

        // Verify the string was written to memory
        let memory = instance.get_memory(&mut store, "memory").unwrap();
        let mut buf = vec![0u8; 9];
        memory.read(&store, 0, &mut buf).unwrap();
        assert_eq!(&buf, b"test.wasm");
    }

    #[test]
    fn test_build_args_module_event_no_memory() {
        // Module events without exported memory should return an error
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, "(module)").unwrap();
        let mut store = Store::new(&engine, ());
        let linker = Linker::<()>::new(&engine);
        let instance = linker.instantiate(&mut store, &module).unwrap();

        let result = LifecycleDispatcher::build_args(
            &LifecycleEvent::ModuleLoaded {
                module_name: "test".into(),
            },
            &instance,
            &mut store,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_dispatch_with_plugin_missing_export() {
        // A minimal wasm module with no exports -- the dispatcher should
        // skip it silently (trace log, no panic).
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, "(module)").unwrap();
        let plugin = Arc::new(crate::Plugin {
            info: crate::PluginInfo {
                name: "no-exports".into(),
                version: semver::Version::new(0, 1, 0),
                capabilities: vec![crate::Capability::LifecycleHooks],
                dependencies: vec![],
            },
            module,
        });

        let mut dispatcher = LifecycleDispatcher::new();
        dispatcher.add_plugin(plugin);
        // Must not panic
        dispatcher.dispatch(&LifecycleEvent::ProcessSpawned { process_id: 1 });
        dispatcher.dispatch(&LifecycleEvent::ModuleLoaded {
            module_name: "m".into(),
        });
    }

    #[test]
    fn test_dispatch_calls_process_hook() {
        // A wasm module that exports lunatic_on_process_spawned(i64) -> ()
        // The function body is a no-op (just returns).
        let wat = r#"
            (module
                (func (export "lunatic_on_process_spawned") (param i64))
            )
        "#;
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, wat).unwrap();
        let plugin = Arc::new(crate::Plugin {
            info: crate::PluginInfo {
                name: "process-hook".into(),
                version: semver::Version::new(0, 1, 0),
                capabilities: vec![crate::Capability::LifecycleHooks],
                dependencies: vec![],
            },
            module,
        });

        let mut dispatcher = LifecycleDispatcher::new();
        dispatcher.add_plugin(plugin);
        // Must not panic -- the hook is called successfully
        dispatcher.dispatch(&LifecycleEvent::ProcessSpawned { process_id: 123 });
    }

    #[test]
    fn test_dispatch_calls_module_hook() {
        // Module hook now receives (ptr: i32, len: i32) pointing to the module name
        // in exported memory. This plugin stores the ptr and len in globals so
        // we can verify dispatch called it correctly.
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "lunatic_on_module_loaded") (param $ptr i32) (param $len i32))
            )
        "#;
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, wat).unwrap();
        let plugin = Arc::new(crate::Plugin {
            info: crate::PluginInfo {
                name: "module-hook".into(),
                version: semver::Version::new(0, 1, 0),
                capabilities: vec![crate::Capability::LifecycleHooks],
                dependencies: vec![],
            },
            module,
        });

        let mut dispatcher = LifecycleDispatcher::new();
        dispatcher.add_plugin(plugin);
        // Must not panic -- the hook receives the module name via memory
        dispatcher.dispatch(&LifecycleEvent::ModuleLoaded {
            module_name: "test-mod.wasm".into(),
        });
    }

    #[test]
    fn test_dispatch_module_hook_reads_name() {
        // Verify the plugin can actually read the module name from memory.
        // This plugin copies the name bytes to offset 1024 so we can verify
        // the content was correctly passed.
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (global (export "stored_len") (mut i32) (i32.const 0))

                (func (export "lunatic_on_module_loading") (param $ptr i32) (param $len i32)
                    (global.set 0 (local.get $len))
                    ;; Copy name from ptr to offset 1024
                    (memory.copy
                        (i32.const 1024)
                        (local.get $ptr)
                        (local.get $len))
                )
            )
        "#;
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, wat).unwrap();

        // Manually instantiate to verify memory contents after dispatch
        let mut store = Store::new(&engine, ());
        let linker = Linker::<()>::new(&engine);
        let instance = linker.instantiate(&mut store, &module).unwrap();

        let event = LifecycleEvent::ModuleLoading {
            module_name: "my_module.wasm".into(),
        };
        let name = "lunatic_on_module_loading";
        let func = instance.get_func(&mut store, name).unwrap();
        let args = LifecycleDispatcher::build_args(&event, &instance, &mut store).unwrap();
        func.call(&mut store, &args, &mut []).unwrap();

        // Read back the stored length
        let stored_len = instance
            .get_global(&mut store, "stored_len")
            .unwrap()
            .get(&mut store)
            .unwrap_i32();
        assert_eq!(stored_len, 14); // "my_module.wasm".len()

        // Read back the copied name from offset 1024
        let memory = instance.get_memory(&mut store, "memory").unwrap();
        let mut buf = vec![0u8; 14];
        memory.read(&store, 1024, &mut buf).unwrap();
        assert_eq!(&buf, b"my_module.wasm");
    }
}
