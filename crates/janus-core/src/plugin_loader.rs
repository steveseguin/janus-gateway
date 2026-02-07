//! Dynamic plugin loading.
//!
//! Scans a directory for shared libraries and loads any that export:
//! - `janus_plugin_abi_version`
//! - `janus_plugin_create` (or a per-plugin fallback symbol)

use crate::config::JanusConfig;
use janus_plugin_api::{
    from_ffi_plugin, JanusPlugin, PluginCallbacks, PluginOpaqueHandle, PLUGIN_LOADER_ABI_VERSION,
};
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

/// Type signature for ABI version symbol.
type PluginAbiVersionFn = unsafe extern "C" fn() -> u32;

/// Type signature for the plugin factory function.
type PluginCreateFn = unsafe extern "C" fn() -> PluginOpaqueHandle;

fn derive_plugin_symbol(path: &Path, suffix: &str) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    let stem = stem.strip_prefix("lib").unwrap_or(&stem);
    if stem.is_empty() {
        return None;
    }
    Some(format!("{}_{}", stem.replace('-', "_"), suffix))
}

fn derive_plugin_create_symbol(path: &Path) -> Option<String> {
    derive_plugin_symbol(path, "create")
}

fn derive_plugin_abi_symbol(path: &Path) -> Option<String> {
    derive_plugin_symbol(path, "abi_version")
}

/// Registry of all loaded plugins, keyed by package name.
#[derive(Default)]
pub struct PluginRegistry {
    plugins: HashMap<String, LoadedPlugin>,
}

impl PluginRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
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

            let mut abi_symbol = "janus_plugin_abi_version".to_string();
            let abi_fn: libloading::Symbol<PluginAbiVersionFn> = match lib
                .get(b"janus_plugin_abi_version")
            {
                Ok(symbol) => symbol,
                Err(primary_err) => {
                    let fallback_symbol = derive_plugin_abi_symbol(path).ok_or_else(|| {
                        crate::Error::PluginLoad(format!(
                            "dlsym janus_plugin_abi_version in {}: {}",
                            path.display(),
                            primary_err
                        ))
                    })?;

                    debug!(
                        path = %path.display(),
                        fallback_symbol = %fallback_symbol,
                        "primary plugin ABI symbol missing; trying fallback"
                    );
                    abi_symbol = fallback_symbol.clone();

                    lib.get(fallback_symbol.as_bytes())
                        .map_err(|fallback_err| {
                            crate::Error::PluginLoad(format!(
                                "dlsym janus_plugin_abi_version in {}: {}; fallback dlsym {} failed: {}",
                                path.display(),
                                primary_err,
                                fallback_symbol,
                                fallback_err
                            ))
                        })?
                }
            };
            let abi_version = abi_fn();
            if abi_version != PLUGIN_LOADER_ABI_VERSION {
                return Err(crate::Error::PluginLoad(format!(
                    "plugin ABI mismatch (symbol {}) in {}: expected {}, got {}",
                    abi_symbol,
                    path.display(),
                    PLUGIN_LOADER_ABI_VERSION,
                    abi_version
                )));
            }

            let mut create_symbol = "janus_plugin_create".to_string();
            let create_fn: libloading::Symbol<PluginCreateFn> = match lib
                .get(b"janus_plugin_create")
            {
                Ok(symbol) => symbol,
                Err(primary_err) => {
                    let fallback_symbol = derive_plugin_create_symbol(path).ok_or_else(|| {
                        crate::Error::PluginLoad(format!(
                            "dlsym janus_plugin_create in {}: {}",
                            path.display(),
                            primary_err
                        ))
                    })?;

                    debug!(
                        path = %path.display(),
                        fallback_symbol = %fallback_symbol,
                        "primary plugin create symbol missing; trying fallback"
                    );
                    create_symbol = fallback_symbol.clone();

                    lib.get(fallback_symbol.as_bytes())
                        .map_err(|fallback_err| {
                            crate::Error::PluginLoad(format!(
                                "dlsym janus_plugin_create in {}: {}; fallback dlsym {} failed: {}",
                                path.display(),
                                primary_err,
                                fallback_symbol,
                                fallback_err
                            ))
                        })?
                }
            };

            let raw = create_fn();
            if raw.is_null() {
                return Err(crate::Error::PluginLoad(format!(
                    "{} returned null in {}",
                    create_symbol,
                    path.display(),
                )));
            }

            let plugin = from_ffi_plugin(raw);
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

    #[test]
    fn derive_symbol_from_standard_lib_name() {
        let path = Path::new("/plugins/libjanus_plugin_echotest.so");
        assert_eq!(
            derive_plugin_create_symbol(path).as_deref(),
            Some("janus_plugin_echotest_create")
        );
    }

    #[test]
    fn derive_symbol_handles_dashes() {
        let path = Path::new("/plugins/libjanus-plugin-textroom.so");
        assert_eq!(
            derive_plugin_create_symbol(path).as_deref(),
            Some("janus_plugin_textroom_create")
        );
    }

    #[test]
    fn derive_abi_symbol_from_standard_lib_name() {
        let path = Path::new("/plugins/libjanus_plugin_echotest.so");
        assert_eq!(
            derive_plugin_abi_symbol(path).as_deref(),
            Some("janus_plugin_echotest_abi_version")
        );
    }
}
