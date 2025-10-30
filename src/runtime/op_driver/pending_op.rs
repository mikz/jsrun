//! Pending operation metadata and result management.

use super::{OpId, OpResult, PromiseId};
use std::future::Future;
use std::pin::Pin;

/// Information about a pending operation.
///
/// This tracks the promise ID and op ID for operations submitted to the driver.
#[derive(Debug, Clone, Copy)]
pub struct PendingOpInfo {
    pub promise_id: PromiseId,
    pub op_id: OpId,
}

impl PendingOpInfo {
    /// Create new pending op info
    pub fn new(promise_id: PromiseId, op_id: OpId) -> Self {
        Self { promise_id, op_id }
    }
}

/// A completed pending operation with its result.
pub struct PendingOp {
    pub info: PendingOpInfo,
    pub result: OpResult,
}

impl PendingOp {
    /// Create a new pending op with a successful result
    pub fn ok(info: PendingOpInfo, value: serde_json::Value) -> Self {
        Self {
            info,
            result: OpResult::Ok(value),
        }
    }

    /// Create a new pending op with an error result
    pub fn err(info: PendingOpInfo, error: String) -> Self {
        Self {
            info,
            result: OpResult::Err(error),
        }
    }

    /// Create from a Result
    pub fn from_result(info: PendingOpInfo, result: Result<serde_json::Value, String>) -> Self {
        match result {
            Ok(v) => Self::ok(info, v),
            Err(e) => Self::err(info, e),
        }
    }
}

/// Wrapper for a boxed future that can be polled.
///
/// This wraps futures submitted to the driver, converting their output
/// to our JSON-based result type.
pub struct OpFuture {
    inner: Pin<Box<dyn Future<Output = OpResult> + 'static>>,
}

impl OpFuture {
    /// Create from an infallible future
    pub fn from_infallible<F, R>(future: F) -> Self
    where
        F: Future<Output = R> + 'static,
        R: serde::Serialize + 'static,
    {
        let wrapped = async move {
            let result = future.await;
            match serde_json::to_value(result) {
                Ok(v) => OpResult::Ok(v),
                Err(e) => OpResult::Err(format!("Serialization error: {}", e)),
            }
        };
        Self {
            inner: Box::pin(wrapped),
        }
    }

    /// Create from a fallible future (returns Result)
    pub fn from_fallible<F, R, E>(future: F) -> Self
    where
        F: Future<Output = Result<R, E>> + 'static,
        R: serde::Serialize + 'static,
        E: std::fmt::Display + 'static,
    {
        let wrapped = async move {
            match future.await {
                Ok(result) => match serde_json::to_value(result) {
                    Ok(v) => OpResult::Ok(v),
                    Err(e) => OpResult::Err(format!("Serialization error: {}", e)),
                },
                Err(e) => OpResult::Err(e.to_string()),
            }
        };
        Self {
            inner: Box::pin(wrapped),
        }
    }
}

impl Future for OpFuture {
    type Output = OpResult;

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pending_op_info() {
        let info = PendingOpInfo::new(123, 456);
        assert_eq!(info.promise_id, 123);
        assert_eq!(info.op_id, 456);
    }

    #[test]
    fn test_pending_op_ok() {
        let info = PendingOpInfo::new(1, 2);
        let op = PendingOp::ok(info, serde_json::json!({"status": "ok"}));

        match op.result {
            OpResult::Ok(v) => assert_eq!(v["status"], "ok"),
            OpResult::Err(_) => panic!("Expected Ok variant"),
        }
    }

    #[test]
    fn test_pending_op_err() {
        let info = PendingOpInfo::new(1, 2);
        let op = PendingOp::err(info, "error message".to_string());

        match op.result {
            OpResult::Ok(_) => panic!("Expected Err variant"),
            OpResult::Err(e) => assert_eq!(e, "error message"),
        }
    }

    #[test]
    fn test_pending_op_from_result() {
        let info = PendingOpInfo::new(1, 2);

        // Test Ok case
        let op = PendingOp::from_result(info, Ok(serde_json::json!(42)));
        match op.result {
            OpResult::Ok(v) => assert_eq!(v, 42),
            OpResult::Err(_) => panic!("Expected Ok variant"),
        }

        // Test Err case
        let op = PendingOp::from_result(info, Err("failed".to_string()));
        match op.result {
            OpResult::Ok(_) => panic!("Expected Err variant"),
            OpResult::Err(e) => assert_eq!(e, "failed"),
        }
    }

    #[tokio::test]
    async fn test_op_future_from_infallible() {
        let future = async { 42 };
        let mut op_future = OpFuture::from_infallible(future);

        let result = (&mut op_future).await;
        match result {
            OpResult::Ok(v) => assert_eq!(v, 42),
            OpResult::Err(_) => panic!("Expected Ok variant"),
        }
    }

    #[tokio::test]
    async fn test_op_future_from_fallible_ok() {
        let future = async { Ok::<_, String>(42) };
        let mut op_future = OpFuture::from_fallible(future);

        let result = (&mut op_future).await;
        match result {
            OpResult::Ok(v) => assert_eq!(v, 42),
            OpResult::Err(_) => panic!("Expected Ok variant"),
        }
    }

    #[tokio::test]
    async fn test_op_future_from_fallible_err() {
        let future = async { Err::<i32, _>("operation failed") };
        let mut op_future = OpFuture::from_fallible(future);

        let result = (&mut op_future).await;
        match result {
            OpResult::Ok(_) => panic!("Expected Err variant"),
            OpResult::Err(e) => assert_eq!(e, "operation failed"),
        }
    }
}
