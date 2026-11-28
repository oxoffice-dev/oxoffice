---
date: 2026-11-28
status: accepted
scope: ci
---

# A faster CI for pull requests: parallel, sharded, light on macOS and Windows

## Context

Measured on main run 46113183563 and pull request run 46114587742
(2026-11-28), and on #716 (about 17 minutes): each of the three `Test` jobs
ran, one step after the other, the wheel build, the Rust tests and the
Python tests.

| Job | Wheel | Rust tests | Python tests | Job |
| --- | --- | --- | --- | --- |
| Test (ubuntu-latest) | 1.4 min | 6.1 min (1.4 compiling, 4.7 running) | 7.7 min | 15.7 min |
| Test (macos-latest) | 1.2 to 2.3 min | 3.8 to 7.6 min | 4.0 to 7.0 min | 9.4 to 17.6 min |
| Test (windows-latest) | 2.3 min | 7.2 to 7.9 min | 8.1 to 8.9 min | 18.3 to 20.0 min |
| Format, clippy, licences, WebAssembly | | | | 4.6 to 5.6 min |
| Timings (ubuntu-latest) | 1.3 to 1.5 min | | 1.9 to 2.6 min (scaling) | 3.6 to 4.5 min |

The pull request waited for the slowest job, Windows. Most of the time is
in two places: the one `docx` test binary (1164 tests, 197 s of the 285 s
the Linux Rust tests take once compiled), and the Python tests
(`test_docx_tracked_changes.py` alone is 46 % of their time, 297 cases,
most of about 1.8 s). The cargo build of the tests, 85 s on Linux with the cache,
is what a shard pays again.

Measured on one 32-core machine at 4 threads, as a runner: `cargo test`
103 s; `cargo nextest` over three shards 39 + 36 + 33 s, the same 1984 tests
(one process each: no cost over `cargo test`); the Python suite 220 s at
4 workers and, in four shards, 137, 133, 132 and 151 s of test time (hash
of the test ids), 52 s for the fourth run alone.

## Decision

The maintainer's decision of 2026-11-28:

1. **Rust and Python tests are separate jobs**, side by side.
2. **Each suite is split into shards on parallel runners.**
   - Rust: cargo-nextest, `--partition hash:N/3` (three shards). nextest
     splits by test name, the tests of one binary included, which libtest
     filters cannot do without a list of modules to keep up to date; one
     process per test cost nothing measurable on the machine above, but it
     does on a runner: see Measured times. It runs the tests
     cargo runs (`--lib --test '*'` over the workspace) and reads the
     `[env]` of `.cargo/config.toml` (the 8 MiB test stack). Installed with
     `taiki-e/install-action` (a pinned SHA, the version pinned, checksum
     verified), `.config/nextest.toml` holds the profiles.
   - Python: `OXOFFICE_SHARD=I/4`, read by `conftest.py`: a test goes to
     the shard its id's CRC-32 picks, so the union is every test, a new
     test moves no other, and the shards' test time stays within 7 % of one
     another. pytest-xdist still spreads a shard over the CPUs. No new
     plugin.
   - Every Python shard builds the wheel itself (the cache keeps it about a
     minute and a half): the shards start together and nothing is handed
     from a job to the next. The timings job still builds its own.
3. **macOS and Windows are light on pull requests**; Linux is complete.
   Each runs one Rust job and one Python job: the wheel built there, the
   smoke test of the CLI, then
   - Rust, `cargo nextest run --profile smoke`: the crates that touch the
     operating system, all their tests (`oxoffice-opc` writing and
     replacing files in place, `oxoffice-core`, `oxoffice-cli` and its
     support, the command line end to end on files, paths and streams;
     `oxoffice-layout`, system fonts; `oxoffice-pdf`), and the save and
     stack tests of the facades (`pptx` `save`, `stack`; `xlsx` `saving`,
     `signatures`) and three docx tests that save a document to a file
     (the docx facade has no stack test): 208 tests, 8 s on the machine
     above. The job counts the tests the profile selects and fails under a
     floor (206, `SMOKE_FLOOR` in `ci.yml`), so that a rename that makes a
     clause match nothing cannot shrink the smoke unseen; every run also
     passes `--no-tests=fail`, so an empty shard fails;
   - Python: `test_save`, `test_package_errors`, `test_limitations`,
     `test_missing_glyphs`, `test_xlsx_workbook`,
     `test_non_xml_characters`, `test_core_property_dates`: 185 tests, 4 s.
   The smoke profile selects tests but builds the whole workspace: the
   features a `-p` subset unifies differ, which recompiles every
   third-party crate, and the cache is the point.
4. **The complete suites run on all three systems on every push to main**
   (and on a dispatch): the same shards on Linux and Windows (3 Rust, 4
   Python); on macOS 2 Rust and 3 Python, 5 jobs in all, the organization's
   cap on concurrent macOS jobs, so that one merge does not queue itself. They
   replace the three `Test` jobs of before, so a platform-only failure
   outside the smoke shows on main, one merge after the pull request
   instead of in it. **No schedule**: `ci.yml` and `full-checks.yml` have
   none (the nightly run was removed on 2026-11-25), and nothing else runs
   these suites at night, so no cron goes and no coverage with it. What
   the removed night schedule once covered, the debug-assertion runs, the
   whole corpus and every scaling call, stays on demand in
   `full-checks.yml`, unchanged.

A `plan` job writes the shard matrices from the event (a pull request, or
anything else), because a job's `if` cannot read its matrix and a skipped
leg would still start a runner. Its numbers, `RUST_SHARDS` and
`PYTHON_SHARDS`, are the knobs.

Caches, as before: one key, `test` (the cargo registry and the third-party
crates of the test build and of the wheel build), restored by every job
(`save-if: false`) and saved by main only. The one saver is the first Python
shard of each platform on main, on a key miss (a new `Cargo.lock` or
toolchain), which compiles the Rust tests right after its wheel, so that the
cache holds both builds, as the single job of before left it. It saves only
once both builds have succeeded (a failing test after them still saves): a
cache with the wheel build alone would hit its exact key from then on, skip
the compile and leave every Rust shard cold. A shard that raced
for the key would leave half of it. `full-checks.yml` restores the same
key. Without a cache a Linux shard compiles about a minute longer (the
workspace crates, most of the compile, always rebuild: a fresh checkout
makes the sources newer than the cached artifacts); with the cache it
compiles the workspace only.

Branch protection: `main` requires no status check (the ruleset forbids
deletion and a non-fast-forward push only), no workflow reads a job name of
`ci.yml`, so the renamed jobs need no aggregating job. A required check
later would take a final job that depends on all of them.

Not this time: selecting tests by the crates a pull request touches, a
smaller `lint`, nextest in `full-checks.yml`.

## Job graph

Before, on a pull request (and on main):

```
lint ------------------------------------ 4.6-5.6 min
timings --------- 3.6-4.5 min
Test (ubuntu)  [wheel 1.4][ Rust 6.1 ][ Python 7.7 ]----------- 15.7 min
Test (macos)   [wheel 1.2-2.3][ Rust 3.8-7.6 ][ Python 4.0-7.0 ] 9.4-17.6 min
Test (windows) [wheel 2.3][ Rust 7.2-7.9 ][ Python 8.9 ]-------- 18.3-20.0 min
```

After, on a pull request (measured on its own run, 46120030509, head 078de2a3; the
run took 6.9 minutes):

```
plan (3 s)
lint ------------------------------------ 5.5 min
timings -------- 3.2 min
Rust tests (ubuntu, 1/3 .. 3/3)     3 x [ nextest build 1.7 + run 3.9-4.7 ]  6.0-6.7 min
Python tests (ubuntu, 1/4 .. 4/4)   4 x [ wheel 1.5 + tests 2.0-2.7 ]       3.9-4.5 min
Rust tests (macos, smoke)           [ build + 208 tests ]                    4.7 min
Python tests (macos, smoke)         [ wheel 0.9 + 7 files ]                  1.4 min
Rust tests (windows, smoke)         [ build + 208 tests ]                    3.5 min
Python tests (windows, smoke)       [ wheel 2.1 + 7 files ]                  3.0 min
```

After, on a push to main or a dispatch: the same, with the macOS and
Windows legs sharded like the Linux ones (Windows 3 Rust and 4 Python
shards; macOS 2 and 3). Not measured yet: the first push to main is the run.

## Measured times

Pull request run 46120030509 (head 078de2a3), against the table of the
Context:

| | Before | Measured |
| --- | --- | --- |
| Pull request, wall time | 17 to 20 min | 6.9 min |
| Linux tests | 15.7 min | 6.0 to 6.7 min (Rust), 3.9 to 4.5 min (Python) |
| macOS | 9.4 to 17.6 min | 4.7 min (Rust smoke), 1.4 min (Python smoke) |
| Windows | 18.3 to 20.0 min | 3.5 min (Rust smoke), 3.0 min (Python smoke) |
| Lint, Timings | 4.6 to 5.6, 3.6 to 4.5 min | 5.5, 3.2 min |
| Push to main | 17 to 20 min | not measured |

The Python shards and the smokes met the estimate; the Rust shards did not
(about 3.5 min expected, 6.0 to 6.7 measured), and they, not lint, are the
longest jobs. The cause is in the tests, not the runners. One process per
test (nextest) gives up what a single `cargo test` process shares: the
nine `render::` tests of the docx binary each render the corpus subset
(122 to 145 s each on a runner, 1100 s of CPU in all, where one process
renders it once), and `equivalent_writings` is one test of 197 s that no
partition can split. The compile of a shard (1.7 min) plus that test is the
floor of any shard that holds it. The fix, for a follow-up: the render
tests together in one shard that runs by libtest filter (`render::`, as
`full-checks.yml` does), the writings walk in a shard of its own, nextest
partitions for the rest (expected: about 4.5 min for the Rust shards).

## Plan

One pull request: `ci.yml`, `.config/nextest.toml`, the shard hook in
`crates/oxoffice-py/tests/conftest.py`, `docs/DEVELOPMENT.md`, this spec.
Checked by `actionlint`, by the three Rust shards and the four Python
shards run locally (the union is the whole suite: 1984 Rust tests, 3118
Python tests collected, no test twice), and by the pull request's own run,
whose times are in the Measured times section; and, after the merge, by the
first push to main, whose three platforms run in full.

Afterwards, if the timings ask for it: `lint` is the longest job of a pull
request (clippy twice, the pptx tests without `render`): the last could be
a job of its own; more Windows shards if they are the longest on main.

## Links

- [One CI for pull requests and main](2026-11-22-one-ci-for-pull-requests-and-main.md),
  [a faster build job](2026-11-21-faster-build-job.md),
  [full checks on demand](2026-11-25-full-checks-on-demand.md)
- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md#ci)
