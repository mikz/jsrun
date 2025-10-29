//! Op system for host-to-JavaScript communication.
//!
//! This module implements the operations (ops) system that allows Rust/Python code
//! to expose functions to JavaScript with permission checking and async support.

use super::config;
use std::collections::HashMap;
use std::sync::Arc;

/// Permission types for ops.
///
/// Each op can require zero or more permissions. The runtime enforces these
/// permissions before executing the op.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Allow network access (optionally restricted to specific hosts)
    Net(Option<String>),
    /// Allow file system access (optionally restricted to specific paths)
    File(Option<String>),
    /// Allow timer/setTimeout operations
    Timers,
    /// Allow environment variable access
    Env,
    /// Allow process/subprocess operations
    Process,
    /// Custom permission with arbitrary name
    Custom(String),
}

impl Permission {
    /// Check if this permission grants access to the requested resource.
    ///
    /// For permissions with optional restrictions (Net, File), this checks
    /// if the requested resource matches the allowed pattern.
    pub fn grants_access_to(&self, requested: &Permission) -> bool {
        match (self, requested) {
            // Exact match
            (Permission::Timers, Permission::Timers) => true,
            (Permission::Env, Permission::Env) => true,
            (Permission::Process, Permission::Process) => true,

            // Network permissions
            (Permission::Net(None), Permission::Net(_)) => true, // Allow all
            (Permission::Net(Some(allowed)), Permission::Net(Some(req))) => {
                // Simple prefix match for now
                req.starts_with(allowed.as_str())
            }

            // File permissions
            (Permission::File(None), Permission::File(_)) => true, // Allow all
            (Permission::File(Some(allowed)), Permission::File(Some(req))) => {
                // Simple prefix match for now
                req.starts_with(allowed.as_str())
            }

            // Custom permissions
            (Permission::Custom(a), Permission::Custom(b)) => a == b,

            // No match
            _ => false,
        }
    }
}

impl From<&config::Permission> for Permission {
    fn from(permission: &config::Permission) -> Self {
        match permission {
            config::Permission::Timers => Permission::Timers,
            config::Permission::Net(host) => Permission::Net(host.clone()),
            config::Permission::File(path) => Permission::File(path.clone()),
        }
    }
}

/// Op execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpMode {
    /// Synchronous operation - returns immediately
    Sync,
    /// Asynchronous operation - returns a promise
    Async,
}

/// Type alias for op handler functions.
///
/// Handlers receive arguments as JSON and return a result as JSON.
pub type OpHandler =
    Arc<dyn Fn(Vec<serde_json::Value>) -> Result<serde_json::Value, String> + Send + Sync>;

/// Metadata for a registered op.
#[derive(Clone)]
pub struct OpMetadata {
    /// Op ID (index in registry)
    pub id: u32,
    /// Op name
    pub name: String,
    /// Execution mode (sync/async)
    pub mode: OpMode,
    /// Required permissions
    pub permissions: Vec<Permission>,
    /// Handler function
    pub handler: OpHandler,
}

/// Registry of operations.
///
/// The OpRegistry stores metadata for all registered ops and provides
/// permission-checked access to op handlers.
pub struct OpRegistry {
    /// Map from op name to metadata
    ops_by_name: HashMap<String, OpMetadata>,
    /// Map from op ID to metadata
    ops_by_id: HashMap<u32, OpMetadata>,
    /// Next available op ID
    next_id: u32,
    /// Granted permissions for this runtime
    granted_permissions: Vec<Permission>,
}

impl OpRegistry {
    /// Create a new empty op registry.
    pub fn new() -> Self {
        Self {
            ops_by_name: HashMap::new(),
            ops_by_id: HashMap::new(),
            next_id: 0,
            granted_permissions: Vec::new(),
        }
    }

    /// Grant a permission to the runtime.
    pub fn grant_permission(&mut self, permission: Permission) {
        self.granted_permissions.push(permission);
    }

    /// Check if a permission is granted.
    pub fn has_permission(&self, requested: &Permission) -> bool {
        self.granted_permissions
            .iter()
            .any(|p| p.grants_access_to(requested))
    }

    /// Register a new op.
    ///
    /// Returns the op ID or an error if an op with the same name already exists.
    pub fn register_op(
        &mut self,
        name: String,
        mode: OpMode,
        permissions: Vec<Permission>,
        handler: OpHandler,
    ) -> Result<u32, String> {
        if self.ops_by_name.contains_key(&name) {
            return Err(format!("Op '{}' is already registered", name));
        }

        let id = self.next_id;
        self.next_id += 1;

        let metadata = OpMetadata {
            id,
            name: name.clone(),
            mode,
            permissions,
            handler,
        };

        self.ops_by_name.insert(name.clone(), metadata.clone());
        self.ops_by_id.insert(id, metadata);

        Ok(id)
    }

    /// Get op metadata by name.
    pub fn get_by_name(&self, name: &str) -> Option<&OpMetadata> {
        self.ops_by_name.get(name)
    }

    /// Get op metadata by ID.
    pub fn get_by_id(&self, id: u32) -> Option<&OpMetadata> {
        self.ops_by_id.get(&id)
    }

    /// Check permissions and call an op.
    pub fn call_op(
        &self,
        op_id: u32,
        args: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let metadata = self
            .get_by_id(op_id)
            .ok_or_else(|| format!("Op {} not found", op_id))?;

        // Check permissions
        for required_perm in &metadata.permissions {
            if !self.has_permission(required_perm) {
                return Err(format!(
                    "Permission denied: op '{}' requires {:?}",
                    metadata.name, required_perm
                ));
            }
        }

        // Call the handler
        (metadata.handler)(args)
    }

    /// Get number of registered ops.
    pub fn len(&self) -> usize {
        self.ops_by_name.len()
    }

    /// Check if registry is empty.
    pub fn is_empty(&self) -> bool {
        self.ops_by_name.is_empty()
    }
}

impl Default for OpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_grants_access() {
        // Timers permission
        assert!(Permission::Timers.grants_access_to(&Permission::Timers));
        assert!(!Permission::Timers.grants_access_to(&Permission::Env));

        // File permission
        assert!(Permission::File(None)
            .grants_access_to(&Permission::File(Some("/var/log".to_string()))));
        let file_perm = Permission::File(Some("/tmp".to_string()));
        assert!(file_perm.grants_access_to(&Permission::File(Some("/tmp/data".to_string()))));
        assert!(!file_perm.grants_access_to(&Permission::File(Some("/etc".to_string()))));

        // Network permission - allow all
        assert!(Permission::Net(None)
            .grants_access_to(&Permission::Net(Some("example.com".to_string()))));
        assert!(Permission::Net(None).grants_access_to(&Permission::Net(None)));

        // Network permission - specific host
        let net_perm = Permission::Net(Some("example.com".to_string()));
        assert!(net_perm.grants_access_to(&Permission::Net(Some("example.com".to_string()))));
        assert!(net_perm.grants_access_to(&Permission::Net(Some("example.com/api".to_string()))));
        assert!(!net_perm.grants_access_to(&Permission::Net(Some("evil.com".to_string()))));
    }

    #[test]
    fn test_op_registry_basic() {
        let mut registry = OpRegistry::new();
        assert!(registry.is_empty());

        // Register an op
        let handler =
            Arc::new(|args: Vec<serde_json::Value>| Ok(serde_json::json!({ "echo": args })));

        let op_id = registry
            .register_op("test_op".to_string(), OpMode::Sync, vec![], handler)
            .unwrap();

        assert_eq!(op_id, 0);
        assert_eq!(registry.len(), 1);

        // Get by name
        let metadata = registry.get_by_name("test_op").unwrap();
        assert_eq!(metadata.name, "test_op");
        assert_eq!(metadata.mode, OpMode::Sync);
        assert_eq!(metadata.id, op_id);

        // Get by ID
        let metadata = registry.get_by_id(op_id).unwrap();
        assert_eq!(metadata.name, "test_op");
        assert_eq!(metadata.id, op_id);
    }

    #[test]
    fn test_op_registry_duplicate_name() {
        let mut registry = OpRegistry::new();

        let handler = Arc::new(|_: Vec<serde_json::Value>| Ok(serde_json::json!(null)));

        registry
            .register_op("test_op".to_string(), OpMode::Sync, vec![], handler.clone())
            .unwrap();

        let result = registry.register_op("test_op".to_string(), OpMode::Sync, vec![], handler);
        assert!(result.is_err());
    }

    #[test]
    fn test_op_call_with_permissions() {
        let mut registry = OpRegistry::new();

        // Register op that requires network permission
        let handler = Arc::new(|_: Vec<serde_json::Value>| Ok(serde_json::json!("network_data")));

        let op_id = registry
            .register_op(
                "fetch".to_string(),
                OpMode::Async,
                vec![Permission::Net(Some("example.com".to_string()))],
                handler,
            )
            .unwrap();

        // Try to call without permission - should fail
        let result = registry.call_op(op_id, vec![]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Permission denied"));

        // Grant permission
        registry.grant_permission(Permission::Net(Some("example.com".to_string())));

        // Now it should work
        let result = registry.call_op(op_id, vec![]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_op_call_handler() {
        let mut registry = OpRegistry::new();

        // Register op that echoes its arguments
        let handler =
            Arc::new(|args: Vec<serde_json::Value>| Ok(serde_json::json!({ "received": args })));

        let op_id = registry
            .register_op("echo".to_string(), OpMode::Sync, vec![], handler)
            .unwrap();

        let result = registry.call_op(
            op_id,
            vec![serde_json::json!("hello"), serde_json::json!(42)],
        );

        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["received"][0], "hello");
        assert_eq!(value["received"][1], 42);
    }
}
