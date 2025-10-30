//! OpDriver pattern for async operation management.
//!
//! This module implements Deno-inspired OpDriver for managing async operations
//! with support for eager completion, lazy polling, and deferred resolution.

mod futures_unordered_driver;
mod pending_op;
mod submission_queue;

pub use futures_unordered_driver::FuturesUnorderedDriver;
// Re-exports for internal use by driver implementations
#[allow(unused_imports)]
pub(crate) use pending_op::{OpFuture, PendingOp, PendingOpInfo};
#[allow(unused_imports)]
pub(crate) use submission_queue::{QueuedOp, SubmissionQueue};

use std::future::Future;
use std::task::{Context, Poll};

/// Type alias for Promise IDs (matches JavaScript promise ring system)
pub type PromiseId = u32;

/// Type alias for Op IDs (operation identifiers)
pub type OpId = u32;

/// Scheduling modes for async operations.
///
/// These modes control when and how operations are polled:
/// - Eager: Poll once immediately, queue if not ready
/// - Lazy: Always queue without immediate poll
/// - Deferred: Queue for later batch processing
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpScheduling {
    /// Poll the future once immediately. If ready, return result; otherwise queue.
    Eager,
    /// Queue the future without polling immediately.
    Lazy,
    /// Queue the future for deferred/batch processing.
    Deferred,
}

/// Result wrapper for operations.
///
/// Currently wraps JSON values to maintain compatibility with existing op system.
/// In the future, this can be extended to support serde_v8 for direct V8 value mapping.
#[derive(Clone, Debug)]
pub enum OpResult {
    /// Successful operation result
    Ok(serde_json::Value),
    /// Operation error
    Err(String),
}

impl OpResult {
    /// Convert to Result type
    pub fn into_result(self) -> Result<serde_json::Value, String> {
        match self {
            OpResult::Ok(v) => Ok(v),
            OpResult::Err(e) => Err(e),
        }
    }

    /// Create from Result type
    pub fn from_result(result: Result<serde_json::Value, String>) -> Self {
        match result {
            Ok(v) => OpResult::Ok(v),
            Err(e) => OpResult::Err(e),
        }
    }
}

/// Statistics about in-flight operations.
#[derive(Debug, Default)]
pub struct OpInflightStats {
    /// Number of pending operations
    pub pending_count: usize,
    /// Total operations submitted
    pub total_submitted: usize,
    /// Total operations completed
    pub total_completed: usize,
}

/// Core trait for operation drivers.
///
/// OpDriver manages the lifecycle of async operations submitted from JavaScript.
/// It handles scheduling, polling, and result delivery for operations.
pub trait OpDriver: Default {
    /// Submit an infallible operation (cannot fail).
    ///
    /// Returns Some(R) if the operation completed eagerly (EAGER mode only),
    /// None if the operation was queued for later polling.
    fn submit_op_infallible<R>(
        &self,
        op_id: OpId,
        promise_id: PromiseId,
        scheduling: OpScheduling,
        op: impl Future<Output = R> + 'static,
    ) -> Option<R>
    where
        R: 'static + serde::Serialize;

    /// Submit a fallible operation (can return Result).
    ///
    /// Returns Some(Result<R, E>) if the operation completed eagerly (EAGER mode only),
    /// None if the operation was queued for later polling.
    fn submit_op_fallible<R, E>(
        &self,
        op_id: OpId,
        promise_id: PromiseId,
        scheduling: OpScheduling,
        op: impl Future<Output = Result<R, E>> + 'static,
    ) -> Option<Result<R, E>>
    where
        R: 'static + serde::Serialize,
        E: 'static + std::fmt::Display;

    /// Poll for ready operations.
    ///
    /// Returns Poll::Ready with (PromiseId, OpId, OpResult) when an operation completes.
    /// Returns Poll::Pending if no operations are ready.
    fn poll_ready(&self, cx: &mut Context) -> Poll<(PromiseId, OpId, OpResult)>;

    /// Get the number of in-flight operations.
    fn len(&self) -> usize;

    /// Check if there are no in-flight operations.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Shutdown the driver, preventing new operations from being polled.
    fn shutdown(&self);

    /// Get statistics about in-flight operations.
    fn stats(&self) -> OpInflightStats;

    /// Check whether the driver has been shut down.
    fn is_shutdown(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_op_result_conversions() {
        // Test Ok conversion
        let result = OpResult::from_result(Ok(serde_json::json!({"value": 42})));
        match result {
            OpResult::Ok(v) => assert_eq!(v["value"], 42),
            OpResult::Err(_) => panic!("Expected Ok variant"),
        }

        // Test Err conversion
        let result = OpResult::from_result(Err("error".to_string()));
        match result {
            OpResult::Ok(_) => panic!("Expected Err variant"),
            OpResult::Err(e) => assert_eq!(e, "error"),
        }

        // Test into_result
        let op_result = OpResult::Ok(serde_json::json!({"status": "ok"}));
        let result = op_result.into_result();
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["status"], "ok");
    }

    #[test]
    fn test_op_scheduling_variants() {
        // Ensure all scheduling modes are available
        let eager = OpScheduling::Eager;
        let lazy = OpScheduling::Lazy;
        let deferred = OpScheduling::Deferred;

        assert_ne!(eager, lazy);
        assert_ne!(lazy, deferred);
        assert_ne!(eager, deferred);
    }

    #[test]
    fn test_op_inflight_stats_default() {
        let stats = OpInflightStats::default();
        assert_eq!(stats.pending_count, 0);
        assert_eq!(stats.total_submitted, 0);
        assert_eq!(stats.total_completed, 0);
    }
}
