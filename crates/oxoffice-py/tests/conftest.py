"""Shared pytest wiring of the Python tests."""
import threading

import pytest

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
