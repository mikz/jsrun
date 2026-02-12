from __future__ import annotations

import asyncio
from concurrent.futures import ThreadPoolExecutor

import pytest

import jsrun


def _explicit_runtime_worker(iterations: int) -> None:
    with jsrun.Runtime() as rt:
        fn = rt.eval("(x) => x + 1")
        rt.bind_function("add", lambda x, y: x + y)
        for i in range(iterations):
            assert rt.eval(f"{i} + 1") == i + 1
            assert fn(i) == i + 1
            assert rt.eval("add(20, 22)") == 42


def _module_runtime_worker(worker_id: int, iterations: int) -> None:
    jsrun.eval(f"globalThis.workerId = {worker_id}")
    assert jsrun.eval("globalThis.workerId") == worker_id

    fn_name = f"add_{worker_id}"
    jsrun.bind_function(fn_name, lambda x: x + 1)
    for i in range(iterations):
        assert jsrun.eval(f"{fn_name}({i})") == i + 1

    jsrun.close_default_runtime()


def test_concurrent_eval_and_function_calls_explicit_runtime() -> None:
    # This test is intentionally small; it exists primarily to catch deadlocks /
    # missing `py.detach(...)` around blocking sync operations on free-threaded builds.
    iterations = 25
    workers = 6

    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = [
            executor.submit(_explicit_runtime_worker, iterations) for _ in range(workers)
        ]
        for future in futures:
            future.result(timeout=30)


def test_concurrent_module_level_eval_and_bind_function() -> None:
    iterations = 10
    workers = 6

    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = [
            executor.submit(_module_runtime_worker, worker_id, iterations)
            for worker_id in range(workers)
        ]
        for future in futures:
            future.result(timeout=30)


@pytest.mark.asyncio
async def test_concurrent_eval_async_single_runtime() -> None:
    with jsrun.Runtime() as rt:
        coros = [rt.eval_async("Promise.resolve(1)") for _ in range(50)]
        results = await asyncio.gather(*coros)
        assert results == [1] * 50
