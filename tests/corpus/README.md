# Test corpus

Realistic Word, PowerPoint and Excel files saved by the editors people
actually use, for the tests that read real documents (round trips, edits,
performance). See
[the test suite spec](../../specs/2026-11-20-test-suite-from-scratch.md) and
[the corpus spec](../../specs/2026-11-21-corpus-at-scale-and-behaviour-tests.md).

- `files/authored/`: one source per family (a report, a contract, a thesis, a
  pitch deck, a purchase tracker...), written by
  [`generate/author.py`](generate/author.py) with python-docx, python-pptx and
  XlsxWriter. The families live in
  [`docx_families.py`](generate/docx_families.py),
  [`pptx_families.py`](generate/pptx_families.py) and
  [`xlsx_families.py`](generate/xlsx_families.py), the raw OOXML the libraries
  lack in [`ooxml.py`](generate/ooxml.py) (Word) and at the top of
  `xlsx_families.py` (a shared formula).
- `files/word/`, `files/powerpoint/`, `files/excel/`, `files/libreoffice/`,
  `files/google/`: the same sources opened and saved by each editor, written
  by [`generate/produce.py`](generate/produce.py), except `word/toc.docx`,
  made by hand in Word (see
  [A table of contents inserted in Word](#a-table-of-contents-inserted-in-word)),
  the Google Sheets copies of the workbooks, which are not made by a script
  here (see [Spreadsheets](#spreadsheets)), and the two workbooks with
  comment threads, made by hand.
- `files/google-docs/`: documents written in Google Docs itself, through the
  Docs API, by [`generate/google_docs.py`](generate/google_docs.py) from the requests of
  [`generate/google_docs.json`](generate/google_docs.json): `review`
  (headings, lists, links to headings, chips, a footnote, a header and a
  footer, a table, a page break, then suggestions) and `comments` (a source
  with comments in every anchor shape, converted, then suggestions over
  them), and `toc` and `reply`, made from them by hand in the browser, and
  `office-comment` and its Word copy, made by hand from a `.docx` edited
  without conversion. See [Written in Google Docs](#written-in-google-docs).
- [`manifest.toml`](manifest.toml): one entry per file, written by
  [`generate/manifest.py`](generate/manifest.py) from the files themselves:
  producer, size, and the features found in the XML. Only `known_failure`
  and `rewritten` are written by hand: the issue that makes a save without an
  edit rewrite some parts of the file, and those parts. The round-trip test
  then expects exactly those parts to change, and nothing worse. The files
  are Word, PowerPoint and Excel files (`.docx`, `.pptx`, `.xlsx`); the
  Google Sheets copies, which no script of ours writes, get the producer
  `Google Sheets export`.

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

# LibreOffice (any machine with soffice) and Google Docs and Slides (an rclone remote,
# OXOFFICE_CORPUS_RCLONE, default gdrive:); Google Sheets is not in this route
python3 tests/corpus/generate/produce.py libreoffice google [--only family ...]

# Word, PowerPoint and Excel, on a Mac with Microsoft Office
uv run --no-project --python 3.12 tests/corpus/generate/produce.py word powerpoint excel [--only family ...]

# The manifest alone, or a check that it is up to date and within budget
python3 tests/corpus/generate/manifest.py [--check]
```

Every producer rewrites the manifest with its version at the end of its run,
so a run only needs a commit.

## Spreadsheets

Four families, written by `xlsx_families.py` with XlsxWriter 3.3.3, each
feature once. Every source is saved by LibreOffice (`files/libreoffice/`) and
Excel for Mac (`files/excel/`, the `excel` route of `produce.py`, which opens
each file, saves it as Open XML, closes it and quits like the Word and
PowerPoint routes), and by Google Sheets (`files/google/`).

| family | carries |
| --- | --- |
| `review` | the review pass of tensorbee/rdocx#281: a purchase tracker of 36 rows (text, numbers, dates, a status), its amount column written as one shared formula (`F2:F37`), a total row, three legacy notes, a link with a tooltip |
| `budget` | styles and layout over three sheets: fonts, fills, borders, alignment and wrap, merged cells, a global and a sheet-scoped name, validations (a list, a whole number range), conditional formats (cell value, colour scale, data bar), frozen panes, an auto-filter with an active filter and its hidden rows, widths, heights, a hidden column, print setup (landscape, fit to one page wide, area, titles, header and footer), a picture and a chart; a ledger table with a calculated column and a total row; a gallery of 25 number formats, built in and custom, one value each |
| `multi` | five sheets (one hidden), formulas across sheets, a legacy and a dynamic array formula, the seven error values, booleans, a formula that returns a string, rich text, a 3,000 character text of ten paragraphs, accents, CJK, Arabic with a right-to-left reading order, an emoji, an internal link |
| `dates1904` | dates, a time, a date and time and a difference in the 1904 date system |

What to know when reading them:

- Every formula carries its cached value, computed by the generator.
  XlsxWriter marks every workbook `fullCalcOnLoad`, so Excel and LibreOffice
  recalculate on open and the copies hold their producer's values (LibreOffice
  caches `#VALUE!` for `SQRT(-1)` and `#NAME?` for `#REF!`: read cached values
  of error formulas from the sources and from Excel's copies).
- XlsxWriter writes every formula alone: `review` is rewritten after the build
  into the shared group Excel writes for a column filled down. LibreOffice
  writes every formula alone again, and drops the dynamic array flag of
  `multi` (a plain array formula) and the calculated column of the ledger
  table; Google Sheets groups per-cell formulas into shared ones itself.
- Google Sheets pads every sheet with empty rows up to row 1000 (about 40 KB
  of XML a sheet, small once zipped), writes an empty persons part and an empty
  drawing.
- Excel for Mac writes personal values into every workbook it saves: the
  folder it saved from (`x15ac:absPath` in `xl/workbook.xml`), the name of
  whoever saved it (`cp:lastModifiedBy`) and the task pane add-ins it carries
  (`xl/webextensions/`). The `excel` route makes each copy neutral right after
  its save (`neutral_excel` in `produce.py`: the url of `absPath` becomes
  `/Users/user/`, `lastModifiedBy` `Corpus Author`, the add-in parts go with
  their relationships and content types, no other byte changes), and
  `python3 tests/corpus/generate/produce.py --neutral-excel <file.xlsx>` does
  the same to a workbook made by hand in Excel (and replaces the account of its
  persons part, see below).
- No file has inline strings (the sources, LibreOffice and Google Sheets write
  shared strings only): a test that reads them builds its own workbook.

### Google Sheets copies

`files/google/*.xlsx` are uploaded from `files/authored/` through the Google
Drive connector with conversion to Google Sheets, then exported as xlsx; the
manifest gives them the producer `Google Sheets export`. `produce.py google`
leaves the workbooks alone (its rclone route only converts documents and
decks).

### Comment threads, made by hand

Notes are in `review` (XlsxWriter writes them as Excel does); threaded
comments need Excel 365 or a browser. Two files, made from `review` and made
neutral afterwards: `produce.py --neutral-excel` for the Excel one (the
neutral step of the route above, and the signed-in account of its persons part,
its comments and its core properties become `Corpus Author`), `python3
tests/corpus/generate/neutral_xlsx.py <file> ["<account name>"]` for the Google
one (the same replacement, with the account name when Google wrote it outside
the persons part):

- `excel/threads.xlsx`: Excel's copy `excel/review.xlsx`, saved under that
  name, with the same texts typed in Excel 365 for Mac (Review, New Comment)
  on the Orders sheet: on `C3` a comment and two replies ("The dates should
  follow the delivery calendar.", "Agreed, I will align them with the
  supplier.", "Done, see the updated list."), on `F10` a comment and one
  reply ("The total looks too high.", "It includes the freight.") then the
  thread resolved (Resolve thread), on `B6` a comment without a reply ("Is
  this supplier still approved?"), and the three notes of `review` left as
  they are (`E5`, `G9`, `A14`). Saved as .xlsx, the document closed, Excel
  quit, `produce.py --neutral-excel` run on it; then the manifest given Excel's version:
  `python3 -c 'import manifest, os; manifest.update({("excel", "xlsx"): "Microsoft Excel for Mac <version>"}, since=os.path.getmtime("../files/excel/threads.xlsx"))'`
  from `tests/corpus/generate`. Excel 365 stores the threads in
  `xl/threadedComments/` with `xl/persons/person.xml`, and each thread as a
  placeholder note in the legacy comments part; the Mac run replaces this
  sentence with what the file shows.
- `google/threads.xlsx`: the Google Sheets conversion of `review`, the same
  threads written in the browser (comments, replies, Resolve on the second
  thread), then File, Download, Microsoft Excel (.xlsx) and neutral. Its
  producer is the default one, `Google Sheets export`. What Google writes for
  threads and for notes is whatever the file shows: the manifest features
  (`threaded-comments`, `comment-reply`, `resolved-comment`, `notes`) say it.

## A table of contents inserted in Word

A table of contents inserted by Word (its own building block, not the field
of a source) needs Word on a Mac and a hand in its window: open a source
with headings, References, Table of Contents, Automatic Table 1, save as
.docx under `files/word/`, close the document, quit Word, then give it
Word's version, from `tests/corpus/generate`:
`python3 -c 'import manifest, os; manifest.update({("word", "docx"): "Microsoft Word for Mac <version>"}, since=os.path.getmtime("../files/word/<file>.docx"))'`.
Word's AppleScript reaches the building blocks of its loaded templates
only, and the gallery's Automatic Table 1 is in none of them (`make new
table of contents` would write the TOC field alone, without the block
control around it). Later fixes to the file can be scripted: open it,
`update (table of contents 1 of doc)`, save, close, quit.

- `word/toc.docx`: the manual with Automatic Table 1 inserted before its
  first heading by Word for Mac 16.115.2 in French, then the whole table
  updated, so its page numbers are Word's: a block control of the "Table of
  Contents" gallery (`w:docPartObj`, `w:docPartUnique`) holding a heading
  and the `TOC \o "1-3" \h \z \u` field, whose entries are links to `_Toc`
  bookmarks Word added on the 11 headings; every style id rewritten in
  French by Word (`Titre1` for `Heading1`, `TM1`, `En-ttedetabledesmatires`,
  `Lienhypertexte`).

## Written in Google Docs

```bash
# An access token with the Drive and Docs scopes; the run replays the requests
# of generate/google_docs.json in a temporary folder of that Drive, exports,
# deletes the folder and rewrites the manifest.
OXOFFICE_GOOGLE_TOKEN=$(gcloud auth print-access-token) python3 tests/corpus/generate/google_docs.py
```

Author names are replaced after export: Google writes the Drive account's
name on every suggestion, and the files carry `Corpus Author` instead
(`neutral_authors`, which changes those attributes only); the account's
numeric id and Drive file ids in Google's round-trip data are replaced too
(`neutral_custom_data`, same lengths).

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

None of these files holds the inline controls tagged `goog_rdk_<n>` that
documents edited in the browser carry (#656): suggestions, person and date
chips, a drop-down chip, links to headings, a table of contents inserted in
Google Docs, a reply written there, and comments in a run, across cells,
rows, a table's edge or a link's edge do not make Google write them in a
converted document, nor do the `google/` copies' converted comments and Word
contents. Google writes them when it exports a `.docx` edited without
conversion, and Word leaves them empty, `<w:sdtContent/>`, when it deletes
the comment inside. Two files show it, made by hand from a python-docx
source (a heading, three paragraphs, a two by two table), each download
made neutral with `python3 tests/corpus/generate/google_docs.py --neutral
<file.docx> "<account name>"` (author names, and the ids in Google's
round-trip data):

- `office-comment.docx`: the source uploaded to Drive, opened in Google Docs
  without conversion, a comment added on a word, then File, Download,
  Microsoft Word: the comment's `w:commentRangeStart` alone in an inline
  control tagged `goog_rdk_0`, and a `customXML` part with Google's
  round-trip data. The `.docx` kept in Drive holds neither: Google keeps the
  comments of such a file in Drive, and deleting or resolving them there
  leaves no control in the next download.
- `office-comment-deleted-in-word.docx`: that download opened in Word for
  Mac 16.115.2 (French UI), its comment deleted, saved without embedding
  fonts (Google's copy asks for them, 5 MB; AppleScript `save as ... embed truetype fonts
  false`): the control stays, `<w:sdtEndPr/>` and an
  empty `<w:sdtContent/>`. Word's user name in `cp:lastModifiedBy` is
  replaced too.
