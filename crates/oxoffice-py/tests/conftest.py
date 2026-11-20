"""Shared pytest wiring of the Python tests.

The scaling tests (test_*_scaling.py) time every public mutating call at N and 4N. They are opt-in while the
quick/full split is being settled: set OXOFFICE_SCALING=full to run them. The completeness checks of the API surface
always run."""
import os
import threading

import pytest

SCALING = os.environ.get("OXOFFICE_SCALING", "")


def pytest_collection_modifyitems(config, items):
    if SCALING == "full":
        return
    skip = pytest.mark.skip(reason="scaling test: set OXOFFICE_SCALING=full to run it")
    for item in items:
        if item.originalname in ("test_call_cost_does_not_grow_with_the_document",
                                 "test_call_cost_does_not_grow_with_the_deck"):
            item.add_marker(skip)


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
