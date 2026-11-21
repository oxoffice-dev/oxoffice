# Development

How work lands in oxoffice. Short on purpose.

## The loop

1. **Spec first** for anything non-trivial: open
   `specs/YYYY-MM-DD-name.md` as `draft` (see [specs/README.md](../specs/README.md)),
   agree on it, set it to `accepted`.
2. **Branch** off `main`: `feat/...`, `fix/...`, `perf/...`, `refactor/...`.
3. **Implement** with tests in the same pull request. A feature reaches the
   Rust facade, then the CLI where it makes sense and Python.
4. **Pull request** with a conventional title (checked by CI). Its body
   follows `.github/pull_request_template.md`: why, what changes, impact,
   tests, next, a few short bullets each, readable at a glance. Long notes,
   tables and measurements go in a `<details>` block. Issues follow the same
   shape.
5. **Independent review**, posted on the pull request, then answer every
   finding (see [Independent review](#independent-review)).
6. **CI green on the three platforms**, squash merge: `gh pr checks` lists
   `Build and test (ubuntu-latest)`, `(macos-latest)` and
   `(windows-latest)`. A pull request whose base was changed to `main` may
   only have its Linux run: push a new commit, or close and reopen it,
   first. A documentation-only pull request runs no build, so there is
   nothing to check. The squash commit takes the title, so
   `main` reads as a list of conventional commits, one per pull request, and
   the changelog is generated from it. Keep a pull request to one topic, so
   that its title can say all of it. Commits on a branch are conventional too.
7. **Update** `docs/` when a structural fact changes, and the spec's status
   and Learnings when it ships.

## Independent review

Every pull request is reviewed before it is merged by someone who did not
write it: a fresh agent given only the pull request, its branch and its spec,
never the author's reasoning, or a person. Whoever works on the pull request
or wants to merge it runs that review or asks for one.

- **Posted, unedited.** One comment on the pull request headed
  `🔍 Independent review`, with the head it read. The reviewer posts it; an
  agent that cannot write to GitHub hands its report to whoever launched it,
  who posts it as is. Each finding has `file:line`, the defect and a failure
  scenario, or the review says it found nothing.
- **Major** when it would ship a defect: wrong behaviour or a missed case, a
  test that passes with the feature broken, Rust, CLI and Python disagreeing,
  an unsafe CI change, a document that misstates how things work. Everything
  else is **minor**. The reviewer classifies, the author may contest, the
  maintainer decides.
- **The author answers every finding** in the thread: fixed, with the commit
  (and its proof for a minor), in the pull request's follow-up issue, or
  why it is not a defect. A major finding is fixed unless the maintainer
  waives it.
- **At most two reviews per pull request**: one full review, then at most
  one delta review, after a major was fixed or after a change no finding
  asked for that can change behaviour (new code, a conflict resolved by hand
  in code, tests or workflows). A third review needs the maintainer's
  agreement; beyond that budget the maintainer decides from the evidence
  (mutation checks, `git range-diff`), without another round.
- **Minors never trigger a review.** A minor found in review is fixed in
  the pull request when the fix is cheap, with its proof stated in the
  answer (a test that fails without it, or a mutation check). A follow-up
  issue is opened only for a real defect that the pull request cannot carry
  (a risky change included), and at most one per pull request: it groups
  those findings. Otherwise say why it is not a defect.
- **A rebase or merge that only resolves a conflict in data**
  (a limitations file, a generated page, the `GROWING` or test-module lists,
  a moved context line) needs no review: `git range-diff` and the tests, and
  one line in the thread saying so. A rebase without conflict needs nothing.
- **A delta review judges the delta only**: defects the new commits bring,
  or majors they fail to fix. No new finding on code the delta does not
  touch.
- **Light review**, still posted, for documentation (links, anchors, text
  agrees with the code and the other docs) and pure code moves (nothing but
  location changed). It counts as the full review.

See [the spec](../specs/2026-11-21-independent-review-before-merge.md).

## Conventional commits

```
<type>(<optional scope>): <lowercase subject>
```

Types: `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `build`, `ci`,
`chore`, `revert`. Scopes are free but usually one of `docx`, `pptx`,
`xlsx`, `cli`, `python`, `pdf`, `layout`, `opc`. A breaking change adds `!`
after the type and a `BREAKING CHANGE:` footer. The changelog groups commits
by type.

## Inner loop

```bash
cargo check -p <crate> --all-targets
cargo test -p <crate>                     # one crate
cargo test -p <crate> --test <file>       # one integration binary
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check --workspace --all-targets --all-features --profile scaling   # without debug assertions, as lint does
cargo fmt --all
cargo build -p oxoffice-cli               # the `oxoffice` binary
maturin develop -m crates/oxoffice-py/Cargo.toml   # the Python package, in a venv
```

- **Exclude the Python crates from `cargo test` on the workspace**
  (`--exclude oxoffice-py --exclude oxoffice-docx-py --exclude oxoffice-pptx-py`).
  PyO3's extension-module feature leaves Python symbols to the host
  interpreter, so a test binary fails to link, on Linux with a confusing
  unresolved-symbol error.
- The integration tests of a facade crate are one binary, one module per
  area: `crates/oxoffice-docx/tests/docx/` (`--test docx`) and
  `crates/oxoffice-pptx/tests/pptx/` (`--test pptx`). A new area is a new
  module, not a new file under `tests/`, so each crate links one binary.
- Run the tests you touched locally. CI runs the whole suite.
- Never `cargo clean` the shared target directory. Use one target
  directory per worktree.
- Rendering checks use the bundled fonts only. System fonts differ between
  machines.

## Scaling tests

`crates/oxoffice-py/tests/test_{docx,pptx}_scaling.py` time every public
mutating call in a document of N paragraphs and in one of 16N (a deck of N
slides and one of 4N). The bodies of the Word documents carry the
identifiers Word writes on every paragraph, run and row, so that each call
takes the route a saved body takes. They need a build without debug assertions: debug builds
check every edit against a copy of the whole document, so every call grows
with it.

```bash
maturin build --profile scaling -m crates/oxoffice-py/Cargo.toml -o dist
pip install --force-reinstall dist/*.whl
OXOFFICE_SCALING=quick pytest crates/oxoffice-py/tests   # the QUICK sets
OXOFFICE_SCALING=full pytest crates/oxoffice-py/tests    # every call
```

Without `OXOFFICE_SCALING`, only the check that every mutating call is timed
runs, and every timed docx call is made once, untimed, on a small document,
which is then saved and opened again: on a debug build that puts each of them
under the whole-document cross-checks.
A docx call of `GROWING` is a strict xfail tied to its issue: when a fix
makes it flat, the XPASS fails the run until the call leaves the set. A call
outside the band it is expected in (under 2.5 times, or 1.5 to 24 times for a
docx call of `GROWING`) is measured again, so that a load spike on a runner
neither fails a flat call nor reads as a fix.

## Render tests

The `render` modules of the docx and pptx test binaries render every file
of the corpus as `convert` and `render` do, with the bundled fonts, and
check two things (see [the spec](../specs/2026-11-21-render-fidelity.md)).
What they share (the PDF reader, the hashes, the baseline files and the
checks) lives in the dev-only crate `oxoffice-render-test`.

- **Invariants**, read from the PDF and the page images: page sizes in the
  order of the sections (one page of the slide size per slide), no page
  with only its header and footer, every word drawn and inside its page (a
  slide's words on its own page), no empty-box glyph, no text over text,
  every picture and table border drawn, no diagnostic but those known to
  change nothing drawn. What fails today is listed in
  `tests/render/known.toml` with its issue, and an entry fails the test once
  its file passes, or, for a whole-document entry, once its finding
  changes.
- **The baseline**: one file per corpus file under `tests/render/baseline/`
  with a hash per page of the pixels and of the PDF. The render is the same
  bytes on Linux, macOS and Windows, so any change fails the test. A pull
  request that changes the render on purpose rewrites the baselines, in a
  commit of its own that says what changed:

```bash
OXOFFICE_RENDER=subset cargo test --profile ci -p oxoffice-docx --test docx render   # check
OXOFFICE_RENDER_UPDATE="<reason>" cargo test --profile ci -p oxoffice-docx --test docx render
# the same with -p oxoffice-pptx --test pptx for the decks
```

`OXOFFICE_RENDER` is `subset` (the corpus without its `long-*` files),
`full` or `off`. Unset, the tests over the corpus skip, except under CI,
where they fail: CI says which set it runs. The tests of the checks
themselves, on a few built documents and corpus files, always run. The
update always renders the whole corpus and only rewrites the files that
moved.

## Known limitations

A limitation users can hit, with a workaround, gets its own file,
`crates/oxoffice-cli/limitations/<id>.json` (one JSON object: `id`, the file
name; `format`, `area`, `title`, `symptom`, `workaround`, and `issue`, an
issue labelled `limitation`). The pull request that fixes it deletes the
file and closes the issue. The build joins the files in name order (it fails
on a file whose `id` is not its name, or not lower-case letters, digits and
dashes); `oxoffice limitations` lists them, and
the release attaches the page built from the same files
(`scripts/limitations_doc.py`, which lint runs too). Nothing to regenerate:
`docs/LIMITATIONS.md` is not committed (`-o docs/LIMITATIONS.md` writes it
for a local look).

A branch that still edits the old `limitations.json` gets a modify/delete
conflict when it rebases: `python3 scripts/limitations_move.py REBASE_HEAD~1
REBASE_HEAD` replays that commit's entries as files (its docstring has the
merge variant).

## CI

- Pin every GitHub Action by its full commit SHA, with the tag as a comment:
  `uses: actions/checkout@<sha> # v7.0.1`. Never a bare tag or branch.
- Python tools used by CI live in `.github/requirements/*.in` and are
  compiled with hashes: `uv pip compile --universal --python-version 3.12
  --generate-hashes X.in -o X.txt`, installed with `pip install
  --require-hashes -r X.txt`.
- Which pull requests run what: one whose base is `main` and that is not a
  draft builds and tests on Linux, macOS and Windows and times the QUICK
  scaling sets on Linux, when it is opened, pushed, reopened or marked ready.
  A draft, or one whose base is another branch, runs lint, WebAssembly and
  the Linux build only. A push to `main` builds, tests and times every
  scaling call on Linux only: its pull request ran the three platforms. The
  nightly run (02:23 UTC) and `workflow_dispatch` run every job, every
  scaling call included, on the three platforms.
- The WebAssembly job also runs the PowerPoint integration tests without
  the `render` feature, as the WebAssembly bindings build the facade: an
  edit must not need the rendering crates.
- **Loop step 6 is the only macOS and Windows check before the nightly
  run.** `main` requires no status check, so nothing stops a merge whose
  macOS or Windows build is red or still queued; such a break shows in the
  nightly run, up to a day later, under the merges that came after. Merge
  only once `gh pr checks` shows the three platforms green.
- **Every push to `main` runs in full**, beside the runs of the pushes
  before it: nothing waits and nothing is cancelled. Lint, WebAssembly and
  the Linux build answer about 10 minutes after each merge, the scaling and
  skill-examples jobs within about 30, so each commit of `main` has its own
  verdict. Re-running a run of `main` touches no other run.
- **A red nightly run is a red `main`.** Look at it with
  `gh run list -w CI -e schedule -L 1`. Whoever sees it first opens an issue
  (or fixes it) the same day; a release waits for a green nightly run (see
  [RELEASE.md](RELEASE.md)). To find the commit, run `workflow_dispatch` on a
  branch at each candidate commit.
- **A change of base to `main` runs nothing**, whether GitHub moves a stacked
  pull request when its base merges or the author changes it by hand. Before
  merging, check the three platforms ran (loop step 6); if not, push a new
  commit, or close and reopen the pull request.
- The `scaling` job builds a second wheel with the `scaling` profile, beside
  the `ci` one, and runs the scaling tests: the QUICK sets on Linux for ready
  pull requests into `main`, every call on Linux on each push to `main`, and
  every call on the three platforms in the nightly run (02:23 UTC) or on
  `workflow_dispatch`. A platform-only growth shows in the nightly run.
- The `skill-examples` job runs the examples of
  [oxoffice-skills](https://github.com/oxoffice-dev/oxoffice-skills) on
  `main` only, against the Linux wheel of `build`
  ([tests/skills/README.md](../tests/skills/README.md)). A change that breaks
  one updates the skills too.
- **Rust caches are saved by runs on `main` only** (pushes, the nightly run,
  a dispatch on `main`), even when a test fails after the compile; pull
  requests restore them and save nothing. A new pull request used to save
  about 2.25 GB of caches that only its own later runs could read: five of
  them filled the 10 GB of the Actions cache and evicted `main`'s. A push to
  `main` builds on Linux only, so the macOS and Windows caches are refreshed
  by the nightly run. After a runner image change or a `Cargo.lock` change
  their key changes and pull requests build those platforms from scratch,
  about 7 minutes more, until the next night: run
  `gh workflow run ci.yml --ref main` to refresh them at once.
- **Lint checks the workspace without debug assertions** too (`cargo check
  --workspace --all-targets --all-features --profile scaling`), on every
  pull request, drafts and stacked ones included: every crate and target on
  Linux, all features on. Clippy and the WebAssembly check use the dev
  profile and the build job the `ci` one, all with debug assertions, so code
  that compiles only with them (an item under `#[cfg(debug_assertions)]`
  used or re-exported without the cfg) fails there first. It fails on
  errors only, for now. Code behind a feature turned off, or for Windows
  only, is still checked without debug assertions only by the nightly run.
- **Python tests run in parallel** in the build job (`pytest -n auto`,
  pytest-xdist, one worker per CPU). A test writes only under `tmp_path`
  (or `tmp_path_factory`). The scaling job stays sequential: its timings
  need a quiet runner.
- On macOS the build job sets `MACOSX_DEPLOYMENT_TARGET=11.0`, the value
  maturin builds the wheel with, meant to let `cargo test` share the units
  both builds have in common. On a warm run it did not: the workspace
  crates still compile twice (their features differ, and on macOS
  something not found yet), see
  [the spec's outcome](../specs/2026-11-21-faster-build-job.md#outcome).
- Dependabot opens the monthly updates. Review them like any change: the
  maintainer, or the agent that merges them, answers the review's findings.

## Documents and Office apps

- Never leave a document open in Word, PowerPoint or any GUI app. Use
  LibreOffice headless for checks. When a script drives Word or PowerPoint
  (to build the test corpus or check validity), it closes every document and
  quits the app at the end.
- Test files we commit are ours or carry a licence that allows it, and are
  listed in the corpus manifest.

## Structure

The code should stay easy to reason about from one file.

- Prefer a type that removes cases (a newtype for a unit, a type that makes an
  invalid state unrepresentable) over one that adds places to look.
- No new trait, generic parameter or feature flag without two users today.
- No wrapper that only forwards. Smallest diff that solves the problem.
- Match the conventions of the file you edit.
