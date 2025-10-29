//! Runtime configuration for isolate-per-tenant execution.
//!
//! This module defines the configuration structure for JavaScript runtimes,
//! including heap limits, permission manifests, and bootstrap options.

use std::time::Duration;

/// Permission types for runtime operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permission {
    /// Allow timer operations (setTimeout, setInterval)
    Timers,
    /// Allow network access with optional host restriction
    Net(Option<String>),
    /// Allow file system access with optional path restriction
    File(Option<String>),
}

/// Runtime configuration for a single JavaScript isolate.
#[derive(Debug, Clone, Default)]
pub struct RuntimeConfig {
    /// Maximum heap size in bytes (None = V8 default)
    pub max_heap_size: Option<usize>,

    /// Initial heap size in bytes (None = V8 default)
    pub initial_heap_size: Option<usize>,

    /// Permissions granted to this runtime
    pub permissions: Vec<Permission>,

    /// Optional timeout for script execution
    pub execution_timeout: Option<Duration>,

    /// Bootstrap script to run on startup
    pub bootstrap_script: Option<String>,
}

impl RuntimeConfig {
    /// Create a new runtime configuration with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum heap size in bytes.
    pub fn with_max_heap_size(mut self, size: usize) -> Self {
        self.max_heap_size = Some(size);
        self
    }

    /// Set initial heap size in bytes.
    pub fn with_initial_heap_size(mut self, size: usize) -> Self {
        self.initial_heap_size = Some(size);
        self
    }

    /// Add a permission to the runtime.
    pub fn with_permission(mut self, permission: Permission) -> Self {
        self.permissions.push(permission);
        self
    }

    /// Set execution timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.execution_timeout = Some(timeout);
        self
    }

    /// Set bootstrap script.
    pub fn with_bootstrap(mut self, script: String) -> Self {
        self.bootstrap_script = Some(script);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RuntimeConfig::default();
        assert!(config.max_heap_size.is_none());
        assert!(config.initial_heap_size.is_none());
        assert!(config.permissions.is_empty());
        assert!(config.execution_timeout.is_none());
        assert!(config.bootstrap_script.is_none());
    }

    #[test]
    fn test_config_builder() {
        let config = RuntimeConfig::new()
            .with_max_heap_size(100 * 1024 * 1024)
            .with_permission(Permission::Timers)
            .with_permission(Permission::Net(Some("example.com".to_string())))
            .with_permission(Permission::File(Some("/tmp".to_string())))
            .with_timeout(Duration::from_secs(30));

        assert_eq!(config.max_heap_size, Some(100 * 1024 * 1024));
        assert_eq!(config.permissions.len(), 3);
        assert!(config.permissions.contains(&Permission::Timers));
        assert!(config
            .permissions
            .contains(&Permission::File(Some("/tmp".to_string()))));
        assert_eq!(config.execution_timeout, Some(Duration::from_secs(30)));
    }
}
