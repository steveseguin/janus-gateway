//! Janus Record&Play plugin — record and replay WebRTC sessions (stub).

use async_trait::async_trait;
use janus_plugin_api::{Jsep, PluginCallbacks, PluginResult, PluginSession};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

/// The Record&Play plugin (stub).
pub struct RecordPlayPlugin {
    callbacks: Option<Arc<dyn PluginCallbacks>>,
}

impl Default for RecordPlayPlugin {
    fn default() -> Self {
        Self { callbacks: None }
    }
}

#[async_trait]
impl janus_plugin_api::JanusPlugin for RecordPlayPlugin {
    fn name(&self) -> &'static str {
        "Janus Record&Play plugin"
    }

    fn package(&self) -> &'static str {
        "janus.plugin.recordplay"
    }

    fn version(&self) -> u32 {
        1
    }

    fn version_string(&self) -> &'static str {
        "0.1.0"
    }

    fn description(&self) -> &'static str {
        "Record and replay WebRTC sessions from file storage."
    }

    fn author(&self) -> &'static str {
        "Janus Rust Contributors"
    }

    async fn init(
        &mut self,
        callbacks: Arc<dyn PluginCallbacks>,
        _config_path: &Path,
    ) -> janus_plugin_api::Result<()> {
        info!("Record&Play plugin initialized");
        self.callbacks = Some(callbacks);
        Ok(())
    }

    async fn destroy(&mut self) -> janus_plugin_api::Result<()> {
        info!("Record&Play plugin destroyed");
        self.callbacks = None;
        Ok(())
    }

    async fn create_session(&self, _session: &PluginSession) -> janus_plugin_api::Result<()> {
        Ok(())
    }

    async fn destroy_session(&self, _session: &PluginSession) -> janus_plugin_api::Result<()> {
        Ok(())
    }

    fn query_session(&self, _session: &PluginSession) -> janus_plugin_api::Result<serde_json::Value> {
        Ok(json!({"status": "stub"}))
    }

    async fn handle_message(
        &self,
        _session: &PluginSession,
        _transaction: &str,
        _body: serde_json::Value,
        _jsep: Option<Jsep>,
    ) -> janus_plugin_api::Result<PluginResult> {
        Err(janus_plugin_api::Error::NotImplemented)
    }
}

/// FFI entry point for dynamic loading.
#[no_mangle]
pub extern "C" fn janus_plugin_recordplay_create() -> *mut dyn janus_plugin_api::JanusPlugin {
    let plugin = RecordPlayPlugin::default();
    Box::into_raw(Box::new(plugin))
}

#[cfg(test)]
mod tests {
    use super::*;
    use janus_plugin_api::JanusPlugin;

    #[test]
    fn plugin_metadata() {
        let plugin = RecordPlayPlugin::default();
        assert_eq!(plugin.name(), "Janus Record&Play plugin");
        assert_eq!(plugin.package(), "janus.plugin.recordplay");
        assert_eq!(plugin.version(), 1);
        assert_eq!(plugin.version_string(), "0.1.0");
        assert_eq!(plugin.author(), "Janus Rust Contributors");
        assert!(!plugin.description().is_empty());
    }

    #[test]
    fn ffi_create_returns_valid_plugin() {
        let raw = janus_plugin_recordplay_create();
        assert!(!raw.is_null());
        unsafe {
            let _ = Box::from_raw(raw);
        }
    }
}
