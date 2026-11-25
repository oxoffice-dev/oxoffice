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
- `files/google-docs/`: documents written in Google Docs itself, through the
  Docs API, by [`generate/google_docs.py`](generate/google_docs.py): `review`
  (headings, lists, links to headings, chips, a footnote, a header and a
  footer, a table, a page break, then suggestions) and `comments` (a source
  with comments in every anchor shape, converted, then suggestions over
  them), and `toc` and `reply`, made from them by hand in the browser. See
  [Written in Google Docs](#written-in-google-docs).
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
  timestamps, the pilot's chart read from `generate/chart.png`. Rerunning
  `author.py` on unchanged code changes no byte on the same machine (another
  machine may compress the other generated pictures differently).
- The report and the deck are the pilot: the behaviour tests pin facts of
  every producer's copy of them, so `produce.py` saves them again only when
  `--only` names them, and the tests then need a look.
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

## Written in Google Docs

```bash
# An access token with the Drive and Docs scopes; the run writes in a
# temporary folder of that Drive, exports, deletes the folder and rewrites the manifest.
OXOFFICE_GOOGLE_TOKEN=$(gcloud auth print-access-token) python3 tests/corpus/generate/google_docs.py
# The requests of one step for a document read by hand (documents.get), to replay with another client
python3 tests/corpus/generate/google_docs.py --plan review fill document.json
```

Author names are replaced after export: Google writes the Drive account's
name on every suggestion, and the files carry `Corpus Author` instead
(`neutral_authors`, which changes those attributes only).

What Google writes from them, and the `google/` copies do not hold:
suggestions as `w:ins` and `w:del` whose runs carry a `w:rPrChange`, a
style suggestion as `w:pPrChange` with a paragraph mark `w:rPrChange`, two
paragraphs joined as a deleted paragraph mark over an empty run in a
`w:del`, an added paragraph as an inserted mark; a bookmark
`_<heading id>` with `w:colFirst` and `w:colLast` on every heading, and a
link to a heading as `w:hyperlink w:anchor` to it; a person chip as a
`mailto:` link and a date chip as plain text; comment marks moved inside the
`w:ins` and `w:del` around them; an unused default namespace on the root
(`.../office/tasks/2019/documenttasks`).

What the Docs API cannot write: comments and replies (the `comments` family
uploads its comments in a converted source; the API has no request for them,
and the Drive comments API needs a client of its own), a table of contents,
bookmarks, equations, drop-down chips. Two files were made by hand in the
browser from the generated ones, then exported; the generator cannot write
them again:

- `toc.docx`: the review with a table of contents inserted in Google Docs (a
  block control of the "Table of Contents" gallery) and a drop-down chip (an
  inline control with a `w:dropDownList`);
- `reply.docx`: the comments with a reply written in Google Docs to one of
  them (`w15:paraIdParent` in `commentsExtended.xml`).

None of these files holds the empty inline controls tagged `goog_rdk_<n>`
written `<w:sdtContent/>` that documents edited in the browser carry
(#656): suggestions, person and date chips, a drop-down chip, links to
headings, a table of contents inserted in Google Docs, a reply written there,
and comments in a run, across cells, rows, a table's edge or a link's edge
do not make Google write them, nor do the `google/` copies' converted
comments and Word contents. What does is unknown, so that shape is tested on
synthetic documents.

A table of contents inserted by Word (its own building block, not the field
of a source) needs Word on a Mac: open a source with headings, References,
Table of Contents, Automatic Table 1, save as .docx under `files/word/`,
close the document, quit Word, then give it Word's version, from
`tests/corpus/generate`:
`python3 -c 'import manifest, os; manifest.update({("word", "docx"): "Microsoft Word for Mac <version>"}, since=os.path.getmtime("../files/word/<file>.docx"))'`.
Scripted, the same is
`make new table of contents at active document with properties {text object:(create range active document start 0 end 0), use heading styles:true, upper heading level:1, lower heading level:3}`
in Word's AppleScript, not tried yet.
