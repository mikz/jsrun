"""
Tests for RuntimeContext - lightweight context handles within a Runtime.

RuntimeContext provides namespace isolation within a single Runtime,
allowing multiple independent execution environments on the same isolate.
"""

import pytest
from jsrun import Runtime


class TestRuntimeContextCreation:
    """Test creating and managing contexts."""

    def test_create_single_context(self):
        """Test creating a single context."""
        runtime = Runtime.spawn()
        try:
            ctx = runtime.create_context()
            assert ctx is not None
            assert not ctx.is_closed()
            ctx.close()
        finally:
            runtime.close()

    def test_create_multiple_contexts(self):
        """Test creating multiple contexts in the same runtime."""
        runtime = Runtime.spawn()
        try:
            ctx1 = runtime.create_context()
            ctx2 = runtime.create_context()
            ctx3 = runtime.create_context()

            assert ctx1 is not None
            assert ctx2 is not None
            assert ctx3 is not None

            ctx1.close()
            ctx2.close()
            ctx3.close()
        finally:
            runtime.close()

    def test_context_close_idempotent(self):
        """Test that closing a context multiple times is safe."""
        runtime = Runtime.spawn()
        try:
            ctx = runtime.create_context()
            ctx.close()
            ctx.close()  # Should not raise
            assert ctx.is_closed()
        finally:
            runtime.close()

    def test_create_context_after_runtime_closed(self):
        """Test that creating context after runtime close raises error."""
        runtime = Runtime.spawn()
        runtime.close()

        with pytest.raises(Exception) as exc_info:
            runtime.create_context()
        assert "closed" in str(exc_info.value).lower()


class TestRuntimeContextEval:
    """Test evaluating code in contexts."""

    def test_eval_simple(self):
        """Test basic evaluation in context."""
        runtime = Runtime.spawn()
        try:
            ctx = runtime.create_context()
            result = ctx.eval("2 + 2")
            assert result == "4"
            ctx.close()
        finally:
            runtime.close()

    def test_eval_string(self):
        """Test string evaluation in context."""
        runtime = Runtime.spawn()
        try:
            ctx = runtime.create_context()
            result = ctx.eval("'hello world'")
            assert result == "hello world"
            ctx.close()
        finally:
            runtime.close()

    def test_eval_after_close(self):
        """Test that eval after close raises error."""
        runtime = Runtime.spawn()
        try:
            ctx = runtime.create_context()
            ctx.close()

            with pytest.raises(Exception) as exc_info:
                ctx.eval("1 + 1")
            assert "closed" in str(exc_info.value).lower()
        finally:
            runtime.close()


class TestRuntimeContextIsolation:
    """Test that contexts have independent state."""

    def test_variable_isolation(self):
        """Test that variables are isolated between contexts."""
        runtime = Runtime.spawn()
        try:
            ctx1 = runtime.create_context()
            ctx2 = runtime.create_context()

            # Set different values in each context
            ctx1.eval("var x = 'context1'")
            ctx2.eval("var x = 'context2'")

            # Each context has its own value
            assert ctx1.eval("x") == "context1"
            assert ctx2.eval("x") == "context2"

            ctx1.close()
            ctx2.close()
        finally:
            runtime.close()

    def test_function_isolation(self):
        """Test that functions are isolated between contexts."""
        runtime = Runtime.spawn()
        try:
            ctx1 = runtime.create_context()
            ctx2 = runtime.create_context()

            ctx1.eval("function foo() { return 'ctx1'; }")
            ctx2.eval("function foo() { return 'ctx2'; }")

            assert ctx1.eval("foo()") == "ctx1"
            assert ctx2.eval("foo()") == "ctx2"

            ctx1.close()
            ctx2.close()
        finally:
            runtime.close()

    def test_state_persistence_within_context(self):
        """Test that state persists across evals in same context."""
        runtime = Runtime.spawn()
        try:
            ctx = runtime.create_context()

            ctx.eval("var counter = 0")
            ctx.eval("counter++")
            ctx.eval("counter++")
            result = ctx.eval("counter")

            assert result == "2"
            ctx.close()
        finally:
            runtime.close()


class TestRuntimeContextManager:
    """Test context manager protocol."""

    def test_context_manager_basic(self):
        """Test using context as context manager."""
        runtime = Runtime.spawn()
        try:
            with runtime.create_context() as ctx:
                result = ctx.eval("3 + 3")
                assert result == "6"
            # Context should be closed after exiting
        finally:
            runtime.close()

    def test_context_manager_auto_close(self):
        """Test that context manager closes context automatically."""
        runtime = Runtime.spawn()
        try:
            ctx = runtime.create_context()
            with ctx:
                result = ctx.eval("5 * 5")
                assert result == "25"
                assert not ctx.is_closed()

            # Should be closed after exiting
            assert ctx.is_closed()
        finally:
            runtime.close()

    def test_context_manager_with_exception(self):
        """Test that context is closed even if exception occurs."""
        runtime = Runtime.spawn()
        try:
            ctx = runtime.create_context()

            try:
                with ctx:
                    ctx.eval("var x = 1")
                    raise ValueError("Test exception")
            except ValueError:
                pass

            # Context should still be closed
            assert ctx.is_closed()
        finally:
            runtime.close()


class TestRuntimeContextAsync:
    """Test async operations in contexts."""

    @pytest.mark.asyncio
    async def test_eval_async_basic(self):
        """Test basic async evaluation in context."""
        with Runtime.spawn() as runtime:
            ctx = runtime.create_context()
            try:
                result = await ctx.eval_async("Promise.resolve(42)")
                assert result == "42"
            finally:
                ctx.close()

    @pytest.mark.asyncio
    async def test_eval_async_multiple_contexts(self):
        """Test async evaluation in multiple contexts."""
        with Runtime.spawn() as runtime:
            ctx1 = runtime.create_context()
            ctx2 = runtime.create_context()

            try:
                result1 = await ctx1.eval_async("Promise.resolve('ctx1')")
                result2 = await ctx2.eval_async("Promise.resolve('ctx2')")

                assert result1 == "ctx1"
                assert result2 == "ctx2"
            finally:
                ctx1.close()
                ctx2.close()

    @pytest.mark.asyncio
    async def test_eval_async_after_close(self):
        """Test that async eval after close raises error."""
        with Runtime.spawn() as runtime:
            ctx = runtime.create_context()
            ctx.close()

            with pytest.raises(Exception) as exc_info:
                await ctx.eval_async("Promise.resolve(1)")
            assert "closed" in str(exc_info.value).lower()


class TestRuntimeAndContextCombined:
    """Test interactions between Runtime.eval() and Context.eval()."""

    def test_runtime_and_context_separate(self):
        """Test that Runtime.eval() and Context.eval() use different namespaces."""
        runtime = Runtime.spawn()
        try:
            # Runtime eval uses primary context
            runtime.eval("var runtimeVar = 'runtime'")

            # Context eval uses separate context
            ctx = runtime.create_context()
            ctx.eval("var contextVar = 'context'")

            # Runtime can't see context variable
            result = runtime.eval("typeof contextVar")
            assert result == "undefined"

            # Context can't see runtime variable
            result = ctx.eval("typeof runtimeVar")
            assert result == "undefined"

            ctx.close()
        finally:
            runtime.close()


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
