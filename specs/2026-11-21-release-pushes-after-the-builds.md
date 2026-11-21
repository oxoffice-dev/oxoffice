---
date: 2026-11-21
status: shipped
scope: release
---

# The release pushes main and the tag after every build is green

## Context

`release.yml` committed the version and pushed it to `main` with the tag
`vX.Y.Z` in its first job, before any build. A build or smoke check that
failed later (the run takes about an hour) left a tag and a version commit
on `main` with nothing published, to delete and revert by hand. The
`dry_run` mode existed to find such failures first.

## Decision

The maintainer's three constraints (2026-11-21): nothing reaches `main` or
the tags before every build, smoke check and asset is green; no merge freeze
during the run; no dry-run mode. The shape follows the release workflow of
kaya-go/kaya, the maintainer's other project.

- A `check` job stops at once on a bad version, a run not dispatched from
  `main`, or an existing tag.
- Every build job (CLI, wheels, sdist) checks out the dispatched commit and
  runs the same version step in its workspace: `scripts/set_version.py`,
  then `cargo update --workspace`. One commit everywhere, as #175 asked,
  without a commit or branch to carry it. The smoke jobs take the wheel
  and the dispatched commit's script; a new `conda-check` job builds and
  tests each conda package into a local channel, the part of the dry run
  worth keeping, since the conda job otherwise runs only after the tag.
- One `release` job, after all of them, checks out the dispatched commit:
  version step, changelog, commit `chore(release): vX.Y.Z` on top of it,
  annotated tag on that commit, so the tag is exactly the built tree plus the
  version and the changelog. Then the assets (installers, limitations page,
  SBOM, SHA256SUMS, provenance) and the release notes, and only then one
  atomic push of `main` and the tag, retried if `main` races. Only the
  GitHub release comes after the push; PyPI and conda follow the tag.
- `main` gets the version commit by a fast-forward when it did not move,
  otherwise by a merge commit `chore(release): merge vX.Y.Z into main`
  (parents: current `main`, the version commit). Pull requests merged during
  the run are not in the release (maintainer's decision), and the tag stays
  reachable from `main`. Merge conflicts can only touch the three files the
  version commit changes: `CHANGELOG.md` takes the release's, `Cargo.toml`
  and `Cargo.lock` take `main`'s, and the version step runs again on the
  merge (a crate added during the run gets the version too).
- A re-run of the `release` job finds the tag on a `chore(release)` commit
  whose parent is the dispatched commit and goes on; a tag on any other
  commit stops it.
- `dry_run` is removed (the input, its conditions, the local-channel branch
  of the conda job): a failed run now publishes nothing.
- Top-level permissions drop to `contents: read`; only the `release` job
  writes.

Rejected: kaya's `git pull --rebase` before tagging (the tag would hold
merges nobody built), a cherry-pick of the version commit onto `main` (the
tag would be a side commit; git-cliff only sees tags reachable from `HEAD`,
so the next changelog would repeat the release), a temporary branch, a merge
freeze. git-cliff 2.14.2 (the pinned one) was checked on a scratch history
with such a merge commit: the commits merged during the run land in the next
release, in the full changelog and in `--latest`.

Not done: a `bump-dev-version` job as kaya has. Cargo and the version script
take `0.2.1-dev`, but in 0.x the next version is often a minor, not a patch,
nothing here tells development builds apart by version, and it would be one
more push to `main` after publication.

## Plan

One pull request: `release.yml`, `docs/RELEASE.md`. Checked by actionlint
and by running the `release` job's shell steps against a scratch bare
repository: `main` not moved (fast-forward), `main` moved (merge commit, tag
on the built tree), `main` moved with conflicts in `Cargo.toml` and
`Cargo.lock` and a new crate (resolved, versions set), a refused push
(retried), a re-run after the push (goes on), a tag on another commit
(stops).

## Links

- #175 (one commit in every build job), specs/2026-11-20-ci-and-release.md
- https://github.com/kaya-go/kaya/blob/main/.github/workflows/release.yml
