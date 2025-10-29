"""
Tests for Op System

This module tests the operations (ops) system that allows Python code to expose
functions to JavaScript with permission checking and sync/async support.
"""

import json
import pytest
from jsrun import Runtime


class TestOpRegistration:
    """Test op registration functionality."""

    def test_register_sync_op(self):
        """Test registering a synchronous op."""
        runtime = Runtime.spawn()
        try:

            def add_numbers(args):
                return args[0] + args[1]

            op_id = runtime.register_op("add", add_numbers, mode="sync")
            assert isinstance(op_id, int)
            assert op_id >= 0
        finally:
            runtime.close()

    def test_register_async_op(self):
        """Test registering an asynchronous op."""
        runtime = Runtime.spawn()
        try:

            def fetch_data(args):
                return {"data": "fetched"}

            op_id = runtime.register_op("fetch", fetch_data, mode="async")
            assert isinstance(op_id, int)
            assert op_id >= 0
        finally:
            runtime.close()

    def test_register_op_with_permissions(self):
        """Test registering an op with permissions."""
        runtime = Runtime.spawn()
        try:

            def read_file(args):
                return "file content"

            op_id = runtime.register_op(
                "readFile", read_file, mode="sync", permissions=["file:/tmp"]
            )
            assert isinstance(op_id, int)
        finally:
            runtime.close()

    def test_register_multiple_ops(self):
        """Test registering multiple ops."""
        runtime = Runtime.spawn()
        try:

            def op1(args):
                return "op1"

            def op2(args):
                return "op2"

            def op3(args):
                return "op3"

            op_id1 = runtime.register_op("op1", op1)
            op_id2 = runtime.register_op("op2", op2)
            op_id3 = runtime.register_op("op3", op3)

            # Each op should get a unique ID
            assert op_id1 != op_id2
            assert op_id2 != op_id3
            assert op_id1 != op_id3
        finally:
            runtime.close()

    def test_register_op_invalid_mode(self):
        """Test that invalid mode raises error."""
        runtime = Runtime.spawn()
        try:

            def my_op(args):
                return "result"

            with pytest.raises(Exception) as exc_info:
                runtime.register_op("test", my_op, mode="invalid")
            assert "Invalid mode" in str(exc_info.value)
        finally:
            runtime.close()

    def test_register_op_after_close(self):
        """Test that registering op after close raises error."""
        runtime = Runtime.spawn()
        runtime.close()

        def my_op(args):
            return "result"

        with pytest.raises(Exception) as exc_info:
            runtime.register_op("test", my_op)
        assert "closed" in str(exc_info.value).lower()


class TestOpHandlers:
    """Test op handler functionality."""

    def test_handler_receives_arguments(self):
        """Test that handler receives arguments correctly."""
        runtime = Runtime.spawn()
        try:
            received_args = []

            def capture_args(args):
                received_args.extend(args)
                return "ok"

            op_id = runtime.register_op("capture", capture_args)

            # Note: Actually calling the op from JavaScript requires the full
            # op dispatch system which is not yet complete.
            # This test just verifies registration succeeds.
            assert op_id >= 0
        finally:
            runtime.close()

    def test_handler_returns_value(self):
        """Test that handler return values are processed."""
        runtime = Runtime.spawn()
        try:

            def return_value(args):
                return {"result": 42, "status": "success"}

            op_id = runtime.register_op("getValue", return_value)
            assert op_id >= 0
        finally:
            runtime.close()

    def test_handler_with_lambda(self):
        """Test registering a lambda as handler."""
        runtime = Runtime.spawn()
        try:
            op_id = runtime.register_op("double", lambda args: args[0] * 2)
            assert op_id >= 0
        finally:
            runtime.close()

    def test_handler_with_closure(self):
        """Test registering a closure as handler."""
        runtime = Runtime.spawn()
        try:
            counter = [0]

            def increment(args):
                counter[0] += 1
                return counter[0]

            op_id = runtime.register_op("increment", increment)
            assert op_id >= 0
        finally:
            runtime.close()

    def test_sync_op_invocation(self):
        """Ensure __host_op_sync__ invokes Python handler."""
        runtime = Runtime.spawn()
        try:

            def concat(args):
                return {"joined": "".join(str(x) for x in args)}

            op_id = runtime.register_op("concat", concat, mode="sync")

            result = runtime.eval(
                f"JSON.stringify(__host_op_sync__({op_id}, 'a', 'b', 'c'))"
            )
            payload = json.loads(result)

            assert payload["joined"] == "abc"
        finally:
            runtime.close()

    def test_sync_call_async_op_errors(self):
        """Calling async-mode ops via sync entry point should raise."""
        runtime = Runtime.spawn()
        try:

            def async_like(args):
                return "value"

            op_id = runtime.register_op("asyncOnly", async_like, mode="async")

            result = runtime.eval(
                f"""
                try {{
                    __host_op_sync__({op_id}, 'x');
                    'ok';
                }} catch (err) {{
                    err && err.message ? err.message : String(err);
                }}
                """
            )

            assert "async" in result
        finally:
            runtime.close()

    @pytest.mark.asyncio
    async def test_async_op_invocation(self):
        """Ensure __host_op_async__ resolves with handler result."""
        with Runtime.spawn() as runtime:

            def collect(args):
                return {"count": len(args), "data": args}

            op_id = runtime.register_op("collect", collect, mode="async")

            result = await runtime.eval_async(
                f"""
                (async () => {{
                    const value = await __host_op_async__({op_id}, 1, 2, 3);
                    return JSON.stringify(value);
                }})()
                """
            )

            payload = json.loads(result)
            assert payload["count"] == 3
            assert payload["data"] == [1, 2, 3]


class TestOpPermissions:
    """Test op permission system."""

    def test_permission_timers(self):
        """Test registering op with timers permission."""
        runtime = Runtime.spawn()
        try:

            def set_timeout(args):
                return "timeout_id"

            op_id = runtime.register_op(
                "setTimeout", set_timeout, permissions=["timers"]
            )
            assert op_id >= 0
        finally:
            runtime.close()

    def test_permission_net_all(self):
        """Test registering op with net permission (all hosts)."""
        runtime = Runtime.spawn()
        try:

            def fetch(args):
                return "response"

            op_id = runtime.register_op("fetch", fetch, permissions=["net:"])
            assert op_id >= 0
        finally:
            runtime.close()

    def test_permission_net_specific_host(self):
        """Test registering op with net permission for specific host."""
        runtime = Runtime.spawn()
        try:

            def fetch_example(args):
                return "response"

            op_id = runtime.register_op(
                "fetchExample", fetch_example, permissions=["net:example.com"]
            )
            assert op_id >= 0
        finally:
            runtime.close()

    def test_permission_file_all(self):
        """Test registering op with file permission (all paths)."""
        runtime = Runtime.spawn()
        try:

            def read_file(args):
                return "content"

            op_id = runtime.register_op("readFile", read_file, permissions=["file:"])
            assert op_id >= 0
        finally:
            runtime.close()

    def test_permission_file_specific_path(self):
        """Test registering op with file permission for specific path."""
        runtime = Runtime.spawn()
        try:

            def read_tmp(args):
                return "content"

            op_id = runtime.register_op("readTmp", read_tmp, permissions=["file:/tmp"])
            assert op_id >= 0
        finally:
            runtime.close()

    def test_permission_env(self):
        """Test registering op with env permission."""
        runtime = Runtime.spawn()
        try:

            def get_env(args):
                return "value"

            op_id = runtime.register_op("getEnv", get_env, permissions=["env"])
            assert op_id >= 0
        finally:
            runtime.close()

    def test_permission_process(self):
        """Test registering op with process permission."""
        runtime = Runtime.spawn()
        try:

            def spawn_process(args):
                return "pid"

            op_id = runtime.register_op("spawn", spawn_process, permissions=["process"])
            assert op_id >= 0
        finally:
            runtime.close()

    def test_multiple_permissions(self):
        """Test registering op with multiple permissions."""
        runtime = Runtime.spawn()
        try:

            def complex_op(args):
                return "result"

            op_id = runtime.register_op(
                "complexOp",
                complex_op,
                permissions=["net:api.example.com", "file:/tmp", "env"],
            )
            assert op_id >= 0
        finally:
            runtime.close()

    def test_custom_permission(self):
        """Test registering op with custom permission."""
        runtime = Runtime.spawn()
        try:

            def custom_op(args):
                return "result"

            op_id = runtime.register_op(
                "customOp", custom_op, permissions=["custom:my_permission"]
            )
            assert op_id >= 0
        finally:
            runtime.close()

    def test_sync_op_permission_denied(self):
        """Sync ops should throw when permissions are missing."""
        runtime = Runtime.spawn()
        try:

            def read_secret(args):
                return "secret"

            op_id = runtime.register_op(
                "readSecret", read_secret, permissions=["file:/secure"]
            )

            result = runtime.eval(
                f"""
                try {{
                    __host_op_sync__({op_id}, '/secure/secret.txt');
                    'ok';
                }} catch (err) {{
                    err && err.message ? err.message : String(err);
                }}
                """
            )

            assert "Permission denied" in result
        finally:
            runtime.close()

    @pytest.mark.asyncio
    async def test_async_op_permission_denied(self):
        """Async ops should reject when permissions are missing."""
        with Runtime.spawn() as runtime:

            def fetch_data(args):
                return {"ok": True}

            op_id = runtime.register_op(
                "fetchData", fetch_data, mode="async", permissions=["net:example.com"]
            )

            result = await runtime.eval_async(
                f"""
                (async () => {{
                    try {{
                        await __host_op_async__({op_id}, 'https://example.com/');
                        return 'ok';
                    }} catch (err) {{
                        return err && err.message ? err.message : String(err);
                    }}
                }})()
                """
            )

            assert "Permission denied" in result


class TestContextManager:
    """Test runtime context manager with ops."""

    def test_ops_work_with_context_manager(self):
        """Test that ops work correctly with context manager."""
        with Runtime.spawn() as runtime:

            def my_op(args):
                return "ok"

            op_id = runtime.register_op("test", my_op)
            assert op_id >= 0


class TestOpBootstrap:
    """Test that op bootstrap JavaScript is injected."""

    def test_op_globals_exist(self):
        """Test that op system globals are available in JavaScript."""
        runtime = Runtime.spawn()
        try:
            # Check if the op bootstrap globals exist
            result = runtime.eval("typeof __host_op_sync__")
            assert result == "function"

            result = runtime.eval("typeof __host_op_async__")
            assert result == "function"

            result = runtime.eval("typeof __resolveOp")
            assert result == "function"
        finally:
            runtime.close()

    def test_promise_stats_available(self):
        """Test that promise registry stats are available."""
        runtime = Runtime.spawn()
        try:
            result = runtime.eval("typeof __getPromiseStats")
            assert result == "function"

            # Call the stats function
            result = runtime.eval("""
                const stats = __getPromiseStats();
                JSON.stringify({
                    hasNextId: typeof stats.nextPromiseId === 'number',
                    hasRingSize: typeof stats.ringSize === 'number',
                    hasMapSize: typeof stats.mapSize === 'number'
                })
            """)

            import json

            stats = json.loads(result)
            assert stats["hasNextId"] is True
            assert stats["hasRingSize"] is True
            assert stats["hasMapSize"] is True
        finally:
            runtime.close()


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
