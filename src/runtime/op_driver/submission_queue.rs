//! Queue for managing submitted operations.

use super::pending_op::{OpFuture, PendingOpInfo};
use std::collections::VecDeque;

/// Entry in the submission queue.
pub struct QueuedOp {
    pub info: PendingOpInfo,
    pub future: OpFuture,
}

impl QueuedOp {
    pub fn new(info: PendingOpInfo, future: OpFuture) -> Self {
        Self { info, future }
    }
}

/// Queue for operations submitted to the driver.
///
/// Operations are added to this queue and then transferred to the
/// polling mechanism (FuturesUnordered in the driver implementation).
pub struct SubmissionQueue {
    queue: VecDeque<QueuedOp>,
}

impl SubmissionQueue {
    /// Create a new empty submission queue
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    /// Add an operation to the queue
    pub fn push(&mut self, op: QueuedOp) {
        self.queue.push_back(op);
    }

    /// Remove and return the next operation from the queue
    pub fn pop(&mut self) -> Option<QueuedOp> {
        self.queue.pop_front()
    }

    /// Get the number of queued operations
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Check if the queue is empty
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Clear all queued operations
    pub fn clear(&mut self) {
        self.queue.clear();
    }

    /// Drain all operations from the queue
    pub fn drain(&mut self) -> impl Iterator<Item = QueuedOp> + '_ {
        self.queue.drain(..)
    }
}

impl Default for SubmissionQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::op_driver::{OpId, PromiseId};

    #[test]
    fn test_submission_queue_basic() {
        let queue = SubmissionQueue::new();
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_submission_queue_push_pop() {
        let mut queue = SubmissionQueue::new();

        let info1 = PendingOpInfo::new(1, 100);
        let future1 = OpFuture::from_infallible(async { 42 });
        queue.push(QueuedOp::new(info1, future1));

        assert_eq!(queue.len(), 1);
        assert!(!queue.is_empty());

        let op = queue.pop().unwrap();
        assert_eq!(op.info.promise_id, 1);
        assert_eq!(op.info.op_id, 100);

        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_submission_queue_fifo() {
        let mut queue = SubmissionQueue::new();

        // Push multiple operations
        for i in 0..5 {
            let info = PendingOpInfo::new(i as PromiseId, i as OpId);
            let future = OpFuture::from_infallible(async move { i });
            queue.push(QueuedOp::new(info, future));
        }

        assert_eq!(queue.len(), 5);

        // Pop in FIFO order
        for i in 0..5 {
            let op = queue.pop().unwrap();
            assert_eq!(op.info.promise_id, i as PromiseId);
            assert_eq!(op.info.op_id, i as OpId);
        }

        assert!(queue.is_empty());
    }

    #[test]
    fn test_submission_queue_clear() {
        let mut queue = SubmissionQueue::new();

        for i in 0..3 {
            let info = PendingOpInfo::new(i, i);
            let future = OpFuture::from_infallible(async move { i });
            queue.push(QueuedOp::new(info, future));
        }

        assert_eq!(queue.len(), 3);
        queue.clear();
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_submission_queue_drain() {
        let mut queue = SubmissionQueue::new();

        for i in 0..3 {
            let info = PendingOpInfo::new(i, i);
            let future = OpFuture::from_infallible(async move { i });
            queue.push(QueuedOp::new(info, future));
        }

        let drained: Vec<_> = queue.drain().collect();
        assert_eq!(drained.len(), 3);
        assert!(queue.is_empty());

        for (i, op) in drained.into_iter().enumerate() {
            assert_eq!(op.info.promise_id, i as PromiseId);
        }
    }
}
