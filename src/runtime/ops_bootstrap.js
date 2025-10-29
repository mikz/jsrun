// Promise Registry and Op System Bootstrap
// Based on Deno Core's 00_infra.js promise management system
//
// This code is injected into every runtime at startup and provides the
// infrastructure for async operations (ops) between JavaScript and Rust/Python.

(function() {
  'use strict';

  // Constants
  const RING_SIZE = 4 * 1024;  // 4KB ring buffer for recent promises
  const NO_PROMISE = null;

  // Promise storage: hybrid approach for performance
  // - Ring buffer for recent promises (O(1) access)
  // - Map for overflow when ring wraps
  const promiseRing = new Array(RING_SIZE);
  for (let i = 0; i < RING_SIZE; i++) {
    promiseRing[i] = NO_PROMISE;
  }
  const promiseMap = new Map();

  // Promise ID counter (wraps at 2^32)
  let nextPromiseId = 0;

  /**
   * Create a new promise and store it with the given ID.
   * Returns the promise that will be resolved when the op completes.
   */
  function setPromise(promiseId) {
    const idx = promiseId % RING_SIZE;

    // If there's already a promise at this index, move it to the map
    const oldRecord = promiseRing[idx];
    if (oldRecord !== NO_PROMISE) {
      promiseMap.set(oldRecord.id, oldRecord.handlers);
    }

    // Create new promise with resolve/reject functions
    let resolve, reject;
    const promise = new Promise((res, rej) => {
      resolve = res;
      reject = rej;
    });

    // Store promise metadata in ring
    promiseRing[idx] = {
      id: promiseId >>> 0,
      handlers: [resolve, reject],
    };

    return promise;
  }

  /**
   * Resolve or reject a promise by its ID.
   * Called by Rust when an async op completes.
   */
  function resolveOp(promiseId, isOk, result) {
    const outOfBounds = promiseId < nextPromiseId - RING_SIZE;

    let promiseHandlers;
    const idx = promiseId % RING_SIZE;
    const record = promiseRing[idx];

    if (!outOfBounds && record !== NO_PROMISE && record.id === (promiseId >>> 0)) {
      promiseHandlers = record.handlers;
      promiseRing[idx] = NO_PROMISE;
    } else {
      // Promise is in the map (overflow storage)
      promiseHandlers = promiseMap.get(promiseId >>> 0);
      if (!promiseHandlers) {
        throw new Error(`Promise ${promiseId} not found`);
      }
      promiseMap.delete(promiseId >>> 0);
    }

    // Resolve or reject based on isOk flag
    if (isOk) {
      promiseHandlers[0](result);  // resolve
    } else {
      promiseHandlers[1](result);  // reject
    }
  }

  /**
   * Synchronous op call - returns result directly.
   * Throws if the op encounters an error.
   */
  function __host_op_sync__(opId, ...args) {
    // This function will be implemented in Rust
    // It's injected as a V8 function template
    throw new Error('__host_op_sync__ not initialized');
  }

  /**
   * Asynchronous op call - returns a promise.
   * The promise will be resolved when the op completes.
   */
  function __host_op_async__(opId, ...args) {
    // Allocate new promise ID
    const promiseId = nextPromiseId;
    nextPromiseId = (nextPromiseId + 1) & 0xffffffff;  // Wrap at 2^32

    // Create and store the promise
    const promise = setPromise(promiseId);

    // Call the Rust op handler (will be injected)
    // This function will be implemented in Rust
    // It's injected as a V8 function template
    try {
      const impl = globalThis.__host_op_async_impl__;
      if (typeof impl !== 'function') {
        throw new Error('__host_op_async_impl__ not initialized');
      }
      impl(opId, promiseId, ...args);
    } catch (err) {
      // If the call fails synchronously, reject immediately
      const idx = promiseId % RING_SIZE;
      const record = promiseRing[idx];
      if (record !== NO_PROMISE && record.id === (promiseId >>> 0)) {
        promiseRing[idx] = NO_PROMISE;
        record.handlers[1](err);
      } else {
        const handlers = promiseMap.get(promiseId >>> 0);
        if (handlers) {
          promiseMap.delete(promiseId >>> 0);
          handlers[1](err);
        } else {
          throw err;
        }
      }
    }

    return promise;
  }

  /**
   * Implementation stub for async ops.
   * This will be replaced by Rust with a proper V8 function.
   */
  function __host_op_async_impl__(opId, promiseId, ...args) {
    throw new Error('__host_op_async_impl__ not initialized');
  }

  // Expose the op system globally
  globalThis.__host_op_sync__ = __host_op_sync__;
  globalThis.__host_op_async__ = __host_op_async__;
  globalThis.__host_op_async_impl__ = __host_op_async_impl__;
  globalThis.__resolveOp = resolveOp;

  // Expose promise registry stats for debugging
  globalThis.__getPromiseStats = function() {
    return {
      nextPromiseId: nextPromiseId,
      ringSize: RING_SIZE,
      mapSize: promiseMap.size,
    };
  };
})();
