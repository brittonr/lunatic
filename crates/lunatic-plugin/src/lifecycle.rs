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
    ProcessExited { process_id: u64, error: Option<String> },
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
    pub fn dispatch(&self, event: &LifecycleEvent) {
        log::trace!("Lifecycle event: {event:?}, notifying {} plugins", self.plugins.len());

        let export_name = Self::event_export_name(event);
        let args = Self::event_args(event);

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

            let mut results = [];
            if let Err(e) = func.call(&mut store, &args, &mut results) {
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

    /// Build the argument list for a lifecycle hook call
    fn event_args(event: &LifecycleEvent) -> Vec<Val> {
        match event {
            LifecycleEvent::ProcessSpawning { process_id }
            | LifecycleEvent::ProcessSpawned { process_id }
            | LifecycleEvent::ProcessExiting { process_id }
            | LifecycleEvent::ProcessExited { process_id, .. } => {
                vec![Val::I64(*process_id as i64)]
            }
            LifecycleEvent::ModuleLoading { .. } | LifecycleEvent::ModuleLoaded { .. } => {
                vec![]
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
    fn test_event_args_process_events() {
        let args =
            LifecycleDispatcher::event_args(&LifecycleEvent::ProcessSpawned { process_id: 42 });
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].unwrap_i64(), 42);

        let args = LifecycleDispatcher::event_args(&LifecycleEvent::ProcessExited {
            process_id: 99,
            error: Some("oops".into()),
        });
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].unwrap_i64(), 99);
    }

    #[test]
    fn test_event_args_module_events() {
        let args = LifecycleDispatcher::event_args(&LifecycleEvent::ModuleLoading {
            module_name: "test".into(),
        });
        assert!(args.is_empty());

        let args = LifecycleDispatcher::event_args(&LifecycleEvent::ModuleLoaded {
            module_name: "test".into(),
        });
        assert!(args.is_empty());
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
        // A wasm module that exports lunatic_on_module_loaded() -> ()
        let wat = r#"
            (module
                (func (export "lunatic_on_module_loaded"))
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
        // Must not panic
        dispatcher.dispatch(&LifecycleEvent::ModuleLoaded {
            module_name: "test-mod".into(),
        });
    }
}
