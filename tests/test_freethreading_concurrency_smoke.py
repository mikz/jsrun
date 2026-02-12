from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

import jsrun


def _worker(iterations: int) -> None:
    with jsrun.Runtime() as rt:
        fn = rt.eval("(x) => x + 1")
        for i in range(iterations):
            assert rt.eval(f"{i} + 1") == i + 1
            assert fn(i) == i + 1


def test_concurrent_eval_and_function_calls() -> None:
    # This test is intentionally small; it exists primarily to catch deadlocks /
    # missing `py.detach(...)` around blocking sync operations on free-threaded builds.
    iterations = 25
    workers = 6

    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = [executor.submit(_worker, iterations) for _ in range(workers)]
        for future in futures:
            future.result(timeout=30)

