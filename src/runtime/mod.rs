//! Tokio-based JavaScript runtime for multi-tenant execution.
//!
//! This module implements a Rust-first, async runtime patterned after `deno_core`.
//! Each runtime owns a single V8 isolate running on a dedicated OS thread with a
//! Tokio event loop.

pub mod config;
pub mod context;
pub mod handle;
pub mod ops;
pub mod python;
pub mod runner;

use once_cell::sync::OnceCell;

/// Global V8 platform instance.
///
/// V8 requires exactly one platform to be initialized before creating isolates.
/// This is a singleton that is initialized once on first access.
static V8_PLATFORM: OnceCell<rusty_v8::SharedRef<rusty_v8::Platform>> = OnceCell::new();

/// Initialize the V8 platform exactly once.
///
/// This function is safe to call multiple times; subsequent calls are no-ops.
/// It must be called before creating any runtimes.
pub fn initialize_platform_once() {
    V8_PLATFORM.get_or_init(|| {
        // Set V8 flags before initialization
        // Enable GC exposure for testing and debugging
        rusty_v8::V8::set_flags_from_string("--expose-gc");

        // Create and initialize the platform
        let platform = rusty_v8::new_default_platform(0, false).make_shared();
        rusty_v8::V8::initialize_platform(platform.clone());
        rusty_v8::V8::initialize();

        platform
    });
}

/// Check if the V8 platform has been initialized.
pub fn is_platform_initialized() -> bool {
    V8_PLATFORM.get().is_some()
}

// Re-export key types for convenience
pub use config::RuntimeConfig;
pub use handle::RuntimeHandle;

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_platform_initialization() {
        initialize_platform_once();
        assert!(is_platform_initialized());

        // Should be safe to call again
        initialize_platform_once();
        assert!(is_platform_initialized());
    }

    #[test]
    fn test_runtime_lifecycle() {
        initialize_platform_once();

        let config = RuntimeConfig::default();
        let mut handle = RuntimeHandle::spawn(config).unwrap();

        assert!(!handle.is_shutdown());

        // Evaluate some code
        let result = handle.eval_sync("40 + 2");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "42");

        // Shutdown
        handle.close().unwrap();
        assert!(handle.is_shutdown());
    }

    #[test]
    fn test_multiple_runtimes_sequential() {
        initialize_platform_once();

        for i in 0..3 {
            let config = RuntimeConfig::default();
            let mut handle = RuntimeHandle::spawn(config).unwrap();

            let code = format!("{} * 2", i);
            let result = handle.eval_sync(&code);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), format!("{}", i * 2));

            handle.close().unwrap();
        }
    }

    #[test]
    fn test_concurrent_runtimes() {
        initialize_platform_once();

        let mut handles = vec![];

        // Spawn multiple runtimes
        for _ in 0..3 {
            let config = RuntimeConfig::default();
            let handle = RuntimeHandle::spawn(config).unwrap();
            handles.push(handle);
        }

        // Use them concurrently
        let mut threads = vec![];
        for (i, handle) in handles.into_iter().enumerate() {
            let t = thread::spawn(move || {
                let code = format!("{} + 100", i);
                let result = handle.eval_sync(&code);
                assert!(result.is_ok());
                assert_eq!(result.unwrap(), format!("{}", i + 100));
            });
            threads.push(t);
        }

        // Wait for all threads
        for t in threads {
            t.join().unwrap();
        }
    }

    #[test]
    fn test_runtime_with_heap_limits() {
        initialize_platform_once();

        let config = RuntimeConfig::new()
            .with_max_heap_size(10 * 1024 * 1024) // 10 MB
            .with_initial_heap_size(1024 * 1024); // 1 MB

        let handle = RuntimeHandle::spawn(config).unwrap();

        let result = handle.eval_sync("'hello'");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn test_runtime_with_bootstrap() {
        initialize_platform_once();

        let config =
            RuntimeConfig::new().with_bootstrap("globalThis.VERSION = '1.0.0';".to_string());

        let handle = RuntimeHandle::spawn(config).unwrap();

        let result = handle.eval_sync("globalThis.VERSION");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "1.0.0");
    }

    #[test]
    fn test_runtime_state_persistence() {
        initialize_platform_once();

        let config = RuntimeConfig::default();
        let handle = RuntimeHandle::spawn(config).unwrap();

        // Set a variable
        let result1 = handle.eval_sync("var counter = 0; counter");
        assert_eq!(result1.unwrap(), "0");

        // Increment it
        let result2 = handle.eval_sync("++counter");
        assert_eq!(result2.unwrap(), "1");

        // Verify persistence
        let result3 = handle.eval_sync("counter");
        assert_eq!(result3.unwrap(), "1");
    }
}
