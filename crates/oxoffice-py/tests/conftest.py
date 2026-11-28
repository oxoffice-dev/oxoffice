"""Shared pytest wiring of the Python tests.

The scaling tests (test_*_scaling.py) time every public mutating call at N and 16N (docx) or 4N (pptx, 16N for the
calls that shift every slide), and the reads of a workbook at N and 4N rows (xlsx). They need a build without debug assertions (`maturin build --profile scaling` or
`--release`): debug builds check every edit against a copy of the whole document, so every call grows with it.
OXOFFICE_SCALING picks what runs:

- unset: only the checks that every mutating call is timed, and each timed docx call made once, untimed
- quick: also the calls of each module's QUICK set, for pull requests and main (ci.yml)
- full: every call, in the full checks (full-checks.yml)

OXOFFICE_SHARD=I/N runs the I-th of N shards of the tests (the CI of pull requests and main runs them on parallel
runners, specs/2026-11-28-faster-pull-request-ci.md): each test goes to the shard its id's checksum picks, so the
shards' union is every test and a new test moves no other.
"""
import os
import threading
import zlib

import pytest

SCALING = os.environ.get("OXOFFICE_SCALING", "")
TIMED_TESTS = (
    "test_call_cost_does_not_grow_with_the_document",
    "test_call_cost_does_not_grow_with_the_deck",
    "test_read_cost_follows_the_cells_read",
)


def debug_build():
    import oxoffice.docx._native as native

    return native._debug_build


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


def shard():
    spec = os.environ.get("OXOFFICE_SHARD", "")
    if not spec:
        return None
    try:
        index, count = map(int, spec.split("/"))
        assert 1 <= index <= count
    except (ValueError, AssertionError):
        raise pytest.UsageError(f"OXOFFICE_SHARD={spec!r}: use I/N, with 1 <= I <= N") from None
    return index - 1, count


def in_shard(item, wanted):
    return wanted is None or zlib.crc32(item.nodeid.encode()) % wanted[1] == wanted[0]


def pytest_collection_modifyitems(config, items):
    wanted = shard()
    kept = [item for item in items if selected(item) and in_shard(item, wanted)]
    kept_ids = {id(item) for item in kept}
    config.hook.pytest_deselected(items=[item for item in items if id(item) not in kept_ids])
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
