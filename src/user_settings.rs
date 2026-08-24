//! A simple persistent key-value store for user settings.
//!
//! This module provides a platform-agnostic interface for storing and retrieving
//! string-based key-value pairs. It uses the `preferences` crate on native
//! platforms and `gloo-storage` on wasm platforms.

#[cfg(not(target_arch = "wasm32"))]
use preferences::{AppInfo, Preferences};

#[cfg(target_arch = "wasm32")]
use gloo_storage::{LocalStorage, Storage};

pub mod consts {
    pub const SOLVER_ANALYZE_LINES: &str = "solver.analyze_lines";
    pub const SOLVER_DETECT_ERRORS: &str = "solver.detect_errors";
    pub const SOLVER_INFER_BACKGROUND: &str = "solver.infer_background";
    pub const SOLVER_RENDER_STYLE: &str = "solver.render_style";
    pub const EDITOR_AUTHOR_NAME: &str = "editor.author_name";
    pub const EDITOR_AUTO_SOLVE: &str = "editor.auto_solve";
}

#[cfg(not(target_arch = "wasm32"))]
const APP_INFO: AppInfo = AppInfo {
    name: "number-loom",
    author: "Paul Stansifer",
};

/// A struct providing access to the persistent key-value store.
pub struct UserSettings;

impl UserSettings {
    /// Retrieves a value from the store for the given key.
    ///
    /// Returns `Some(String)` if the key exists, otherwise `None`.
    pub fn get(key: &str) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let map = match preferences::PreferencesMap::<String>::load(&APP_INFO, "user_settings")
            {
                Ok(map) => map,
                // If loading fails (e.g., file not found), return None.
                Err(_) => return None,
            };
            map.get(key).cloned()
        }

        #[cfg(target_arch = "wasm32")]
        {
            match LocalStorage::get(key) {
                Ok(value) => Some(value),
                // If getting fails (e.g., key not found), return None.
                Err(_) => None,
            }
        }
    }

    /// Retrieves a boolean value from the store, defaulting to `false` for a key that's missing
    /// (or that holds something that isn't a bool).
    pub fn get_bool(key: &str) -> bool {
        Self::get(key)
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(false)
    }

    /// Sets a value in the store for the given key.
    ///
    /// This will overwrite any existing value for the same key.
    pub fn set(key: &str, value: &str) -> anyhow::Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Load the existing map, or create a new one if it doesn't exist.
            let mut map = preferences::PreferencesMap::<String>::load(&APP_INFO, "user_settings")
                .unwrap_or_default();
            map.insert(key.to_string(), value.to_string());
            map.save(&APP_INFO, "user_settings")
                .map_err(|e| anyhow::anyhow!(e))
        }

        #[cfg(target_arch = "wasm32")]
        {
            LocalStorage::set(key, value).map_err(|e| anyhow::anyhow!(e.to_string()))
        }
    }
}
