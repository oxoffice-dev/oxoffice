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
  (and its proof for a minor), a follow-up issue for a minor, or why it is
  not a defect. A major finding is fixed unless the maintainer waives it.
- **At most two reviews per pull request**: one full review, then at most
  one delta review, after a major was fixed or after a change no finding
  asked for that can change behaviour (new code, a conflict resolved by hand
  in code, tests or workflows). A third review needs the maintainer's
  agreement; beyond that budget the maintainer decides from the evidence
  (mutation checks, `git range-diff`), without another round.
- **Minors never trigger a review.** Fix one with a proof stated in the
  answer (a test that fails without the fix, or a mutation check), or open a
  follow-up issue, or say why it is not a defect. A minor that would need a
  risky change goes to a follow-up issue, not into the pull request.
- **A rebase or merge that only resolves a conflict in data**
  (`limitations.json`, a generated page, the `GROWING` or test-module lists,
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

## Known limitations

A limitation users can hit, with a workaround, goes in
`crates/oxoffice-cli/limitations.json` with an issue labelled `limitation`,
then `python3 scripts/limitations_doc.py` regenerates
[LIMITATIONS.md](LIMITATIONS.md). The pull request that fixes it removes the
entry and closes the issue.

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
  the Linux build only. A push to `main` builds and tests on the three
  platforms and times every scaling call on Linux. The nightly run (02:23 UTC)
  and `workflow_dispatch` run every job, every scaling call included, on the
  three platforms.
- **A red nightly run is a red `main`.** Whoever sees it first opens an issue
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
