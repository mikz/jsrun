//! FuturesUnordered-based OpDriver implementation.

use super::pending_op::{OpFuture, PendingOp, PendingOpInfo};
use super::submission_queue::{QueuedOp, SubmissionQueue};
use super::{OpDriver, OpId, OpInflightStats, OpResult, OpScheduling, PromiseId};
use futures::stream::{FuturesUnordered, StreamExt};
use futures::task::noop_waker_ref;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Tagged future with metadata for tracking in FuturesUnordered.
struct TaggedFuture {
    info: PendingOpInfo,
    future: OpFuture,
}

impl Future for TaggedFuture {
    type Output = PendingOp;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.future).poll(cx) {
            Poll::Ready(result) => Poll::Ready(PendingOp {
                info: self.info,
                result,
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// OpDriver implementation using FuturesUnordered for efficient concurrent polling.
///
/// This driver manages async operations submitted from JavaScript, providing:
/// - Eager completion: Poll once immediately, queue if not ready
/// - Lazy polling: Always queue without immediate poll
/// - Deferred resolution: Queue ready results for batch processing
pub struct FuturesUnorderedDriver {
    /// Queue for pending operations
    queue: RefCell<SubmissionQueue>,
    /// FuturesUnordered for concurrent polling
    futures: RefCell<FuturesUnordered<TaggedFuture>>,
    /// Track total submitted operations
    total_submitted: RefCell<usize>,
    /// Track total completed operations
    total_completed: RefCell<usize>,
    /// Shutdown flag
    shutdown: RefCell<bool>,
}

impl Default for FuturesUnorderedDriver {
    fn default() -> Self {
        Self {
            queue: RefCell::new(SubmissionQueue::new()),
            futures: RefCell::new(FuturesUnordered::new()),
            total_submitted: RefCell::new(0),
            total_completed: RefCell::new(0),
            shutdown: RefCell::new(false),
        }
    }
}

impl FuturesUnorderedDriver {
    /// Transfer queued operations to FuturesUnordered
    fn drain_queue(&self) {
        let mut queue = self.queue.borrow_mut();
        let futures = self.futures.borrow_mut();

        for queued in queue.drain() {
            futures.push(TaggedFuture {
                info: queued.info,
                future: queued.future,
            });
        }
    }
}

impl OpDriver for FuturesUnorderedDriver {
    fn submit_op_infallible<R>(
        &self,
        op_id: OpId,
        promise_id: PromiseId,
        scheduling: OpScheduling,
        op: impl Future<Output = R> + 'static,
    ) -> Option<R>
    where
        R: 'static + serde::Serialize,
    {
        if *self.shutdown.borrow() {
            return None;
        }

        *self.total_submitted.borrow_mut() += 1;

        let info = PendingOpInfo { promise_id, op_id };

        match scheduling {
            OpScheduling::Lazy => {
                // Always queue without polling
                let future = OpFuture::from_infallible(op);
                self.queue.borrow_mut().push(QueuedOp::new(info, future));
                None
            }
            OpScheduling::Eager => {
                // Poll once immediately
                let mut pinned = Box::pin(op);
                let waker = noop_waker_ref();
                let mut cx = Context::from_waker(waker);

                match pinned.as_mut().poll(&mut cx) {
                    Poll::Ready(result) => {
                        // Eager completion!
                        *self.total_completed.borrow_mut() += 1;
                        Some(result)
                    }
                    Poll::Pending => {
                        // Not ready, queue it
                        let future = OpFuture::from_infallible(pinned);
                        self.queue.borrow_mut().push(QueuedOp::new(info, future));
                        None
                    }
                }
            }
            OpScheduling::Deferred => {
                // Always queue, even if ready immediately
                let future = OpFuture::from_infallible(op);
                self.queue.borrow_mut().push(QueuedOp::new(info, future));
                None
            }
        }
    }

    fn submit_op_fallible<R, E>(
        &self,
        op_id: OpId,
        promise_id: PromiseId,
        scheduling: OpScheduling,
        op: impl Future<Output = Result<R, E>> + 'static,
    ) -> Option<Result<R, E>>
    where
        R: 'static + serde::Serialize,
        E: 'static + std::fmt::Display,
    {
        if *self.shutdown.borrow() {
            return None;
        }

        *self.total_submitted.borrow_mut() += 1;

        let info = PendingOpInfo { promise_id, op_id };

        match scheduling {
            OpScheduling::Lazy => {
                // Always queue without polling
                let future = OpFuture::from_fallible(op);
                self.queue.borrow_mut().push(QueuedOp::new(info, future));
                None
            }
            OpScheduling::Eager => {
                // Poll once immediately
                let mut pinned = Box::pin(op);
                let waker = noop_waker_ref();
                let mut cx = Context::from_waker(waker);

                match pinned.as_mut().poll(&mut cx) {
                    Poll::Ready(result) => {
                        // Eager completion!
                        *self.total_completed.borrow_mut() += 1;
                        Some(result)
                    }
                    Poll::Pending => {
                        // Not ready, queue it
                        let future = OpFuture::from_fallible(pinned);
                        self.queue.borrow_mut().push(QueuedOp::new(info, future));
                        None
                    }
                }
            }
            OpScheduling::Deferred => {
                // Always queue, even if ready immediately
                let future = OpFuture::from_fallible(op);
                self.queue.borrow_mut().push(QueuedOp::new(info, future));
                None
            }
        }
    }

    fn poll_ready(&self, cx: &mut Context) -> Poll<(PromiseId, OpId, OpResult)> {
        if *self.shutdown.borrow() {
            return Poll::Pending;
        }

        // Transfer queued ops to futures
        self.drain_queue();

        // Poll FuturesUnordered for ready operations
        let mut futures = self.futures.borrow_mut();
        match futures.poll_next_unpin(cx) {
            Poll::Ready(Some(pending_op)) => {
                *self.total_completed.borrow_mut() += 1;
                Poll::Ready((
                    pending_op.info.promise_id,
                    pending_op.info.op_id,
                    pending_op.result,
                ))
            }
            Poll::Ready(None) | Poll::Pending => Poll::Pending,
        }
    }

    fn len(&self) -> usize {
        self.queue.borrow().len() + self.futures.borrow().len()
    }

    fn shutdown(&self) {
        *self.shutdown.borrow_mut() = true;
        self.queue.borrow_mut().clear();
        self.futures.borrow_mut().clear();
    }

    fn stats(&self) -> OpInflightStats {
        OpInflightStats {
            pending_count: self.len(),
            total_submitted: *self.total_submitted.borrow(),
            total_completed: *self.total_completed.borrow(),
        }
    }

    fn is_shutdown(&self) -> bool {
        *self.shutdown.borrow()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::ready;

    #[test]
    fn test_driver_creation() {
        let driver = FuturesUnorderedDriver::default();
        assert_eq!(driver.len(), 0);
        assert!(!*driver.shutdown.borrow());
    }

    #[test]
    fn test_submit_eager_ready() {
        let driver = FuturesUnorderedDriver::default();
        let future = ready(42);

        let result = driver.submit_op_infallible(1, 100, OpScheduling::Eager, future);

        // Should complete eagerly
        assert_eq!(result, Some(42));
        assert_eq!(driver.len(), 0); // Nothing queued
        assert_eq!(*driver.total_submitted.borrow(), 1);
        assert_eq!(*driver.total_completed.borrow(), 1);
    }

    #[test]
    fn test_submit_lazy() {
        let driver = FuturesUnorderedDriver::default();
        let future = ready(42);

        let result = driver.submit_op_infallible(1, 100, OpScheduling::Lazy, future);

        // Should not complete eagerly
        assert_eq!(result, None);
        assert_eq!(driver.len(), 1); // Queued
        assert_eq!(*driver.total_submitted.borrow(), 1);
        assert_eq!(*driver.total_completed.borrow(), 0);
    }

    #[test]
    fn test_submit_deferred() {
        let driver = FuturesUnorderedDriver::default();
        let future = ready(42);

        let result = driver.submit_op_infallible(1, 100, OpScheduling::Deferred, future);

        // Should not complete eagerly even though ready
        assert_eq!(result, None);
        assert_eq!(driver.len(), 1);
    }

    #[tokio::test]
    async fn test_poll_ready() {
        let driver = FuturesUnorderedDriver::default();

        // Submit a lazy operation
        driver.submit_op_infallible(1, 100, OpScheduling::Lazy, ready(42));

        // Poll for ready operations
        let (promise_id, op_id, result) =
            futures::future::poll_fn(|cx| driver.poll_ready(cx)).await;

        assert_eq!(promise_id, 100);
        assert_eq!(op_id, 1);
        match result {
            OpResult::Ok(v) => assert_eq!(v, 42),
            OpResult::Err(_) => panic!("Expected Ok"),
        }

        assert_eq!(driver.len(), 0);
        assert_eq!(*driver.total_completed.borrow(), 1);
    }

    #[tokio::test]
    async fn test_multiple_operations() {
        let driver = FuturesUnorderedDriver::default();

        // Submit multiple operations
        driver.submit_op_infallible(1, 100, OpScheduling::Lazy, ready(1));
        driver.submit_op_infallible(2, 101, OpScheduling::Lazy, ready(2));
        driver.submit_op_infallible(3, 102, OpScheduling::Lazy, ready(3));

        assert_eq!(driver.len(), 3);

        // Poll all operations
        let mut results = vec![];
        for _ in 0..3 {
            let (promise_id, _, result) =
                futures::future::poll_fn(|cx| driver.poll_ready(cx)).await;
            results.push((promise_id, result));
        }

        assert_eq!(driver.len(), 0);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_fallible_eager_ok() {
        let driver = FuturesUnorderedDriver::default();
        let future = ready(Ok::<_, String>(42));

        let result = driver.submit_op_fallible(1, 100, OpScheduling::Eager, future);

        assert_eq!(result, Some(Ok(42)));
        assert_eq!(driver.len(), 0);
    }

    #[test]
    fn test_fallible_eager_err() {
        let driver = FuturesUnorderedDriver::default();
        let future = ready(Err::<i32, _>("error"));

        let result = driver.submit_op_fallible(1, 100, OpScheduling::Eager, future);

        assert!(result.is_some());
        assert!(result.unwrap().is_err());
        assert_eq!(driver.len(), 0);
    }

    #[test]
    fn test_shutdown() {
        let driver = FuturesUnorderedDriver::default();

        driver.submit_op_infallible(1, 100, OpScheduling::Lazy, ready(42));
        assert_eq!(driver.len(), 1);

        driver.shutdown();

        assert!(driver.is_empty());
        assert!(*driver.shutdown.borrow());

        // New submissions should be ignored
        let result = driver.submit_op_infallible(2, 101, OpScheduling::Eager, ready(99));
        assert_eq!(result, None);
    }

    #[test]
    fn test_stats() {
        let driver = FuturesUnorderedDriver::default();

        driver.submit_op_infallible(1, 100, OpScheduling::Eager, ready(1));
        driver.submit_op_infallible(2, 101, OpScheduling::Lazy, ready(2));
        driver.submit_op_infallible(3, 102, OpScheduling::Lazy, ready(3));

        let stats = driver.stats();
        assert_eq!(stats.total_submitted, 3);
        assert_eq!(stats.total_completed, 1); // Only eager completed
        assert_eq!(stats.pending_count, 2); // Two lazy queued
    }
}
