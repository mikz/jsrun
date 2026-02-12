import sys
import sysconfig

import pytest


def _is_freethreaded() -> bool:
    return sysconfig.get_config_var("Py_GIL_DISABLED") == 1


def _gil_enabled() -> bool | None:
    probe = getattr(sys, "_is_gil_enabled", None)
    if probe is None:
        return None
    try:
        return bool(probe())
    except Exception:
        return None


def test_import_does_not_enable_gil() -> None:
    if not _is_freethreaded():
        pytest.skip("Not a free-threaded CPython build (Py_GIL_DISABLED != 1).")

    initial = _gil_enabled()
    if initial is None:
        pytest.skip("sys._is_gil_enabled() unavailable on this interpreter.")
    if initial:
        pytest.skip("GIL enabled at start; run with PYTHON_GIL=0 or -X gil=0.")

    import jsrun  # noqa: F401

    assert _gil_enabled() is False
