# jsrun + CPython free-threading (3.14t) plan

_Last reviewed: 2026-02-12_

## Goals (“done”)
1. Wheels for CPython 3.14 free-threaded ABI (`cp314t`) are built and published for jsrun’s supported platforms.
2. Importing `jsrun` on a free-threaded interpreter does **not** enable the GIL (no warning, and `sys._is_gil_enabled()` stays `False` when forced off).
3. Concurrency smoke tests pass: multiple Python threads calling `jsrun.eval()` concurrently, plus `eval_async` under asyncio, plus Python callbacks invoked from JS.

### Acceptance checks
- Confirm you’re running a free-threaded build:
  - `python -VV` contains “free-threading build”, or
  - `sysconfig.get_config_var("Py_GIL_DISABLED") == 1`
- Force the GIL to stay disabled and fail on warnings:
  - `PYTHON_GIL=0 python -W error::RuntimeWarning -c "import jsrun, sys; assert not sys._is_gil_enabled()"`
  - (alternative: `python -X gil=0 ...`)
- Concurrency smoke (examples):
  - `ThreadPoolExecutor` repeatedly calling `jsrun.eval("1+1")`
  - `asyncio` calls via `jsrun.eval_async("Promise.resolve(1)")`
  - Python callbacks invoked from JS (`bind_function`) under concurrent load

---

## Current repo state (facts)
- Rust:
  - `pyo3 = 0.28.0` (`Cargo.toml`).
  - The extension module is `jsrun._jsrun`; Rust module init is `#[pymodule] fn _jsrun(...)` in `src/lib.rs`.
  - `_jsrun` sets `gil_used = false` in `src/lib.rs` (PR 1).
  - Global Python-object singleton: `src/runtime/python/mod.rs` uses `PyOnceLock<Py<JsUndefined>>`.
  - Blocking sync calls detach where appropriate (e.g. `Runtime.eval`, `JsFunction.__call__`, `Runtime.close` / `terminate`, sync op registration).
- Python:
  - `python/jsrun/__init__.py` stores the default runtime per-thread (`threading.local()`) or per-asyncio-task (task attribute), not in a `ContextVar`.
  - pytest treats `PytestUnraisableExceptionWarning` containing `unsendable` as an error (`pyproject.toml`).
- CI:
  - Linux x86_64 wheel tests include `python-version: 3.14t` and run `tests/test_freethreading_import.py` under `PYTHON_GIL=0`.
  - Wheel installation in CI uses `pip install --no-index --find-links dist jsrun` so pip selects the correct wheel tag (`cp314-cp314` vs `cp314-cp314t`).
  - macOS has a dedicated `3.14t` test job.
  - Linux aarch64 has a dedicated `3.14t` test job using `/opt/python/cp314-cp314t/bin/python` in a manylinux container.

---

## What CPython requires for extensions (summary)
### 1) Declare “GIL not used” or CPython will enable the GIL at import
CPython free-threaded builds enable the GIL at import time unless the extension explicitly opts in:
- Multi-phase init: set the `Py_mod_gil` slot to `Py_MOD_GIL_NOT_USED`
- Single-phase init: call `PyUnstable_Module_SetGIL(m, Py_MOD_GIL_NOT_USED)` (free-threaded build only)

PyO3 maps this to:
- `#[pymodule(gil_used = false)]` (opt in)
- `#[pymodule(gil_used = true)]` (opt out)

Note: PyO3 0.23–0.27 defaulted to `gil_used = true`, so this repo (0.27) must opt in explicitly.

### 2) Build separate `cp314t` wheels
Free-threaded builds are a distinct ABI, with `t` in tags (e.g. `cp314t`).
The limited API / stable ABI (`abi3`) is not available for free-threaded wheels.

### 3) Detach around blocking operations
Even without the GIL, threads attach/detach so the runtime can run GC and other global synchronization.
Blocking while attached can deadlock; detach before blocking/waiting on mutexes, channel receives, and other long-running operations.

---

## Gaps + risks (repo-specific)
None identified that block free-threaded `3.14t` support.

---

## Roadmap (PR-sized)
### PR 1: “GIL stays off” proof-of-life (done)
### PR 2: Single-init safety (done)
### PR 3: Default runtime storage fix (ContextVar → task/thread storage)
### PR 4: Treat “unsendable” unraisables as pytest errors
### PR 5: Detach audit for blocking sync calls (+ concurrency smoke)
### PR 6: CI + publishing guardrails for `3.14t` (macOS arm64 + Linux aarch64)
### PR 8: Upgrade PyO3 to 0.28.0 and revalidate

---

## Test scenarios (must cover)
1. Import + GIL stays off (`PYTHON_GIL=0`, warnings-as-errors).
2. ThreadPool sync smoke (`jsrun.eval`).
3. ThreadPool async smoke (`jsrun.eval_async`).
4. Python callback ops smoke under concurrency (`bind_function`).
5. Context inheritance regression:
   - Parent thread initializes default runtime.
   - Child thread starts (inherits contextvars by default on free-threaded builds).
   - Child thread uses `jsrun.eval(...)` and gets its own runtime.

---

## Validation commands (local)
### Normal CPython (default project env)
- `uv sync --frozen --group testing`
- `uv run -m pytest -q`

### Free-threaded CPython 3.14t (wheel-based)
```sh
uvx python@3.14t -VV

TMPDIR=$(mktemp -d)
uv venv --seed -p 3.14t "$TMPDIR/venv"
"$TMPDIR/venv/bin/python" -m pip install -q pytest pytest-asyncio

uvx maturin build --release --out "$TMPDIR/dist" --interpreter "$TMPDIR/venv/bin/python"
"$TMPDIR/venv/bin/python" -m pip install -q --no-index --find-links "$TMPDIR/dist" --force-reinstall jsrun

PYTHON_GIL=0 "$TMPDIR/venv/bin/python" -W error::RuntimeWarning -m pytest -q
```

## References
- PyO3: Supporting Free-Threaded Python
  https://pyo3.rs/main/free-threading
- CPython: C API Extension Support for Free Threading
  https://docs.python.org/3/howto/free-threading-extensions.html
- CPython: Python support for free threading (PYTHON_GIL / -X gil / sys._is_gil_enabled)
  https://docs.python.org/3/howto/free-threading-python.html
- GitHub Actions: setup-python free-threaded versions
  https://github.com/actions/setup-python
- cibuildwheel free-threaded options (optional background)
  https://cibuildwheel.pypa.io/en/stable/options/

## Validation notes (why these edits are necessary)
- “Must declare GIL not used or import enables GIL” and the exact CPython mechanisms (`Py_mod_gil` / `PyUnstable_Module_SetGIL`) come from the CPython extension howto.
- “Free-threaded wheels are separate ABI; no limited API/stable ABI; `t` suffix; cibuildwheel builds free-threaded by default on 3.14” comes from CPython packaging guidance.
- PyO3 defaults + opt-in/out rules (including the PyO3 0.23–0.27 default and `#[pymodule(gil_used = false)]`) are in the PyO3 free-threading guide.
- `PyOnceLock` / `OnceLockExt::get_or_init_py_attached` guidance is in the PyO3 free-threading guide.
- “Detach around blocking operations to avoid deadlocks” is reinforced by PyO3’s `Python` API docs and aligns with CPython’s advice to keep using thread-state APIs around blocking work.
- “contextvars inherited by default on free-threaded builds” is in CPython’s free-threading behavior doc.
- Free-threaded `python-version: '3.13t'` syntax in GitHub Actions is documented by `actions/setup-python` (basis for `3.14t`).

## Assumptions / defaults
- Keep `Runtime`/`JsFunction`/`JsStream` thread-affine (still `unsendable`); no attempt to make objects safely shareable across Python threads.
- Focus on `cp314t` as the target; optionally include `cp313t` coverage if the build matrix already produces those wheels.
