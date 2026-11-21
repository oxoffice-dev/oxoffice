# Test corpus

Realistic Word and PowerPoint files saved by the editors people actually use,
for the tests that read real documents (round trips, edits, performance). See
[the test suite spec](../../specs/2026-11-20-test-suite-from-scratch.md) and
[the corpus spec](../../specs/2026-11-21-corpus-at-scale-and-behaviour-tests.md).

- `files/authored/`: one source per family (a report, a contract, a thesis, a
  pitch deck...), written by [`generate/author.py`](generate/author.py) with
  python-docx and python-pptx. The families live in
  [`docx_families.py`](generate/docx_families.py) and
  [`pptx_families.py`](generate/pptx_families.py), the raw OOXML the libraries
  lack in [`ooxml.py`](generate/ooxml.py).
- `files/word/`, `files/powerpoint/`, `files/libreoffice/`, `files/google/`:
  the same sources opened and saved by each editor, written by
  [`generate/produce.py`](generate/produce.py).
- [`manifest.toml`](manifest.toml): one entry per file, written by
  [`generate/manifest.py`](generate/manifest.py) from the files themselves:
  producer, size, and the features found in the XML. Only `known_failure`
  and `rewritten` are written by hand: the issue that makes a save without an
  edit rewrite some parts of the file, and those parts. The round-trip test
  then expects exactly those parts to change, and nothing worse.

## Rules

- Only files we made, or files whose licence allows redistribution, with the
  licence in the manifest.
- At most 1 MiB a file and 20 MiB in all, checked by the docx corpus test, so
  clones and CI stay quick.
- Sources are deterministic: fixed dates and seeds, zip entries with fixed
  timestamps. Rerunning `author.py` on unchanged code changes no byte.
- Producers that drive an app close every document after its save and quit
  the app, even when a file fails.

## Regenerate

```bash
# Sources, then the manifest (any machine)
uv run --no-project --with python-docx==1.2.0 --with python-pptx==1.0.2 \
    --with pillow==13.0.0 --with XlsxWriter==3.3.3 tests/corpus/generate/author.py [family ...]

# LibreOffice (any machine with soffice) and Google (an rclone remote, OXOFFICE_CORPUS_RCLONE, default gdrive:)
python3 tests/corpus/generate/produce.py libreoffice google [--only family ...]

# Word and PowerPoint, on a Mac with Microsoft Office
uv run --no-project --python 3.12 tests/corpus/generate/produce.py word powerpoint [--only family ...]

# The manifest alone, or a check that it is up to date and within budget
python3 tests/corpus/generate/manifest.py [--check]
```

Every producer rewrites the manifest with its version at the end of its run,
so a run only needs a commit.
