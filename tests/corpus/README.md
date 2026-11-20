# Test corpus

Realistic Word and PowerPoint files saved by the editors people actually use,
for the tests that read real documents (round trips, edits, performance). See
[the test suite spec](../../specs/2026-11-20-test-suite-from-scratch.md).

- `files/authored/`: the source documents, written by
  [`generate/author.py`](generate/author.py) with python-docx and python-pptx.
- `files/word/`, `files/powerpoint/`, `files/libreoffice/`, `files/google/`:
  the same documents opened and saved by each editor, written by
  [`generate/produce.py`](generate/produce.py).
- [`manifest.toml`](manifest.toml): one entry per file, with its producer and
  the features it carries. Add the entry with the file.

## Rules

- Only files we made, or files whose licence allows redistribution, with the
  licence in the manifest.
- About 20 MB in total and 1 MB per file, so clones and CI stay quick.
- Producing needs a Mac with Word and PowerPoint, LibreOffice, and an rclone
  remote for Google Drive. The script closes every document and quits Word
  and PowerPoint when it is done.

## Regenerate

```bash
uv run --with python-docx==1.2.0 --with python-pptx==1.0.2 --with pillow==13.0.0 tests/corpus/generate/author.py
python3 tests/corpus/generate/produce.py            # or name the producers: word powerpoint libreoffice google
```
