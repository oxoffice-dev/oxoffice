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
6. **CI all green**, squash merge: `gh pr checks` shows every check
   passed, `Test (ubuntu-latest)`, `(macos-latest)` and `(windows-latest)`
   among them. A pull request whose base was changed to `main` may have no
   run on that base: push a new commit, or close and reopen it, first. A
   documentation-only pull request runs no build, only the title check.
   The squash commit takes the title, so
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
- **The review is sized by risk.** A **light** review reads the diff and
  the spec, without building: for a change of about 150 lines or fewer
  outside tests away from the core edit path, documentation (links,
  anchors, text agrees with the code and the other docs), CI, tests-only
  changes and pure code moves (nothing but location changed). A **full** review may build, run the tests and mutation checks: for
  anything larger, and for any change to the core edit path (owner
  inventories and snapshots, splices and in-place edits, the redliner,
  comparison, identifier allocation, readers and writers).
- **One review per pull request.** A major is fixed with its proof: a test
  that fails without the fix (the mutation result in the answer), the
  crate's suite and clippy green, and the author's own reading of the fix
  diff, stated in the answer. Then it merges without another review. A
  **delta review** runs only when the fix is a redesign: more than about 150
  lines outside tests, or a change of behaviour no finding asked for (new
  code, a conflict resolved by hand in code, tests or workflows); it judges
  the delta only. A third review needs the maintainer's agreement; beyond
  that the maintainer decides from the evidence (mutation checks,
  `git range-diff`).
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

See [the spec](../specs/2026-11-21-independent-review-before-merge.md).

## Issues and fixes

Finish what you start.

- **A bug or a regression that comes back gets its test then, one at a
  time.** The suite was cut to the tests that run what the code needs
  ([a smaller suite](../specs/2026-11-22-a-smaller-test-suite.md)): a test
  is added for a defect seen, not ahead of one.
- **Fix the class, not the instance.** Before the review, look for the same
  defect in sibling places (the other attributes the same helper reads, the
  other setters with the same gap, the same escape in another reader, the
  other package) and fix them in the same pull request, each with its test.
- **A pull request closes its issue.** It says `Closes #N` when it fixes
  all of it. What a fix still leaves stays in the same issue: the pull
  request says `Refs #N`, the issue is updated with what remains, and
  closed when all of it is done. An issue kept open for leftovers carries
  the leftover's severity: change its label (often to S3 or S4) and put the
  remaining list at the top of its body, so the count of open S2 issues
  says what is really open. A minor review finding is fixed in the
  pull request when the fix is cheap, answered when it is not a defect, and
  becomes a new issue only when it is a different defect the pull request
  cannot carry: at most one per pull request. A major is fixed unless the
  maintainer waives it (see [Independent review](#independent-review)).
- **Fixed, not documented.** A case a real file or a known producer can hit
  is fixed; one no real file and no producer shows is refused with a plain
  message, and its issue is closed. A limitation is the exception (see
  [Known limitations](#known-limitations)).
- **One root cause, one issue.** When several issues share a cause (an
  escape, a measurement parser, a text decoder), open one issue for the
  cause, link the others to it, and fix the cause.
- **Every issue says how bad it is and who meets it**: a severity label
  (`S1 data loss`, `S2 wrong result`, `S3 rare or workaround`, `S4 polish`),
  an area label (`area: docx`, `area: pptx`, `area: render`,
  `area: python`, `area: ci`) and a "Seen with" line (which producer, which
  kind of file, which system). Search for an existing issue first. The bug report form
  (`.github/ISSUE_TEMPLATE/bug.yml`) asks for what happens, what it was seen
  with, what was expected and, if known, an issue with the same cause;
  labels are set at triage. An issue opened with `gh issue create
  --body-file` skips the form: give it the same `###` headings (What
  happens, Seen with, Expected, Same cause as).
- **Order of work**: S1 at once, S2 next, S3 and S4 in batches by module,
  one pull request per batch, never one pull request per trivial issue.
- **Test rounds are scheduled, not continuous.** A round that finds bugs
  faster than they are fixed waits for the fix queue; the workflows of real
  consumers decide what is tested first.
- **Measure**: issues opened and closed per hour, open issues by severity,
  follow-up issues per pull request (target: fewer than one in three).

## Conventional commits

```
<type>(<optional scope>): <lowercase subject>
```

Types: `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `build`, `ci`,
`chore`, `style`, `revert`. Scopes are free but usually one of `docx`, `pptx`,
`xlsx`, `cli`, `python`, `pdf`, `layout`, `opc`. A breaking change adds `!`
after the type and a `BREAKING CHANGE:` footer. The changelog groups commits
by type.

## Inner loop

```bash
cargo check -p <crate> --all-targets
cargo test -p <crate>                     # one crate
cargo test -p <crate> --test <file>       # one integration binary
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --workspace --all-targets --all-features --profile scaling -- -D warnings   # without debug assertions, as lint does
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
- Run the tests you touched locally (`cargo test -p <crate>` runs the unit
  tests in `src/` too). CI runs the unit tests (`--lib`), the integration
  tests (`--test '*'`) and the Python tests; the unit tests gated on debug
  assertions run in the full checks.
- Never `cargo clean` the shared target directory. Use one target
  directory per worktree.
- Rendering checks use the bundled fonts only (the `*_deterministic`
  helpers, `RenderOptions::with_bundled_fonts_only`, CLI
  `--bundled-fonts-only`, Python `bundled_fonts_only=True`). System fonts
  differ between machines: a test of the default render asserts what holds
  on any machine, and the system-font fallback is tested with fixed fonts
  (`crates/oxoffice-layout/tests/system_fallback.rs`).

## Scaling tests

`crates/oxoffice-py/tests/test_{docx,pptx}_scaling.py` time every public
mutating call in a document of N paragraphs and in one of 16N (a deck of N
slides and one of 4N, or 16N for the calls that insert or move a slide
before the others). The bodies of the Word documents carry the
identifiers Word writes on every paragraph, run and row, so that each call
takes the route a saved body takes. They need a build without debug assertions: debug builds
check every edit against a copy of the whole document, so every call grows
with it.

```bash
maturin build --profile scaling -m crates/oxoffice-py/Cargo.toml -o dist
pip install --force-reinstall dist/*.whl
OXOFFICE_SCALING=quick pytest crates/oxoffice-py/tests   # the QUICK sets (ci.yml)
OXOFFICE_SCALING=full pytest crates/oxoffice-py/tests    # every call (full-checks.yml)
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

**Deck text layout.** Tools check whether text fits its frame from
`Presentation::text_layout_deterministic`, at the frame's width and at 95 %
of it. Every frame of the training and pitch decks is pinned in
`crates/oxoffice-pptx/tests/snapshots/text-layout/` (lines, text height,
overflow, at both widths). A change that moves them rewrites those files
(`OXOFFICE_SNAPSHOTS_UPDATE=1 cargo test --profile ci -p oxoffice-pptx
--test pptx text_layout`) and its title says so, ending in `, deck text
layout moves`: the changelog prints the squash titles only, and it tells
the tools whose fit checks may change.

## Writings tests

The `equivalent_writings` modules of the docx and pptx test binaries write
each input again the way another producer may write the same XML (an empty
element with an end tag or self-closed, other prefixes, the default
namespace, indentation, attribute order, quotes and spacing, character
references), prove each rewrite says the same through a canonical form, and
check that open, validate, the text read, a save, an edit and (Word) a
comparison with accept and reject give what they give on the original (see
[the spec](../specs/2026-11-25-equivalent-xml-writings.md)). Pull requests
run a sample, `OXOFFICE_WORKFLOWS=full` every corpus copy but the long Word
ones and the thesis copies.
A difference known today is listed in `tests/corpus/writings-known.toml`,
strict like every known failure. Code that needs where an element's content
or end tag is reads it with `oxoffice_core::xml_element`, never by searching
for `</name>`, and where a tag or its attributes end with
`xml_element::start_tag`, never by taking `/>` as its close:
`crates/oxoffice-core/tests/xml_element.rs` refuses both in `crates/*/src`
(an element written `<x></x>` has no `/>`). A reader of a document part goes
through `oxoffice_core::xml_reader`, which reads `<x></x>` as `<x/>`; the
same file refuses a quick-xml reader in the crates that read them.
`--nocapture` prints every finding.

## Known limitations

Avoid them (maintainer decision, 2026-11-27):

- **A case a real file or a known producer can hit is fixed**, not
  documented (Word, LibreOffice, Google Docs and the other producers of
  `tests/corpus/README.md`).
- **A case never seen in a real file and written by no known producer is
  not documented.** The code may keep refusing it, with a plain error
  message that names no limitation id.
- **A limitation file stays only for a case that can happen and cannot be
  fixed now**; the pull request that adds or keeps one says why.

Such a case gets its own file, `crates/oxoffice-cli/limitations/<id>.json`
(one JSON object: `id`, the file name; `format`, `area`, `title`,
`symptom`, `workaround`, and `issue`, an issue labelled `limitation`). The
pull request that fixes it deletes the file and closes the issue. The build
joins the files in name order (it fails on a file whose `id` is not its
name, or not lower-case letters, digits and dashes); `oxoffice limitations`
lists them, and
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
- **One CI for pull requests and main** (`ci.yml`): every pull request,
  drafts and stacked ones included, and every push to `main` run the same
  jobs on Linux, macOS and Windows, in about 10 minutes: lint (with the
  WebAssembly check and the PowerPoint tests without the `render` feature:
  the WebAssembly bindings build the facade, so an edit must not need the
  rendering crates); `Test` on the three platforms, built without debug
  assertions (`--profile scaling`); `Timings` on Linux, the QUICK scaling
  sets. Debug assertions run in every local debug build and `cargo test`, in
  lint's pptx tests without `render`, and in the full checks (the writings
  walk apart). A push to `main` runs in full, beside the runs before it, so
  every merge has its own verdict. See
  [the spec](../specs/2026-11-22-one-ci-for-pull-requests-and-main.md).
- **The full sets run on demand** (`full-checks.yml`, "Full checks", about
  an hour, no schedule): on Linux, the Rust tests (four shards side by
  side; the writings walk without debug assertions) and the Python tests
  under debug assertions with the whole render corpus and every producer's
  copy in the workflow walks, and the stack of a release-profile wheel; every scaling call on Linux and Windows. The render
  is the same bytes on every platform and the walks are platform-free, so
  macOS and Windows run the tests in the CI of pull requests only.
- **The full checks do not grow.** Their budget is about an hour, the
  longest job; a set added to them takes the place of another or runs a
  sample, and the pull request that adds one says which, with the timings
  of a run on its branch.
- **When to run them**: before a release ([RELEASE.md](RELEASE.md)), and
  on a pull request whose change the CI of pull requests does not cover: a
  call that leaves or joins `GROWING`, a change to the corpus walks, a
  snapshot, the render. Run it on a branch or
  a tag with `gh workflow run full-checks.yml --ref BRANCH`; to find the
  commit that broke it, push a branch at each candidate commit and run it
  there. On `main` with `-f report=true`, a failure opens or comments one
  issue labelled `ci-full-checks`, closed by the next green reporting run;
  whoever sees it fixes it the same day. Another workflow can call it with
  `workflow_call`, granting `issues: write`.
- **Merge only once `gh pr checks` is all green**: `main` requires no status
  check, so nothing stops a merge whose macOS or Windows run is red or still
  queued.
- **A change of base to `main` runs nothing**, whether GitHub moves a stacked
  pull request when its base merges or the author changes it by hand. Push a
  new commit, or close and reopen the pull request, so that its last run is
  on its current base.
- The examples of
  [oxoffice-skills](https://github.com/oxoffice-dev/oxoffice-skills) are
  tested by that repository's CI, against the release the skills pin and
  every way they document to install it. To try a change here before it
  ships, run the example steps of its `tests/README.md` (not
  `check_install.py`, which checks the released packages) with a wheel
  built here installed in place of the pinned release. A change that breaks one says so in its pull
  request, and the skills follow at the next release.
- **Rust caches are saved by runs on `main` only** (pushes, a dispatch on
  `main`), even when a test fails after the compile; pull requests and the
  full checks restore them and save nothing. A new pull request used to save
  about 2.25 GB of caches that only its own later runs could read: five of
  them filled the 10 GB of the Actions cache and evicted `main`'s.
- **Lint runs clippy without debug assertions** too (`cargo clippy
  --workspace --all-targets --all-features --profile scaling -- -D
  warnings`), on every pull request, drafts and stacked ones included:
  every crate and target on Linux, all features on, warnings denied. The
  test jobs build without debug assertions but only their own targets and
  features: an item under `#[cfg(debug_assertions)]` used or re-exported
  without the cfg, or crate-private code dead without it, fails here.
- **Python tests run in parallel** (`pytest -n auto`, pytest-xdist, one
  worker per CPU). A test writes only under `tmp_path` (or
  `tmp_path_factory`). The scaling tests stay sequential: their timings need
  a quiet runner.
- On macOS the test jobs set `MACOSX_DEPLOYMENT_TARGET=11.0`, the value
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
