//! Python-facing runtime handle with shutdown guard.
//!
//! This module provides the `RuntimeHandle` type that Python code uses to interact
//! with the runtime thread. It manages the lifecycle and ensures clean shutdown.

use super::config::RuntimeConfig;
use super::runner::{spawn_runtime_thread, HostCommand};
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc as async_mpsc;

/// Handle to a running JavaScript runtime.
///
/// This handle allows Python code to communicate with the runtime thread
/// via a message-passing channel. The handle ensures proper cleanup on drop.
/// Multiple clones of the handle can exist, all sharing access to the same runtime.
#[derive(Clone)]
pub struct RuntimeHandle {
    /// Command sender to the runtime thread
    tx: Option<async_mpsc::UnboundedSender<HostCommand>>,
    /// Whether the runtime has been shut down
    shutdown: Arc<Mutex<bool>>,
}

impl RuntimeHandle {
    /// Spawn a new runtime with the given configuration.
    pub fn spawn(config: RuntimeConfig) -> Result<Self, String> {
        let tx = spawn_runtime_thread(config)?;

        Ok(Self {
            tx: Some(tx),
            shutdown: Arc::new(Mutex::new(false)),
        })
    }

    /// Evaluate JavaScript code synchronously.
    ///
    pub fn eval_sync(&self, code: &str) -> Result<String, String> {
        if *self.shutdown.lock().unwrap() {
            return Err("Runtime has been shut down".to_string());
        }

        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| "Runtime has been shut down".to_string())?;

        let (result_tx, result_rx) = mpsc::channel();

        tx.send(HostCommand::Eval {
            code: code.to_string(),
            responder: result_tx,
        })
        .map_err(|_| "Failed to send command to runtime".to_string())?;

        result_rx
            .recv()
            .map_err(|_| "Failed to receive result from runtime".to_string())?
    }

    /// Evaluate JavaScript code asynchronously with optional timeout.
    ///
    /// This method supports promises and async JavaScript code. The timeout
    /// is specified in milliseconds.
    pub async fn eval_async(&self, code: &str, timeout_ms: Option<u64>) -> Result<String, String> {
        if *self.shutdown.lock().unwrap() {
            return Err("Runtime has been shut down".to_string());
        }

        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| "Runtime has been shut down".to_string())?;

        let (result_tx, result_rx) = mpsc::channel();

        tx.send(HostCommand::EvalAsync {
            code: code.to_string(),
            timeout_ms,
            responder: result_tx,
        })
        .map_err(|_| "Failed to send command to runtime".to_string())?;

        // Use tokio blocking task to wait for sync channel
        tokio::task::spawn_blocking(move || {
            result_rx
                .recv()
                .map_err(|_| "Failed to receive result from runtime".to_string())
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))??
    }

    /// Check if the runtime has been shut down.
    pub fn is_shutdown(&self) -> bool {
        *self.shutdown.lock().unwrap()
    }

    /// Shut down the runtime gracefully.
    pub fn close(&mut self) -> Result<(), String> {
        let mut shutdown_guard = self.shutdown.lock().unwrap();
        if *shutdown_guard {
            return Ok(()); // Already shut down
        }

        if let Some(tx) = self.tx.take() {
            // Only set shutdown flag if we successfully send the command
            // This ensures only one handle actually shuts down the runtime
            tx.send(HostCommand::Shutdown)
                .map_err(|_| "Failed to send shutdown command".to_string())?;

            *shutdown_guard = true;
        }

        Ok(())
    }

    /// Register an op with the runtime.
    ///
    /// This sends a RegisterOp command to the runtime thread and waits for
    /// the op ID to be returned.
    pub fn register_op(
        &self,
        name: String,
        mode: super::ops::OpMode,
        permissions: Vec<super::ops::Permission>,
        handler: super::ops::OpHandler,
    ) -> Result<u32, String> {
        if *self.shutdown.lock().unwrap() {
            return Err("Runtime has been shut down".to_string());
        }

        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| "Runtime has been shut down".to_string())?;

        let (result_tx, result_rx) = mpsc::channel();

        tx.send(HostCommand::RegisterOp {
            name,
            mode,
            permissions,
            handler,
            responder: result_tx,
        })
        .map_err(|_| "Failed to send register_op command".to_string())?;

        result_rx
            .recv()
            .map_err(|_| "Failed to receive op registration result".to_string())?
    }

    /// Create a new context in this runtime.
    pub fn create_context(&self) -> Result<usize, String> {
        if *self.shutdown.lock().unwrap() {
            return Err("Runtime has been shut down".to_string());
        }

        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| "Runtime has been shut down".to_string())?;

        let (result_tx, result_rx) = mpsc::channel();

        tx.send(HostCommand::CreateContext {
            responder: result_tx,
        })
        .map_err(|_| "Failed to send create_context command".to_string())?;

        result_rx
            .recv()
            .map_err(|_| "Failed to receive context creation result".to_string())?
    }

    /// Drop a context by ID.
    pub fn drop_context(&self, context_id: usize) -> Result<(), String> {
        if let Some(tx) = self.tx.as_ref() {
            tx.send(HostCommand::DropContext { context_id })
                .map_err(|_| "Failed to send drop_context command".to_string())?;
        }
        Ok(())
    }

    /// Evaluate code in a specific context.
    pub fn eval_in_context(&self, context_id: usize, code: &str) -> Result<String, String> {
        if *self.shutdown.lock().unwrap() {
            return Err("Runtime has been shut down".to_string());
        }

        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| "Runtime has been shut down".to_string())?;

        let (result_tx, result_rx) = mpsc::channel();

        tx.send(HostCommand::EvalInContext {
            context_id,
            code: code.to_string(),
            responder: result_tx,
        })
        .map_err(|_| "Failed to send eval_in_context command".to_string())?;

        result_rx
            .recv()
            .map_err(|_| "Failed to receive eval result".to_string())?
    }

    /// Evaluate code asynchronously in a specific context.
    pub async fn eval_in_context_async(
        &self,
        context_id: usize,
        code: &str,
        timeout_ms: Option<u64>,
    ) -> Result<String, String> {
        if *self.shutdown.lock().unwrap() {
            return Err("Runtime has been shut down".to_string());
        }

        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| "Runtime has been shut down".to_string())?;

        let (result_tx, result_rx) = mpsc::channel();

        tx.send(HostCommand::EvalInContextAsync {
            context_id,
            code: code.to_string(),
            timeout_ms,
            responder: result_tx,
        })
        .map_err(|_| "Failed to send eval_in_context_async command".to_string())?;

        tokio::task::spawn_blocking(move || {
            result_rx
                .recv()
                .map_err(|_| "Failed to receive eval result".to_string())
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))??
    }
}

// Drop implementation deliberately does NOT close the runtime automatically.
// This is because RuntimeHandle can be cloned, and we don't want clones
// (e.g., those created for async operations) to trigger shutdown.
// The Python layer ensures proper cleanup via context managers or explicit close() calls.

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_runtime_handle_spawn() {
        let config = RuntimeConfig::default();
        let handle = RuntimeHandle::spawn(config);
        assert!(handle.is_ok());
    }

    #[test]
    fn test_runtime_handle_eval() {
        let config = RuntimeConfig::default();
        let handle = RuntimeHandle::spawn(config).unwrap();

        let result = handle.eval_sync("3 + 3");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "6");
    }

    #[test]
    fn test_runtime_handle_close() {
        let config = RuntimeConfig::default();
        let mut handle = RuntimeHandle::spawn(config).unwrap();

        assert!(!handle.is_shutdown());

        let close_result = handle.close();
        assert!(close_result.is_ok());
        assert!(handle.is_shutdown());

        // Should be able to close again without error
        let close_again = handle.close();
        assert!(close_again.is_ok());
    }

    #[test]
    fn test_runtime_handle_drop() {
        let config = RuntimeConfig::default();
        let handle = RuntimeHandle::spawn(config).unwrap();

        // Drop the handle
        drop(handle);

        // Give the thread time to shutdown
        thread::sleep(Duration::from_millis(100));

        // If we got here without hanging, the drop worked
    }

    #[test]
    fn test_eval_after_shutdown() {
        let config = RuntimeConfig::default();
        let mut handle = RuntimeHandle::spawn(config).unwrap();

        handle.close().unwrap();

        let result = handle.eval_sync("1 + 1");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Runtime has been shut down");
    }

    #[test]
    fn test_multiple_evals() {
        let config = RuntimeConfig::default();
        let handle = RuntimeHandle::spawn(config).unwrap();

        let result1 = handle.eval_sync("var x = 10; x");
        assert_eq!(result1.unwrap(), "10");

        let result2 = handle.eval_sync("x + 5");
        assert_eq!(result2.unwrap(), "15");

        let result3 = handle.eval_sync("x * 2");
        assert_eq!(result3.unwrap(), "20");
    }

    #[tokio::test]
    async fn test_eval_async_with_promise() {
        let config = RuntimeConfig::default();
        let handle = RuntimeHandle::spawn(config).unwrap();

        let result = handle.eval_async("Promise.resolve(123)", None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "123");
    }

    #[tokio::test]
    async fn test_eval_async_with_timeout() {
        let config = RuntimeConfig::default();
        let handle = RuntimeHandle::spawn(config).unwrap();

        let result = handle.eval_async("new Promise(() => {})", Some(100)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timeout"));
    }
}
