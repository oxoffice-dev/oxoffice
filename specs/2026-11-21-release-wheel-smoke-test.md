---
date: 2026-11-21
status: accepted
scope: release
---

# Release wheels run on the main thread before publishing

## Context

Unoptimised builds overflow the main thread's stack on Windows on PowerPoint
work: opening a 5-slide deck in the `scaling` profile crashed Python, and
`test_save.py` crashed in the `ci` profile. The Python tests now run each
test on an 8 MiB thread (#14), so CI no longer sees it.

The release wheels (fat LTO, opt-level 3) have much smaller frames, but
nothing checks them. A Windows user who calls `Presentation("deck.pptx")`
from a script runs on the main thread of `python.exe`, which reserves
2,000,000 bytes (read from the 3.12 executable's header; most other Windows
programs, including those that embed Python, get the linker's 1 MiB). An
overflow ends the process without a traceback.

On Linux, the 0.1.0 wheel needs about 816 KiB of stack to open a deck: 38
frames, a few of them very large (`resolve_master_records` 165 KB, the
binding's `Presentation.__new__` 148 KB, `from_fragment` 136 KB), so the
margin is large values kept on the stack, not recursion.

## Decision

A `smoke` job in `release.yml` installs each wheel on its own platform (Linux
x86_64 and aarch64, macOS arm64 and x86_64, Windows x64) and runs
`scripts/wheel_smoke.py` with the runner's Python: every Word and PowerPoint
file of the corpus is opened, edited (paragraph, table, slide added,
duplicated and removed, counted replacement), saved, reopened and rendered to
PDF, all on the main thread, and the `oxoffice` command answers
`--version`. The GitHub release, PyPI and prefix.dev wait for it.

The main thread alone guards little: 8 MiB on Linux and macOS, 2,000,000
bytes under `python.exe`. So the job then measures the smallest thread stack
on which the same work completes (a bisection in fresh processes, shown in
the job summary, in steps of 16 KiB, 64 KiB on Windows where a reserve is
rounded to that) and **fails above a budget of 1 MiB** (`STACK_BUDGET_KIB`),
the main thread of most Windows programs that embed Python. On Windows the
probe threads are made with `CreateThread` and
`STACK_SIZE_PARAM_IS_A_RESERVATION`: `threading.stack_size` only sets the
commit size there, and the executable's 2,000,000-byte reserve would hide
any smaller need. A measure that completes on 64 KiB fails the job too, since
the stack size was then not honoured, and so does a probe that ends with a
Python error, shown with its traceback rather than read as an overflow.

The version the wheel reports must be the release version, and `prepare`
only accepts `X.Y.Z`, before it tags: a prerelease would need its own PEP 440
and conda spellings and a GitHub release not marked latest, none of which the
workflow does yet.

It is a plain script, not pytest, so that nothing moves the work to another
thread. It does not replace the tests: it checks that the wheels we publish
load and work where users run them.

Not done here:

- The CLI binaries already run on an 8 MiB thread on Windows, and the conda
  packages repackage the same wheels.
- The tag is still pushed before the builds, so a failed check leaves the
  release commit and tag with nothing published. `docs/RELEASE.md` says to
  dry run first and how to recover. Tagging after the checks means building
  from an unpushed commit, a separate change.

## Plan

1. The job and the script, then a dry run of the release workflow from the
   branch, which builds `main` (0.1.0) and runs the script on its wheels.
2. If a wheel is over the budget: cut the stack the open and commit paths
   use (large values kept by value), or run the bindings' work on a thread
   with a larger stack, then a patch release.

## Links

- #15, where the release wheels were left unchecked
- #14, the Python tests on an 8 MiB thread
- [2026-11-20-ci-and-release.md](2026-11-20-ci-and-release.md)
