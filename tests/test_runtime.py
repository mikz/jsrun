"""
Phase 1 integration tests for the Tokio-based Runtime class.

These tests verify that the new Runtime API can:
1. Spawn a runtime successfully
2. Evaluate JavaScript code synchronously
3. Maintain state across evaluations
4. Shut down cleanly
5. Support context manager protocol
"""

import pytest
from jsrun import Runtime


class TestRuntimeBasics:
    """Basic Runtime creation and lifecycle tests."""

    def test_runtime_spawn(self):
        """Test that Runtime.spawn() creates a new runtime."""
        runtime = Runtime.spawn()
        assert runtime is not None
        assert not runtime.is_closed()
        runtime.close()

    def test_runtime_eval_simple(self):
        """Test basic JavaScript evaluation."""
        runtime = Runtime.spawn()
        try:
            result = runtime.eval("2 + 2")
            assert result == "4"
        finally:
            runtime.close()

    def test_runtime_eval_string(self):
        """Test string evaluation."""
        runtime = Runtime.spawn()
        try:
            result = runtime.eval("'hello world'")
            assert result == "hello world"
        finally:
            runtime.close()

    def test_runtime_eval_expression(self):
        """Test more complex expressions."""
        runtime = Runtime.spawn()
        try:
            result = runtime.eval("Math.max(10, 20, 30)")
            assert result == "30"
        finally:
            runtime.close()

    def test_runtime_close(self):
        """Test runtime shutdown."""
        runtime = Runtime.spawn()
        assert not runtime.is_closed()

        runtime.close()
        assert runtime.is_closed()

    def test_runtime_close_idempotent(self):
        """Test that close() can be called multiple times safely."""
        runtime = Runtime.spawn()
        runtime.close()
        runtime.close()  # Should not raise
        assert runtime.is_closed()

    def test_runtime_eval_after_close(self):
        """Test that eval after close raises an error."""
        runtime = Runtime.spawn()
        runtime.close()

        with pytest.raises(RuntimeError) as exc_info:
            runtime.eval("1 + 1")
        assert "closed" in str(exc_info.value).lower()


class TestRuntimeStatePersistence:
    """Tests for state persistence across evaluations."""

    def test_variable_persistence(self):
        """Test that variables persist across eval calls."""
        runtime = Runtime.spawn()
        try:
            runtime.eval("var x = 10;")
            result = runtime.eval("x")
            assert result == "10"

            runtime.eval("x = 20;")
            result = runtime.eval("x")
            assert result == "20"
        finally:
            runtime.close()

    def test_function_persistence(self):
        """Test that functions persist across eval calls."""
        runtime = Runtime.spawn()
        try:
            runtime.eval("function add(a, b) { return a + b; }")
            result = runtime.eval("add(5, 7)")
            assert result == "12"
        finally:
            runtime.close()

    def test_object_persistence(self):
        """Test that objects persist across eval calls."""
        runtime = Runtime.spawn()
        try:
            runtime.eval("var obj = {name: 'test', value: 42};")
            result = runtime.eval("obj.name")
            assert result == "test"

            result = runtime.eval("obj.value")
            assert result == "42"

            runtime.eval("obj.value = 100;")
            result = runtime.eval("obj.value")
            assert result == "100"
        finally:
            runtime.close()

    def test_multiple_sequential_evals(self):
        """Test multiple sequential evaluations."""
        runtime = Runtime.spawn()
        try:
            runtime.eval("var counter = 0;")

            for i in range(1, 6):
                runtime.eval("counter++;")
                result = runtime.eval("counter")
                assert result == str(i)
        finally:
            runtime.close()


class TestRuntimeContextManager:
    """Tests for context manager protocol support."""

    def test_context_manager_basic(self):
        """Test basic context manager usage."""
        with Runtime.spawn() as runtime:
            result = runtime.eval("3 + 3")
            assert result == "6"
        # Runtime should be closed after exiting context

    def test_context_manager_auto_close(self):
        """Test that context manager closes runtime automatically."""
        runtime = Runtime.spawn()
        assert not runtime.is_closed()

        with runtime:
            result = runtime.eval("5 * 5")
            assert result == "25"
            assert not runtime.is_closed()

        # Should be closed after exiting
        assert runtime.is_closed()

    def test_context_manager_with_exception(self):
        """Test that runtime is closed even if exception occurs."""
        runtime = Runtime.spawn()

        try:
            with runtime:
                runtime.eval("var x = 1;")
                raise ValueError("Test exception")
        except ValueError:
            pass

        # Runtime should still be closed
        assert runtime.is_closed()


class TestRuntimeConcurrent:
    """Tests for multiple concurrent runtimes."""

    def test_multiple_runtimes_independent(self):
        """Test that multiple runtimes have independent state."""
        runtime1 = Runtime.spawn()
        runtime2 = Runtime.spawn()

        try:
            runtime1.eval("var x = 'runtime1';")
            runtime2.eval("var x = 'runtime2';")

            result1 = runtime1.eval("x")
            result2 = runtime2.eval("x")

            assert result1 == "runtime1"
            assert result2 == "runtime2"
        finally:
            runtime1.close()
            runtime2.close()

    def test_sequential_runtime_creation(self):
        """Test creating multiple runtimes sequentially."""
        for i in range(3):
            runtime = Runtime.spawn()
            try:
                result = runtime.eval(f"{i} * 2")
                assert result == str(i * 2)
            finally:
                runtime.close()


class TestRuntimeEdgeCases:
    """Edge case tests for runtime behavior."""

    def test_eval_undefined(self):
        """Test evaluation of undefined."""
        runtime = Runtime.spawn()
        try:
            result = runtime.eval("undefined")
            assert result == "undefined"
        finally:
            runtime.close()

    def test_eval_null(self):
        """Test evaluation of null."""
        runtime = Runtime.spawn()
        try:
            result = runtime.eval("null")
            assert result == "null"
        finally:
            runtime.close()

    def test_eval_boolean(self):
        """Test evaluation of booleans."""
        runtime = Runtime.spawn()
        try:
            assert runtime.eval("true") == "true"
            assert runtime.eval("false") == "false"
        finally:
            runtime.close()

    def test_eval_array(self):
        """Test evaluation of arrays."""
        runtime = Runtime.spawn()
        try:
            result = runtime.eval("[1, 2, 3]")
            # Array toString representation
            assert result == "1,2,3"
        finally:
            runtime.close()

    def test_eval_object(self):
        """Test evaluation of objects."""
        runtime = Runtime.spawn()
        try:
            result = runtime.eval("({a: 1, b: 2})")
            # Object toString representation
            assert "[object Object]" in result
        finally:
            runtime.close()

    def test_eval_empty_string(self):
        """Test evaluation of empty statement."""
        runtime = Runtime.spawn()
        try:
            result = runtime.eval("")
            # Empty script returns undefined
            assert result == "undefined"
        finally:
            runtime.close()


# ============================================================================
# Phase 2: Async Execution Core Tests
# ============================================================================


class TestRuntimeAsync:
    """Test async evaluation with promises (Phase 2)."""

    @pytest.mark.asyncio
    async def test_eval_async_without_promise(self):
        """Test async eval with non-promise value."""
        with Runtime.spawn() as runtime:
            result = await runtime.eval_async("10 + 20")
            assert result == "30"

    @pytest.mark.asyncio
    async def test_eval_async_with_promise_resolved(self):
        """Test async eval with resolved promise."""
        with Runtime.spawn() as runtime:
            result = await runtime.eval_async("Promise.resolve(42)")
            assert result == "42"

    @pytest.mark.asyncio
    async def test_eval_async_with_promise_string(self):
        """Test async eval with promise resolving to string."""
        with Runtime.spawn() as runtime:
            result = await runtime.eval_async("Promise.resolve('async result')")
            assert result == "async result"

    @pytest.mark.asyncio
    async def test_eval_async_with_deferred_resolution(self):
        """Test async eval with deferred promise resolution."""
        with Runtime.spawn() as runtime:
            code = """
                new Promise((resolve) => {
                    // Immediately queued microtask via then
                    Promise.resolve().then(() => resolve('deferred result'));
                })
            """
            result = await runtime.eval_async(code)
            assert result == "deferred result"

    @pytest.mark.asyncio
    async def test_eval_async_promise_chain(self):
        """Test async eval with promise chain."""
        with Runtime.spawn() as runtime:
            code = """
                Promise.resolve(10)
                    .then(x => x * 2)
                    .then(x => x + 5)
            """
            result = await runtime.eval_async(code)
            assert result == "25"

    @pytest.mark.asyncio
    async def test_eval_async_promise_rejection(self):
        """Test async eval with rejected promise."""
        with Runtime.spawn() as runtime:
            with pytest.raises(RuntimeError, match="Promise rejected"):
                await runtime.eval_async("Promise.reject(new Error('test error'))")

    @pytest.mark.asyncio
    async def test_eval_async_promise_rejection_value(self):
        """Test async eval with rejected promise (non-Error value)."""
        with Runtime.spawn() as runtime:
            with pytest.raises(RuntimeError, match="Promise rejected"):
                await runtime.eval_async("Promise.reject('custom error')")


class TestRuntimeTimeout:
    """Test timeout functionality (Phase 2)."""

    @pytest.mark.asyncio
    async def test_eval_async_timeout_success(self):
        """Test that fast promises complete before timeout."""
        with Runtime.spawn() as runtime:
            result = await runtime.eval_async(
                "Promise.resolve('quick')", timeout_ms=1000
            )
            assert result == "quick"

    @pytest.mark.asyncio
    async def test_eval_async_timeout_expiry(self):
        """Test that timeout is enforced for slow operations."""
        with Runtime.spawn() as runtime:
            # Promise that never resolves
            code = "new Promise(() => {})"
            with pytest.raises(RuntimeError, match="timeout"):
                await runtime.eval_async(code, timeout_ms=100)

    @pytest.mark.asyncio
    async def test_eval_async_no_timeout(self):
        """Test async eval without timeout."""
        with Runtime.spawn() as runtime:
            # Should complete without timeout
            result = await runtime.eval_async("Promise.resolve(123)")
            assert result == "123"

    @pytest.mark.asyncio
    async def test_eval_async_timeout_with_promise_chain(self):
        """Test timeout with promise chains that complete in time."""
        with Runtime.spawn() as runtime:
            code = """
                Promise.resolve()
                    .then(() => Promise.resolve())
                    .then(() => 'nested')
            """
            result = await runtime.eval_async(code, timeout_ms=1000)
            assert result == "nested"


class TestRuntimeAsyncConcurrency:
    """Test concurrent async runtime operations (Phase 2)."""

    @pytest.mark.asyncio
    async def test_multiple_async_evals_sequential(self):
        """Test sequential async evaluations."""
        with Runtime.spawn() as runtime:
            result1 = await runtime.eval_async("Promise.resolve(1)")
            result2 = await runtime.eval_async("Promise.resolve(2)")
            result3 = await runtime.eval_async("Promise.resolve(3)")

            assert result1 == "1"
            assert result2 == "2"
            assert result3 == "3"

    @pytest.mark.asyncio
    async def test_multiple_runtimes_async(self):
        """Test multiple runtimes with async operations."""
        runtime1 = Runtime.spawn()
        runtime2 = Runtime.spawn()

        try:
            # Run async evals concurrently on different runtimes
            result1 = await runtime1.eval_async("Promise.resolve('runtime1')")
            result2 = await runtime2.eval_async("Promise.resolve('runtime2')")

            assert result1 == "runtime1"
            assert result2 == "runtime2"
        finally:
            runtime1.close()
            runtime2.close()

    @pytest.mark.asyncio
    async def test_mixed_sync_async_eval(self):
        """Test mixing sync and async eval calls."""
        with Runtime.spawn() as runtime:
            # Sync eval
            sync_result = runtime.eval("10 + 10")
            assert sync_result == "20"

            # Async eval
            async_result = await runtime.eval_async("Promise.resolve(30)")
            assert async_result == "30"

            # Another sync eval to verify state
            final_result = runtime.eval("5 * 5")
            assert final_result == "25"

    @pytest.mark.asyncio
    async def test_async_state_persistence(self):
        """Test that state persists across async evaluations."""
        with Runtime.spawn() as runtime:
            # Set up state
            runtime.eval("var asyncCounter = 0;")

            # Multiple async evals modifying state
            await runtime.eval_async("Promise.resolve(++asyncCounter)")
            result = await runtime.eval_async("Promise.resolve(asyncCounter)")
            assert result == "1"

            await runtime.eval_async("Promise.resolve(++asyncCounter)")
            result = runtime.eval("asyncCounter")
            assert result == "2"


class TestRuntimeAsyncErrors:
    """Test error handling in async evaluation (Phase 2)."""

    @pytest.mark.asyncio
    async def test_eval_async_syntax_error(self):
        """Test that syntax errors are properly reported in async eval."""
        with Runtime.spawn() as runtime:
            with pytest.raises(RuntimeError, match="failed"):
                await runtime.eval_async("this is not valid javascript {{{")

    @pytest.mark.asyncio
    async def test_eval_async_runtime_error(self):
        """Test that runtime errors in promises are caught."""
        with Runtime.spawn() as runtime:
            code = """
                new Promise((resolve, reject) => {
                    reject(new Error('Runtime error in promise'));
                })
            """
            with pytest.raises(RuntimeError, match="Promise rejected"):
                await runtime.eval_async(code)

    @pytest.mark.asyncio
    async def test_eval_async_after_close(self):
        """Test that async eval after close raises an error."""
        runtime = Runtime.spawn()
        runtime.close()

        with pytest.raises(RuntimeError, match="closed"):
            await runtime.eval_async("Promise.resolve(1)")
