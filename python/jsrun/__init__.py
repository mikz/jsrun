"""High-level Python bindings for the jsrun runtime."""

import asyncio
import atexit
import threading
from dataclasses import dataclass
from collections.abc import Callable
from typing import Any, TypeVar, cast, overload

from ._jsrun import (
    InspectorConfig,
    InspectorEndpoints,
    JavaScriptError,
    JsFunction,
    JsStream,
    JsUndefined,
    Runtime,
    RuntimeConfig,
    RuntimeStats,
    RuntimeTerminated,
    SnapshotBuilder,
    undefined,
)

F = TypeVar("F", bound=Callable[..., Any])


@overload
def _runtime_bind(self: Runtime, func: F, /, *, name: str | None = ...) -> F: ...


@overload
def _runtime_bind(
    self: Runtime, func: None = ..., /, *, name: str | None = ...
) -> Callable[[F], F]: ...


def _runtime_bind(
    self: Runtime, func: object | None = None, /, *, name: str | None = None
) -> Callable[[F], F] | F:
    """Bind a Python callable to the runtime via a decorator-friendly API.

    The helper accepts both synchronous and asynchronous callables, forwarding
    registration to :meth:`Runtime.bind_function` while returning the original
    callable for continued direct usage.
    """

    def _register(target: F) -> F:
        binding_name = name if name is not None else getattr(target, "__name__", None)
        if not binding_name:
            raise ValueError("A function name is required when binding to JavaScript")
        self.bind_function(binding_name, target)
        return target

    if func is None:
        return _register

    if not callable(func):
        raise TypeError("runtime.bind expects a callable or to be used as a decorator")

    return _register(cast(F, func))


@dataclass(slots=True)
class _RuntimeSlot:
    runtime: Runtime
    closed: bool = False

    def close(self) -> None:
        if self.closed:
            return
        self.closed = True
        if not self.runtime.is_closed():
            self.runtime.close()


def _current_asyncio_task() -> asyncio.Task[Any] | None:
    try:
        task = asyncio.current_task()
    except RuntimeError:
        task = None
    return task


_TASK_SLOT_ATTR = "__jsrun_default_runtime_slot__"
_thread_local = threading.local()


def _get_task_slot(task: asyncio.Task[Any]) -> _RuntimeSlot | None:
    return getattr(task, _TASK_SLOT_ATTR, None)


def _set_task_slot(task: asyncio.Task[Any], slot: _RuntimeSlot | None) -> None:
    if slot is None:
        if hasattr(task, _TASK_SLOT_ATTR):
            delattr(task, _TASK_SLOT_ATTR)
        return
    setattr(task, _TASK_SLOT_ATTR, slot)


def _get_thread_slot() -> _RuntimeSlot | None:
    return getattr(_thread_local, "slot", None)


def _set_thread_slot(slot: _RuntimeSlot | None) -> None:
    if slot is None:
        if hasattr(_thread_local, "slot"):
            delattr(_thread_local, "slot")
        return
    _thread_local.slot = slot


setattr(Runtime, "bind", _runtime_bind)


def get_default_runtime() -> Runtime:
    """Get or create a runtime isolated to the current context.

    In an asyncio app, this is per-task (e.g., per-request).
    In a sync app, this is per-thread.

    The runtime is created with default configuration. For custom configuration
    (heap limits, bootstrap code, etc.), use the Runtime class directly.

    Returns:
        Runtime: The context-local runtime instance.
    """
    task = _current_asyncio_task()
    if task is not None:
        slot = _get_task_slot(task)
        if slot is None or slot.runtime.is_closed():
            slot = _RuntimeSlot(runtime=Runtime())
            _set_task_slot(task, slot)
            task.add_done_callback(lambda _: slot.close())
        return slot.runtime

    slot = _get_thread_slot()
    if slot is None or slot.runtime.is_closed():
        slot = _RuntimeSlot(runtime=Runtime())
        _set_thread_slot(slot)
    return slot.runtime


def close_default_runtime() -> None:
    """Close the current context's runtime, if one exists."""
    task = _current_asyncio_task()
    if task is not None:
        slot = _get_task_slot(task)
        if slot is None:
            return
        slot.close()
        _set_task_slot(task, None)
        return

    slot = _get_thread_slot()
    if slot is None:
        return
    slot.close()
    _set_thread_slot(None)


def _close_default_runtime_on_exit() -> None:
    try:
        close_default_runtime()
    except RuntimeError:
        pass


atexit.register(_close_default_runtime_on_exit)


def eval(code: str) -> Any:
    """Evaluate JavaScript code synchronously using the default context-local runtime.

    This is a convenience function for simple use cases. Each asyncio task or thread
    gets its own isolated runtime automatically.

    For custom configuration or fine-grained control, use the Runtime class directly.

    Args:
        code: JavaScript code to evaluate.

    Returns:
        The result of the JavaScript evaluation, converted to Python types.

    Raises:
        JavaScriptError: If the JavaScript code throws an exception.

    Example:
        >>> import jsrun
        >>> jsrun.eval("2 + 2")
        4
        >>> jsrun.eval("Math.sqrt(16)")
        4.0
    """
    return get_default_runtime().eval(code)


async def eval_async(code: str, **kwargs) -> Any:
    """Evaluate JavaScript code asynchronously using the default context-local runtime.

    This is a convenience function for simple async use cases. Each asyncio task or
    thread gets its own isolated runtime automatically.

    For custom configuration or fine-grained control, use the Runtime class directly.

    Args:
        code: JavaScript code to evaluate.
        **kwargs: Additional arguments passed to Runtime.eval_async (e.g., timeout).

    Returns:
        The result of the JavaScript evaluation, converted to Python types.

    Raises:
        JavaScriptError: If the JavaScript code throws an exception.
        TimeoutError: If timeout is specified and exceeded.

    Example:
        >>> import asyncio
        >>> import jsrun
        >>> asyncio.run(jsrun.eval_async("Promise.resolve(42)"))
        42
    """
    return await get_default_runtime().eval_async(code, **kwargs)


def bind_function(name: str, handler: Callable[..., Any]) -> None:
    """Bind a Python function to the default context-local runtime.

    The function will be available as a global in JavaScript. Both sync and async
    Python functions are supported.

    Args:
        name: The name to bind in JavaScript globalThis.
        handler: The Python callable to bind (sync or async).

    Example:
        >>> import jsrun
        >>> jsrun.bind_function("add", lambda a, b: a + b)
        >>> jsrun.eval("add(2, 3)")
        5
    """
    get_default_runtime().bind_function(name, handler)


def bind_object(name: str, obj: dict) -> None:
    """Bind a Python dict as a JavaScript object in the default context-local runtime.

    Args:
        name: The name to bind in JavaScript globalThis.
        obj: The Python dict to expose as a JavaScript object.

    Example:
        >>> import jsrun
        >>> jsrun.bind_object("config", {"version": "1.0", "debug": True})
        >>> jsrun.eval("config.version")
        '1.0'
    """
    get_default_runtime().bind_object(name, obj)


__all__ = [
    "eval",
    "eval_async",
    "get_default_runtime",
    "close_default_runtime",
    "bind_function",
    "bind_object",
    "Runtime",
    "RuntimeConfig",
    "InspectorConfig",
    "InspectorEndpoints",
    "SnapshotBuilder",
    "JsFunction",
    "JsUndefined",
    "RuntimeStats",
    "JavaScriptError",
    "RuntimeTerminated",
    "undefined",
    "JsStream",
]
