"""
Type stubs for the jsrun Python extension module.
"""

import types
from typing import (
    Any,
    Callable,
    List,
    Optional,
    Self,
    Tuple,
)

__all__ = [
    "Runtime",
    "JavaScriptError",
    "PromiseTimeoutError",
    "V8Error",
]

# Exception hierarchy
class JavaScriptError(Exception):
    """Exception raised when a JavaScript function throws an error."""

    ...

class PromiseTimeoutError(Exception):
    """Exception raised when a promise times out during await."""

    ...

class V8Error(Exception):
    """Base exception for V8 errors."""

    ...

# Core runtime types

class Runtime:
    """
    Async JavaScript runtime.

    Each Runtime runs on a dedicated OS thread with its own V8 isolate
    and provides async-first JavaScript execution with promise support.
    """

    def __init__(self) -> None: ...
    def eval(self, code: str) -> str:
        """
        Evaluate JavaScript code synchronously.

        This is a convenience method that blocks on the async evaluation.
        For better performance with promises, use eval_async() instead.

        Args:
            code: JavaScript source code to evaluate

        Returns:
            JSON string representation of the result

        Raises:
            RuntimeError: If evaluation fails or times out
            JavaScriptError: If JavaScript code throws an exception

        Example:
            >>> runtime = v8.Runtime()
            >>> result = runtime.eval("1 + 1")
            >>> print(result)
            "2"
        """
        ...

    async def eval_async(self, code: str, *, timeout_ms: Optional[int] = None) -> str:
        """
        Evaluate JavaScript code asynchronously.

        This method supports promises and will wait for them to resolve.
        It's the recommended way to execute JavaScript code.

        Args:
            code: JavaScript source code to evaluate
            timeout_ms: Optional timeout in milliseconds

        Returns:
            JSON string representation of the result

        Raises:
            RuntimeError: If evaluation fails
            JavaScriptError: If JavaScript code throws an exception
            PromiseTimeoutError: If timeout is exceeded

        Example:
            >>> runtime = v8.Runtime()
            >>> result = await runtime.eval_async("Promise.resolve(42)")
            >>> print(result)
            "42"
        """
        ...

    def register_op(
        self,
        name: str,
        handler: Callable[..., Any],
        *,
        mode: str = "sync",
    ) -> int:
        """
        Register a host operation that can be called from JavaScript.

        Args:
            name: Operation name (must be unique)
            handler: Python callable that handles the operation
            mode: Operation mode ("sync" or "async")

        Returns:
            Operation ID that can be used in JavaScript

        Raises:
            RuntimeError: If registration fails

        Example:
            >>> def add_handler(a, b):
            ...     return a + b
            >>> op_id = runtime.register_op("add", add_handler, mode="sync")
            >>> # From JavaScript: __host_op_sync__(op_id, 10, 20)  # Returns 30
        """
        ...

    def is_closed(self) -> bool:
        """
        Check if the runtime is closed.

        Returns:
            True if the runtime is closed, False otherwise
        """
        ...

    def close(self) -> None:
        """
        Close the runtime and free all resources.

        After calling close(), the runtime can no longer be used.
        This method is called automatically when using the runtime
        as a context manager.
        """
        ...

    def bind_function(
        self,
        name: str,
        handler: Callable[..., Any],
    ) -> None:
        """
        Expose a Python handler as a global JavaScript function.

        Args:
            name: Global function name (assigned on `globalThis`)
            handler: Python callable invoked when JS calls the function

        Example:
            >>> runtime = v8.Runtime()
            >>> def add(a, b): return a + b
            >>> runtime.bind_function("add", add)
            >>> runtime.eval("add(1, 2)")
            "3"
        """
        ...

    def __enter__(self) -> Self:
        """Context manager entry - returns self."""
        ...

    def __exit__(
        self,
        exc_type: Optional[type[BaseException]],
        exc_val: Optional[BaseException],
        exc_tb: Optional[types.TracebackType],
    ) -> bool:
        """Context manager exit - closes the runtime."""
        ...

    def __repr__(self) -> str: ...
