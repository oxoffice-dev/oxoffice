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
5. **Independent review**, posted on the pull request, then fix the findings
   (see [Independent review](#independent-review)).
6. **CI green**, squash merge. The squash commit takes the title, so
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

- The reviewer itself posts its findings on the pull request, in one comment
  headed `🔍 Independent review` with the head it read: major and minor, each
  with `file:line`, the defect and a failure scenario, or that it found
  nothing. The pull request keeps the trace.
- The author fixes them, or answers in the thread why one is not a defect. A
  major finding is fixed unless the maintainer waives it. A substantial fix
  gets a new review of what changed.
- It looks for wrong behaviour and missed cases first, then tests that would
  pass with the feature broken, Rust, CLI and Python disagreeing, unsafe CI
  changes. Documentation and pure code moves get a light review.

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
mutating call in a document of N paragraphs (or slides) and in one of 4N. They
need a build without debug assertions: debug builds check every edit against
a copy of the whole document, so every call grows with it.

```bash
maturin build --profile scaling -m crates/oxoffice-py/Cargo.toml -o dist
pip install --force-reinstall dist/*.whl
OXOFFICE_SCALING=quick pytest crates/oxoffice-py/tests   # the QUICK sets
OXOFFICE_SCALING=full pytest crates/oxoffice-py/tests    # every call
```

Without `OXOFFICE_SCALING`, only the check that every mutating call is timed
runs. A docx call of `GROWING` is a strict xfail tied to its issue: when a fix
makes it flat, the XPASS fails the run until the call leaves the set.

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
- The `scaling` job builds a second wheel with the `scaling` profile, beside
  the `ci` one, and runs the scaling tests: the QUICK sets on pull requests,
  every call on `main`.
- Dependabot opens the monthly updates. Review them like any change.

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
