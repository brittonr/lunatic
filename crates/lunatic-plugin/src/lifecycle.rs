use std::sync::Arc;

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
    /// TODO: Invoke the plugin's lifecycle hook function via wasmtime
    /// For now this logs the event and is a no-op
    pub fn dispatch(&self, event: &LifecycleEvent) {
        log::trace!("Lifecycle event: {event:?}, notifying {} plugins", self.plugins.len());
        // TODO: For each plugin, instantiate and call its lifecycle hook export
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
}
