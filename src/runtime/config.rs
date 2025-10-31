//! Runtime configuration for isolate-per-tenant execution.
//!
//! This module defines the configuration structure for JavaScript runtimes,
//! including heap limits, permission manifests, and bootstrap options.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use std::time::Duration;

/// Permission types for runtime operations.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Surface area for host integrations even if Rust impls lag behind.
pub enum Permission {
    /// Allow timer operations (setTimeout, setInterval)
    Timers,
    /// Allow network access with optional host restriction
    Net(Option<String>),
    /// Allow file system access with optional path restriction
    File(Option<String>),
}

/// Runtime configuration for a single JavaScript isolate.
#[pyclass(module = "jsrun")]
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // Exposed to Python bindings; some fields are not wired yet in Rust.
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

#[pymethods]
impl RuntimeConfig {
    /// Create a new runtime configuration with default settings.
    #[new]
    #[pyo3(signature = (
        max_heap_size = None,
        initial_heap_size = None,
        bootstrap = None,
        timeout = None,
        permissions = None
    ))]
    fn new(
        max_heap_size: Option<usize>,
        initial_heap_size: Option<usize>,
        bootstrap: Option<String>,
        timeout: Option<&Bound<'_, PyAny>>,
        permissions: Option<Vec<(String, Option<String>)>>,
    ) -> PyResult<Self> {
        let mut config = RuntimeConfig::default();

        // Set max heap size if provided
        if let Some(size) = max_heap_size {
            config.max_heap_size = Some(size);
        }

        // Set initial heap size if provided
        if let Some(size) = initial_heap_size {
            config.initial_heap_size = Some(size);
        }

        // Set bootstrap script if provided
        if let Some(script) = bootstrap {
            config.bootstrap_script = Some(script);
        }

        // Set timeout if provided
        if let Some(timeout_value) = timeout {
            let duration = if let Ok(seconds) = timeout_value.extract::<f64>() {
                Duration::from_secs_f64(seconds)
            } else if let Ok(seconds) = timeout_value.extract::<u64>() {
                Duration::from_secs(seconds)
            } else if let Ok(seconds) = timeout_value.extract::<i64>() {
                Duration::from_secs(seconds as u64)
            } else {
                // Try to extract as timedelta
                let py = timeout_value.py();
                let timedelta = py.import("datetime")?.getattr("timedelta")?;
                if timeout_value.is_instance(&timedelta)? {
                    let total_seconds: f64 =
                        timeout_value.getattr("total_seconds")?.call0()?.extract()?;
                    Duration::from_secs_f64(total_seconds)
                } else {
                    return Err(PyValueError::new_err(
                        "Timeout must be a number (seconds) or datetime.timedelta object",
                    ));
                }
            };
            config.execution_timeout = Some(duration);
        }

        // Add permissions if provided
        if let Some(permissions_list) = permissions {
            for (kind, scope) in permissions_list {
                let permission = match kind.as_str() {
                    "timers" => Permission::Timers,
                    "net" => Permission::Net(scope),
                    "file" => Permission::File(scope),
                    other => {
                        return Err(PyValueError::new_err(format!(
                            "Unknown permission '{}', expected 'timers', 'net', or 'file'",
                            other
                        )))
                    }
                };
                config.permissions.push(permission);
            }
        }

        Ok(config)
    }

    /// Get maximum heap size in bytes.
    #[getter]
    fn max_heap_size(&self) -> Option<usize> {
        self.max_heap_size
    }

    /// Set maximum heap size in bytes.
    #[setter]
    fn set_max_heap_size(&mut self, bytes: usize) {
        self.max_heap_size = Some(bytes);
    }

    /// Get initial heap size in bytes.
    #[getter]
    fn initial_heap_size(&self) -> Option<usize> {
        self.initial_heap_size
    }

    /// Set initial heap size in bytes.
    #[setter]
    fn set_initial_heap_size(&mut self, bytes: usize) {
        self.initial_heap_size = Some(bytes);
    }

    /// Get bootstrap script.
    #[getter]
    fn bootstrap(&self) -> Option<String> {
        self.bootstrap_script.clone()
    }

    /// Set bootstrap script.
    #[setter]
    fn set_bootstrap(&mut self, source: String) {
        self.bootstrap_script = Some(source);
    }

    /// Get execution timeout in seconds.
    #[getter]
    fn timeout(&self) -> Option<f64> {
        self.execution_timeout.map(|d| d.as_secs_f64())
    }

    /// Set execution timeout.
    /// Accepts float/int as seconds or datetime.timedelta object.
    #[setter]
    fn set_timeout<'py>(&mut self, timeout: &Bound<'py, PyAny>) -> PyResult<()> {
        let duration = if let Ok(seconds) = timeout.extract::<f64>() {
            Duration::from_secs_f64(seconds)
        } else if let Ok(seconds) = timeout.extract::<u64>() {
            Duration::from_secs(seconds)
        } else if let Ok(seconds) = timeout.extract::<i64>() {
            Duration::from_secs(seconds as u64)
        } else {
            // Try to extract as timedelta
            let py = timeout.py();
            let timedelta = py.import("datetime")?.getattr("timedelta")?;
            if timeout.is_instance(&timedelta)? {
                let total_seconds: f64 = timeout.getattr("total_seconds")?.call0()?.extract()?;
                Duration::from_secs_f64(total_seconds)
            } else {
                return Err(PyValueError::new_err(
                    "Timeout must be a number (seconds) or datetime.timedelta object",
                ));
            }
        };

        self.execution_timeout = Some(duration);
        Ok(())
    }

    /// Get the list of permissions as tuples (kind, scope).
    #[getter]
    fn permissions(&self) -> Vec<(String, Option<String>)> {
        self.permissions
            .iter()
            .map(|p| match p {
                Permission::Timers => ("timers".to_string(), None),
                Permission::Net(scope) => ("net".to_string(), scope.clone()),
                Permission::File(scope) => ("file".to_string(), scope.clone()),
            })
            .collect()
    }

    /// Set permissions for the runtime.
    /// Accepts a list of tuples [(kind, scope), ...]
    #[setter]
    fn set_permissions(&mut self, permissions: Vec<(String, Option<String>)>) -> PyResult<()> {
        // Clear existing permissions
        self.permissions.clear();

        // Add new permissions
        for (kind, scope) in permissions {
            let permission = match kind.as_str() {
                "timers" => Permission::Timers,
                "net" => Permission::Net(scope),
                "file" => Permission::File(scope),
                other => {
                    return Err(PyValueError::new_err(format!(
                        "Unknown permission '{}', expected 'timers', 'net', or 'file'",
                        other
                    )))
                }
            };
            self.permissions.push(permission);
        }

        Ok(())
    }

    fn __repr__(&self) -> String {
        format!("RuntimeConfig({:?})", self)
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

    #[allow(clippy::field_reassign_with_default)]
    #[test]
    fn test_config_builder() {
        let mut config = RuntimeConfig::default();
        config.max_heap_size = Some(100 * 1024 * 1024);
        config.permissions.push(Permission::Timers);
        config
            .permissions
            .push(Permission::Net(Some("example.com".to_string())));
        config
            .permissions
            .push(Permission::File(Some("/tmp".to_string())));
        config.execution_timeout = Some(Duration::from_secs(30));

        assert_eq!(config.max_heap_size, Some(100 * 1024 * 1024));
        assert_eq!(config.permissions.len(), 3);
        assert!(config.permissions.contains(&Permission::Timers));
        assert!(config
            .permissions
            .contains(&Permission::File(Some("/tmp".to_string()))));
        assert_eq!(config.execution_timeout, Some(Duration::from_secs(30)));
    }
}
