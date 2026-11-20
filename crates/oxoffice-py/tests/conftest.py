"""Shared pytest wiring of the Python tests.

The scaling tests (test_*_scaling.py) time every public mutating call at N and 4N. They need a build without debug
assertions (`maturin build --profile scaling` or `--release`): debug builds check every edit against a copy of the
whole document, so every call grows with it. OXOFFICE_SCALING picks what runs:

- unset: only the checks that every mutating call is timed
- quick: also the calls of each module's QUICK set, for pull requests
- full: every call, for main
"""
import os
import threading

import pytest

SCALING = os.environ.get("OXOFFICE_SCALING", "")
TIMED_TESTS = ("test_call_cost_does_not_grow_with_the_document", "test_call_cost_does_not_grow_with_the_deck")


def debug_build():
    import oxoffice.docx._native as native

    return hasattr(native, "_staging_work")


def pytest_configure(config):
    if SCALING not in ("", "quick", "full"):
        raise pytest.UsageError(f"OXOFFICE_SCALING={SCALING!r}: use quick or full")
    if SCALING and debug_build():
        raise pytest.UsageError("OXOFFICE_SCALING needs a build without debug assertions: "
                                "maturin build --profile scaling (or --release)")


def selected(item):
    if item.originalname not in TIMED_TESTS or SCALING == "full":
        return True
    return SCALING == "quick" and item.callspec.params["name"] in item.module.QUICK


def pytest_collection_modifyitems(config, items):
    kept = [item for item in items if selected(item)]
    config.hook.pytest_deselected(items=[item for item in items if not selected(item)])
    items[:] = kept


# Unoptimised builds (the `ci` profile) need more stack for some PowerPoint
# edits than the 1 MiB of the Windows main thread. Each test runs on a thread
# with the 8 MiB the `oxoffice` command gives its own thread.
STACK = 8 * 1024 * 1024


@pytest.hookimpl(tryfirst=True)
def pytest_pyfunc_call(pyfuncitem):
    args = {name: pyfuncitem.funcargs[name] for name in pyfuncitem._fixtureinfo.argnames}
    raised = []

    def run():
        try:
            pyfuncitem.obj(**args)
        except BaseException as error:  # noqa: BLE001 - raised again on the main thread
            raised.append(error)

    previous = threading.stack_size(STACK)
    try:
        thread = threading.Thread(target=run)
        thread.start()
    finally:
        threading.stack_size(previous)
    thread.join()
    if raised:
        raise raised[0]
    return True
