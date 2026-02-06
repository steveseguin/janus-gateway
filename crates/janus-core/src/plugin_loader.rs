//! Dynamic plugin loading.
//!
//! Scans a directory for shared libraries and loads any that export the
//! `janus_plugin_create` symbol.

use crate::config::JanusConfig;
use janus_plugin_api::{JanusPlugin, PluginCallbacks, PLUGIN_API_VERSION};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// A loaded plugin with its library handle.
pub struct LoadedPlugin {
    pub plugin: Box<dyn JanusPlugin>,
    /// Keep the library handle alive so symbols remain valid.
    _library: libloading::Library,
    pub path: PathBuf,
}

/// Type signature for the plugin factory function.
type PluginCreateFn = unsafe extern "C" fn() -> *mut dyn JanusPlugin;

/// Registry of all loaded plugins, keyed by package name.
pub struct PluginRegistry {
    plugins: HashMap<String, LoadedPlugin>,
}

impl PluginRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    /// Load all plugins from the given directory.
    ///
    /// Scans for `.so` (Linux) / `.dylib` (macOS) files, attempts to load
    /// each one, and registers successful loads.
    pub fn load_from_directory(
        &mut self,
        dir: &Path,
        disable_list: &[String],
    ) -> crate::Result<()> {
        let extension = if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        };

        let entries = std::fs::read_dir(dir).map_err(|e| {
            crate::Error::Config(format!(
                "cannot read plugins directory {}: {}",
                dir.display(),
                e
            ))
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == extension) {
                match self.load_plugin(&path, disable_list) {
                    Ok(Some(pkg)) => info!(plugin = %pkg, path = %path.display(), "plugin loaded"),
                    Ok(None) => {} // skipped (disabled)
                    Err(e) => error!(path = %path.display(), error = %e, "failed to load plugin"),
                }
            }
        }

        if self.plugins.is_empty() {
            warn!("no plugins loaded");
        }

        Ok(())
    }

    /// Attempt to load a single plugin. Returns the package name on success,
    /// None if the plugin was on the disable list.
    fn load_plugin(
        &mut self,
        path: &Path,
        disable_list: &[String],
    ) -> crate::Result<Option<String>> {
        // Safety: we must trust that the .so exports a valid function.
        unsafe {
            let lib = libloading::Library::new(path).map_err(|e| {
                crate::Error::PluginLoad(format!("dlopen {}: {}", path.display(), e))
            })?;

            let create_fn: libloading::Symbol<PluginCreateFn> =
                lib.get(b"janus_plugin_create").map_err(|e| {
                    crate::Error::PluginLoad(format!(
                        "dlsym janus_plugin_create in {}: {}",
                        path.display(),
                        e
                    ))
                })?;

            let raw = create_fn();
            if raw.is_null() {
                return Err(crate::Error::PluginLoad(format!(
                    "janus_plugin_create returned null in {}",
                    path.display()
                )));
            }

            let plugin = Box::from_raw(raw);
            let package = plugin.package().to_string();

            // Check disable list
            if disable_list.iter().any(|d| d == &package) {
                debug!(plugin = %package, "plugin disabled, skipping");
                // Drop the plugin; we intentionally don't register it.
                // But we need to keep `lib` alive until after `plugin` is dropped.
                drop(plugin);
                return Ok(None);
            }

            let version = plugin.version();
            info!(
                plugin = %package,
                version = version,
                name = plugin.name(),
                "plugin registered"
            );

            self.plugins.insert(
                package.clone(),
                LoadedPlugin {
                    plugin,
                    _library: lib,
                    path: path.to_path_buf(),
                },
            );

            Ok(Some(package))
        }
    }

    /// Initialize all loaded plugins with the given callbacks.
    pub async fn init_all(
        &mut self,
        callbacks: Arc<dyn PluginCallbacks>,
        config: &JanusConfig,
    ) -> crate::Result<()> {
        let config_path = &config.general.configs_folder;
        for (package, loaded) in &mut self.plugins {
            debug!(plugin = %package, "initializing plugin");
            loaded
                .plugin
                .init(Arc::clone(&callbacks), config_path)
                .await
                .map_err(|e| {
                    crate::Error::PluginLoad(format!("plugin {} init failed: {}", package, e))
                })?;
        }
        Ok(())
    }

    /// Destroy all loaded plugins.
    pub async fn destroy_all(&mut self) {
        for (package, loaded) in &mut self.plugins {
            debug!(plugin = %package, "destroying plugin");
            if let Err(e) = loaded.plugin.destroy().await {
                error!(plugin = %package, error = %e, "plugin destroy failed");
            }
        }
    }

    /// Get a reference to a plugin by package name.
    pub fn get(&self, package: &str) -> Option<&dyn JanusPlugin> {
        self.plugins.get(package).map(|l| l.plugin.as_ref())
    }

    /// Get a mutable reference to a plugin by package name.
    pub fn get_mut(&mut self, package: &str) -> Option<&mut dyn JanusPlugin> {
        self.plugins.get_mut(package).map(|l| l.plugin.as_mut())
    }

    /// List all loaded plugin package names.
    pub fn packages(&self) -> Vec<&str> {
        self.plugins.keys().map(|s| s.as_str()).collect()
    }

    /// Number of loaded plugins.
    pub fn count(&self) -> usize {
        self.plugins.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry() {
        let reg = PluginRegistry::new();
        assert_eq!(reg.count(), 0);
        assert!(reg.packages().is_empty());
        assert!(reg.get("janus.plugin.echotest").is_none());
    }

    #[test]
    fn load_from_nonexistent_directory_errors() {
        let mut reg = PluginRegistry::new();
        let result = reg.load_from_directory(Path::new("/nonexistent"), &[]);
        assert!(result.is_err());
    }

    #[test]
    fn load_from_empty_directory() {
        let dir = std::env::temp_dir().join("janus_test_empty_plugins");
        let _ = std::fs::create_dir_all(&dir);
        let mut reg = PluginRegistry::new();
        // Should succeed but warn about no plugins
        let result = reg.load_from_directory(&dir, &[]);
        assert!(result.is_ok());
        assert_eq!(reg.count(), 0);
        let _ = std::fs::remove_dir(&dir);
    }
}
