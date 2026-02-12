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
  - `pyo3 = 0.27.0` (`Cargo.toml`).
  - The extension module is `jsrun._jsrun`; Rust module init is `#[pymodule] fn _jsrun(...)` in `src/lib.rs`.
  - `_jsrun` currently does **not** set `gil_used = false`.
  - Global Python-object singleton: `src/runtime/python/mod.rs` uses `OnceLock<Py<JsUndefined>>`.
  - Some sync calls already use `py.detach(...)` (e.g. `Runtime.eval`), but other sync paths block without detaching (notably `JsFunction.__call__`, plus various `RuntimeHandle` command/recv methods).
- Python:
  - `python/jsrun/__init__.py` stores a ContextVar `_RuntimeSlot(runtime, owner)` where owner is the current asyncio Task (if any) else the current Thread.
  - This already causes the default runtime to be recreated when used from a different thread/task; keep this, but add a free-threaded regression test because free-threaded builds inherit contextvars into new threads by default.

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
- Gap A: `_jsrun` module not marked `gil_used = false` → import enables GIL.
- Gap B: Global singleton uses `OnceLock<Py<...>>` → replace with `PyOnceLock` or use `OnceLockExt::get_or_init_py_attached`.
- Gap C: Blocking sync operations run while attached:
  - `JsFunction.__call__` blocks on `call_function_sync` without `py.detach`
  - other `RuntimeHandle` blocking calls (close/terminate/register_op/etc) need a detach audit
- Gap D: CI likely builds `cp314t` wheels already (manylinux images ship `cp314-cp314t` and the build uses `maturin build --find-interpreter`), but:
  - no CI job runs under a free-threaded interpreter
  - wheel-selection logic can pick a non-`t` wheel when testing “3.14”

---

## Roadmap (PR-sized)
### PR 1: “GIL stays off” proof-of-life
- Rust:
  - `src/lib.rs`: change to `#[pymodule(gil_used = false)] fn _jsrun(...)`.
- Tests:
  - Add a free-threading-only import test:
    - skip unless `sysconfig.get_config_var("Py_GIL_DISABLED") == 1`
    - run with `PYTHON_GIL=0` and `-W error::RuntimeWarning`
    - assert `sys._is_gil_enabled() is False` after `import jsrun`
- CI:
  - Add `python-version: '3.14t'` jobs (at least Linux x86_64 + macOS arm64).

### PR 2: Single-init safety (PyOnceLock / OnceLockExt)
- `src/runtime/python/mod.rs`:
  - Replace `OnceLock<Py<JsUndefined>>` with `PyOnceLock<Py<JsUndefined>>`, or
  - keep `OnceLock` but use `OnceLockExt::get_or_init_py_attached(py, ...)`.

### PR 3: Detach around blocking sync calls
- Add `py.detach(|| ...)` around blocking Rust operations that do not touch Python objects.
- Minimum targets:
  - `JsFunction.__call__`: detach around `handle.call_function_sync(...)`.
  - `Runtime.close`, `Runtime.terminate`, `Runtime.register_op`, and other sync methods that wait on runtime-thread responses.

### PR 4: CI + publishing guardrails
- Ensure `cp314t` wheels are explicitly tested:
  - Linux x86_64: when running `3.14t`, install a `*cp314t*` wheel (not `*cp314*`).
  - Linux aarch64: do not use `python:3.14-slim` for `t` testing; run tests in a container that has a free-threaded interpreter (e.g. manylinux’s `/opt/python/cp314-cp314t/bin/python`).
  - macOS arm64: run tests under `actions/setup-python@v6` with `python-version: 3.14t`.
- Add a release gate:
  - If artifacts contain any `*cp31*t*` wheel, require the free-threaded test jobs to pass before publishing.

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
