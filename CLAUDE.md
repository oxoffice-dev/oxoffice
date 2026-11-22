# CLAUDE.md

oxoffice: Word and PowerPoint files (spreadsheets planned) in pure Rust, with
a Rust API, one `oxoffice` CLI and one `oxoffice` Python package as equals.
A friendly fork of rdocx, see [NOTICE](NOTICE).

## Read first

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): crates, dependency rule,
  model conventions, what is deliberately wrong.
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): the loop, commits, commands.
- [specs/README.md](specs/README.md): when and how to write a spec.
- Roadmap: the issues labelled
  [`roadmap`](https://github.com/oxoffice-dev/oxoffice/issues?q=is%3Aissue%20state%3Aopen%20label%3Aroadmap),
  one per workstream; the principles are in
  [README.md](README.md#-how-the-library-is-meant-to-be-used).

## Rules

1. **Spec first** for any non-trivial change. `docs/` is a snapshot of today,
   `specs/` the timestamped log. Update both proactively.
2. **Conventional commits**, lowercase subject. Never commit or push unless
   asked. No `Co-Authored-By` or other attribution lines.
3. **Rust facade first**, then the CLI where it makes sense, then Python. A
   feature is done when it reaches all three.
4. **Tests come with the change.** Name a test after the behaviour, never
   after a ticket. Run what you touched locally, CI runs the rest.
5. **Exclude the Python crates from workspace `cargo test`**, they cannot link
   a test binary.
6. **Never open documents in Word, PowerPoint or another GUI app** and leave
   them open. LibreOffice headless for checks. Scripts that drive Word or
   PowerPoint close every document and quit.
7. **Keep unmodelled XML verbatim and child order exact.** See the model
   conventions in the architecture doc.
8. **Known limitations** live in `crates/oxoffice-cli/limitations/`, one
   JSON file per limitation named by its id (shown by `oxoffice
   limitations`). Add one with its issue, remove it with the fix. The
   release builds the page; `docs/LIMITATIONS.md` is not committed.
9. **An independent review before every merge**, posted on the pull request;
   every finding answered first (fixed, or why it is not a defect; a major
   is fixed unless the maintainer waives it). At most two reviews: one
   full, one delta after a major fix. A minor is fixed in the pull request
   when the fix is cheap, with its proof (a test that fails without it, or
   a mutation check), and not re-reviewed; a follow-up issue only for a
   real defect the pull request cannot carry, at most one per pull request,
   grouping its findings. See
   [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#independent-review).
10. **Issues and fixes: finish what you start.** Fix the class, not the
    instance; a pull request closes its issue, and what it leaves stays
    in that issue; one root cause, one issue. Every issue has a severity
    (`S1`-`S4`), an area and a "Seen with" line. See
    [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#issues-and-fixes).

## Commands

```bash
cargo check -p <crate> --all-targets
cargo test -p <crate>
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --workspace --all-targets --all-features --profile scaling -- -D warnings
cargo fmt --all
cargo build -p oxoffice-cli
maturin develop -m crates/oxoffice-py/Cargo.toml
```
