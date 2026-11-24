# oxoffice-docx-cli

This crate is the `oxoffice docx` command tree of the `oxoffice` binary,
built by the `oxoffice-cli` crate.

`oxoffice-docx-cli` turns DOCX files into inspectable and scriptable shell artifacts.
It extracts content, reports structure, validates packages, applies changes,
and produces fixed or flow output without an Office host.

## Capabilities

- Human-readable or JSON structure and metadata inspection.
- Accepted-view plain text or schema-1 rich extraction with typed nested paths,
  followed by the text boxes, headers, footers, notes, and comments.
- Deterministic point-space body layout fragments for shell automation.
- PDF, HTML, Markdown, PNG, JPEG, and multi-page TIFF conversion.
- Page-range rendering, guarded literal replacement, diffing, and validation
  verdicts that check every related part and every style id.
- Comment thread inspection and mutation with explicit body run ranges or an
  anchor text, and optional RFC 3339 dates.
- Tracked revision inspection, filtered resolution, table-of-contents rebuilds,
  and document comparison at run, word, or character granularity with ignore
  options.
- Bibliography sources in and out as JSON, the style, citations after a piece
  of text or at a run boundary, bibliographies, and the update of their results.
- Atomic form filling accepts repeatable `--tag` assignments or strict flat
  `--values` JSON over unique SDT tags/aliases and legacy names.
- Package-preserving edits cover the final S74 paragraph, run, typography,
  table, section, settings, field, form, equation, and drawing surface.


## Package signatures

```sh
oxoffice docx sign input.docx --key private-key.pem --cert certificate.pem -o signed.docx --json
oxoffice docx verify-signatures signed.docx --json
```

The key must be PKCS#8 RSA and the certificate X.509, as DER or one unencrypted
PEM block each, at most1MiB per input. Signing refuses existing destinations,
including aliases, and publishes only a valid complete candidate. Verification
returns schema1 JSON with status and signatures. Status is valid, invalid,
partial or unsigned. Exit0 requires at least one valid complete signature and
no invalid or incomplete report. Runtime and integrity failures exit1, and
usage errors exit2. Certificate-chain trust is not evaluated. The native signer
refuses already signed inputs and packages with external relationships.
Both commands return exit1 on report output errors, including a closed stdout.
Signing publishes before reporting, so this error can leave a valid new output.

## Use it when

Use this crate for shell automation. Use the
[`oxoffice-docx`](https://docs.rs/oxoffice-docx) library when these operations need to run
inside a Rust application.

## Relationship

The binary delegates document behavior to `oxoffice-docx` and shares path and JSON
conventions with `oxoffice-pptx-cli` through `oxoffice-cli-support`.

## Example

```sh
cargo install oxoffice-cli --version '^0.1.0'

oxoffice docx inspect report.docx
oxoffice docx text report.docx
oxoffice docx text report.docx --json
oxoffice docx count report.docx 'Acme Corp' --json
oxoffice docx layout report.docx --json
oxoffice docx replace template.docx -p TOKEN -v ready --expect 1 -o report.docx
oxoffice docx convert report.docx --to pdf -o report.pdf
oxoffice docx convert redline.docx --to pdf --revision-view tracked -o redline.pdf
oxoffice docx convert form.docx --to pdf --fillable-forms -o form.pdf
oxoffice docx render redline.docx --revision-view tracked --revision-style legal
oxoffice docx diff before.docx after.docx
oxoffice docx diff before.docx after.docx --exit-code --json
oxoffice docx validate report.docx
oxoffice docx render report.docx --page 0 -o rendered
oxoffice docx comment list report.docx --json
oxoffice docx comment add report.docx --start-paragraph 0 --start-run 0 \
  --end-paragraph 0 --end-run 1 --author Reviewer --text 'Check this' \
  --date 2026-09-13T12:00:00Z -o commented.docx
oxoffice docx comment add report.docx --anchor 'target words' --occurrence 1 \
  --author Reviewer --text 'Check this' -o commented.docx
oxoffice docx comment add report.docx --anchor 'Churn' --tables-only \
  --author Reviewer --text 'Check this figure' -o commented.docx
oxoffice docx revision accept reviewed.docx --author Reviewer -o accepted.docx
oxoffice docx compare original.docx edited.docx --author Reviewer \
  --timestamp 2026-09-13T12:00:00Z -o redline.docx
oxoffice docx compare original.docx edited.docx --author Reviewer \
  --timestamp 2026-09-13T12:00:00Z --granularity word --ignore-comments \
  --ignore-story header -o redline.docx
oxoffice docx toc rebuild report.docx -o refreshed.docx
```

Comment `add` ranges use zero-based body paragraph and run boundaries. The
start is inclusive and the end is exclusive. Run boundaries count the runs that
`text --json` lists, including the runs inside inline content controls and
tracked insertions. A range that cannot be anchored exactly, such as one that
crosses the edge of an inline content control, is refused. In place of the
four range flags, `--anchor TEXT` comments on the zero-based `--occurrence`
(default 0) of a literal, case-sensitive text of the main story, through body
paragraphs, tables and block content controls, and splits the runs at both
ends of the match. `--tables-only` counts only the matches in table cells,
nested tables included. A text that does not occur, an occurrence past the last
match, and a match that cannot be anchored exactly exit unsuccessfully without
creating the output. Comment replies,
resolution, and removal select a decimal comment id. Comment `add` and `reply`
date the comment with the current time, in UTC to the second.
`--date` writes an RFC 3339 timestamp instead, and `--undated` writes no date,
as Word leaves comments once its Document Inspector removes personal
information; the two conflict. An invalid timestamp exits unsuccessfully
without creating the output.

`compare` replaces each changed run whole by default (`--granularity run`),
like the Rust and Python APIs. `--granularity word` marks only the changed
words, and `--granularity character` marks only the changed characters.
`--ignore-formatting`, `--ignore-whitespace`, `--ignore-fields`,
and `--ignore-comments` keep the original side of those differences.
`--ignore-comments` keeps the original's comments and anchors and drops the
comments of the edited file. The repeatable `--ignore-story KIND` keeps one
story of the original, where `KIND` is a Python `Story.kind` name: `body`,
`header`, `footer`, `comment`, `text_box`, `footnote`, or `endnote`. Ignoring
the `comment` story excludes only the comments part, so a pair whose comments
differ needs `--ignore-comments`. An unknown granularity or story name is a
usage error, a repeated story fails, and neither creates the output. The
`--json` record states the options that ran. The original may already hold
tracked changes. A paragraph, table or block content control holding one stays
byte for byte when both inputs hold it unchanged. Editing, deleting, moving or
inserting such an owner is refused with its location, without publishing output.
Section properties holding revisions must also stay unchanged in both inputs.
Changing them fails without publishing output.
Bookmark ids, which Word renumbers on every save, and the `_Toc` names a
rebuilt table of contents gives its heading bookmarks are no change: a body
bookmark both files hold keeps the original's id, a heading paired with one of
the original's takes its `_Toc` name, a new heading takes a fresh one where the
original uses its own, and the links and page references of the edited file
follow them. Footnote and endnote ids, which Word also renumbers, take the
original's, the notes of each kind paired in reference order. A footnote or an
endnote added or removed is inserted or deleted whole with its reference, and
one whose reference moved is deleted with its old reference and inserted again,
under a new id, with the new one, as Word shows it.
No tracked change records a bookmark: a paragraph whose bookmarks differ is
deleted and inserted whole, the insertion first, and a bookmark both files hold
is written once, outside the revision marks, its start in the insertion and its
end in the deletion, as Word resolves them. Accepting leaves such an end just
after the inserted paragraph, and rejecting such a start at the restored
paragraph's start, over the same text. Such a paragraph is still refused when a
bookmark both files hold starts or ends inside its text, or sits under another
id or inside a hyperlink or control, or when it holds a complex field or a
comment range both files hold.
Reported counts include newly created revisions only, even when a new review
uses an existing author's name.

For an unsupported edit, retain the original redline separately. Resolve the
prior revisions on two independent input copies, then compare those resolved
copies. This removes the selected prior markup from the new baseline:

```bash
oxoffice docx revision accept original-redline.docx --author "Reviewer A" --output original-resolved.docx
oxoffice docx revision accept edited-redline.docx --author "Reviewer A" --output edited-resolved.docx
oxoffice docx compare original-resolved.docx edited-resolved.docx --author "Reviewer B" --timestamp 2026-10-27T13:00:00Z --output second-review.docx
```

Use the existing `--id` selector when the same author has several reviews.

Revision `list` reports every supported story and names the story of each
revision. Revision `accept` and `reject` operate across every supported story
and accept at most one selector: `--id`,
`--author`, or the paired `--start-date` and `--end-date` RFC 3339 bounds.
Omitting a selector resolves all modeled revisions. Every mutation, comparison,
and TOC rebuild requires `-o/--output`, publishes only a complete validated
DOCX, and supports a schema-1 record through `--json`. `compare` reports how
many revisions it created in each story while retaining the main-body count.
A `.docx`, `.docm`, `.dotx`, or `.dotm` output extension selects the package class the output
declares, so a template edited into `report.docx` is written as a document. An
input that carries a VBA project cannot change to a macro-free extension.

`text` prints each paragraph with the same accepted-view text as `text --json`:
tracked insertions and move destinations are included, and tracked deletions
and move sources are left out. Plain `text` covers body paragraphs and the
paragraphs inside content controls, table cells, and nested tables in document
order.

After the body, `text` prints every other story: text boxes, headers, footers,
footnotes, endnotes, and comments, part by part in the order of
`Document::stories`. Each package part starts with a line that names the kind of
its first story and the part, such as `--- header (/word/header1.xml) ---`. Each
direct paragraph or block content control of its stories follows on one line in
reading order (`Document::other_story_texts`): a table's cells where the table
sits, a text box after the paragraph that anchors it, the cells of a table in a
body text box included, with the accepted-view text of Python `StoryItem.text`.
A part without any text, such as an empty header variant, is left out. A text
box that Word writes twice, DrawingML in `mc:Choice` and a VML copy in
`mc:Fallback`, is never read from the copy. When a story part cannot be read,
such as a truncated header or a header relationship to a missing part, `text`
and the Markdown and HTML conversions keep the body, print one warning on stderr
that names the part when it can, list no other story, and exit successfully.
`text --json` then states `main` scope. `validate` reports that part as an
error.

`text --json` reports accepted-view paragraphs in source order. Each paragraph
has a zero-based `body_index`, a typed zero-based path within that body item,
its direct style and numbering, and accepted-view runs. Its `text` also holds
the text inside smart tags, inline custom XML elements, and simple fields,
which `runs` does not list. Run `formatting` is
`null` when no direct run properties exist. Otherwise it records nullable
direct bold, italic, strike, underline, font, point size, colour, highlight,
language, and character style values. The record has `all-supported-stories`
scope, and its `stories` array lists the other stories as plain `text` does,
one entry per story with its Python `Story.kind` name as `kind`, its
`part_name`, its `owner_index`, and `items` in story order. Each item is a
direct paragraph or block content control with its story `index_path`, its `kind`, its direct
`style` (always `null` for a content control), and its `text`. When the styles
cannot be read, `style` is `null` throughout and one warning goes to stderr.

`count` counts a literal, case-sensitive text in every paragraph of every
story, as `Document::count_text` does: the body with its table cells, text
boxes and block content controls, each header and footer part, the footnotes,
the endnotes and the comments, in the accepted view, a text box in a run inside
an inline smart tag or custom XML element included. It prints one
`kind: count` line per story kind that holds the text, in the order body,
header, footer, footnote, endnote, comment, then `total: N` and
`outside comments: N`, the count of what `replace` searches. With `--json`,
the record holds `text`, `revision_view`, `counts` (an object keyed by story
kind), `total` and `outside_comments`. Occurrences do not overlap and never cross the end of a
paragraph. A header or footer counts once per part, as `replace` replaces it
once per part, so an `outside comments` count above what `replace --expect`
finds names text that replacement cannot reach, such as a match across the
edge of a content control or a tracked insertion, or the paragraphs of a block
custom XML element. Comments are counted in `total` only: `replace` never
searches them. Part names are OPC names, with a leading slash
(`/word/header1.xml`). An empty text exits unsuccessfully.

`convert --to md` and `convert --to html` append the same stories after the
body, one section per part under a bold label. They leave comments out,
because comments annotate a document rather than belong to it.

`convert` to PDF or images and `render` use the accepted revision view by
default. `--revision-view tracked` shows both sides of tracked changes, and
`--revision-style legal` colours them as legal comparison tools do: insertions
blue with a double underline, deletions red and struck through, moved text
green. `--revision-style` needs `--revision-view tracked`.
Unknown view names are usage errors. HTML and Markdown conversion refuse
the tracked view before creating output.

`convert --to pdf --fillable-forms` exports the content controls and legacy
form fields of the body, its text boxes, the headers and footers and the notes as fillable PDF form fields: text, check boxes, lists and dates, with their
current values. A control tagged `signature` or `signature:<name>` becomes an
empty signature field. The flag needs `--to pdf` and the accepted view.
`oxoffice limitations` lists the kinds of controls it leaves static, and a
control of the body, a text box, a header, a footer or a note that would be a field but is left static says why on
standard error; a picture control without a signature tag or a group control
is no field, and stays static without a message.

`validate` exits unsuccessfully when a relationship of the main document points
at a missing part, when a part has no declared content type, when an XML part
that the main document relates to is not well formed, or when the main
document, a header, a footer, the notes, or the comments name a paragraph,
character, or table style id that no style defines. Word silently falls back
to the default style for such an id. A style id inside a tracked property
change is not checked, because it records the formatting before the change,
and neither is one inside `mc:Fallback`, which Word does not read, or an empty
id.
Empty paragraphs, heading level gaps, missing metadata, and paragraphs numbered
by a numbering instance or level the document does not define are warnings.

`layout --json` uses the fonts of a render: font families resolve in the
bundled fonts, and a system font only draws the characters they lack;
`--bundled-fonts-only` gives the same geometry on every machine and
`--system-fonts` resolves families in the system fonts first. It lists every direct body
item, including preserved items that have no fragments. Each laid-out fragment
uses points from the top-left page origin and records one-based physical and
displayed page numbers. A body item that crosses a page boundary has one
fragment on each occupied page.

`diff` compares the accepted-view paragraph text of every story: body
paragraphs, the paragraphs of the body's table cells and nested tables, text
boxes, the headers and footers of each section, footnotes, endnotes, and
comments. Each story is compared as one sequence through a shortest edit
script, found by Myers' linear-space algorithm, so a few edits in a long
document stay fast and memory stays proportional to the story length. Between
two matched paragraphs, removed and added paragraphs pair in order as changed
paragraphs. A changed paragraph prints a `-` line and a `+` line, an added one
only `+`, and a removed one only `-`. The summary line reads
`N paragraph(s) changed, A added, R removed.`, which replaces the
`N paragraph(s) differ.` line of earlier releases.

Each line locates its paragraph between brackets. A body paragraph keeps its
one-based position among the body paragraphs, such as `[2]`. A table cell
paragraph of the body is located as `[table 1, row 1, cell 2, paragraph 1]`,
where the table counts the tables placed directly in the body and the rest is
the `text --json` path made one-based. A header or footer is named by its type
and the first section that references it, as
`[header default, section 1, paragraph 1]` or
`[footer first, section 2, paragraph 1]`, and the two files pair their headers
and footers by that name. Inserting a new first section with its own header
therefore shows the old header as changed and itself as added under section 2.
Notes and comments count in their part, as `[footnote 1, paragraph 1]` or
`[comment 2, paragraph 1]`, and a text box of the body as
`[text box 1, paragraph 1]`. A table cell inside a header, a footer, or a
notes or comments part is labelled `table cell K` in that part, without a row
and cell path, as `[header default, section 1, table cell 3, paragraph 1]`.
Outside the body, `paragraph N` counts the paragraphs of its story and a block
content control reads as `content control N`, counted apart. A story that
cannot be read is printed as `(not compared: ...)` instead of being counted as
equal.

`diff --json` writes a schema-1 record with the counts, each difference with
its story kind, locations, and texts, and the stories not compared.
`--exit-code` exits with 1 when the compared text differs and 2 on an
error or an unreadable story, as `diff` and `cmp` do. An unreadable story
remains in `not_compared` even when another story differs. Without
`--exit-code`, `diff` exits with 0 after printing a partial comparison.

`replace --expect N` publishes only when the run-aware replacement count is
exactly `N`. A mismatch exits unsuccessfully without creating or replacing the
requested output.

A tab or a positional tab reads as a tab character, a non-breaking hyphen as
`-`, and a line, page or column break as a line feed, as `text` prints them: a
placeholder holding one matches it, and a value holding one writes a `w:tab`,
a plain hyphen or a line break (a carriage return or a Windows line ending
writes one line break too; a page or column break matched and written back
becomes a line break, a positional tab an ordinary tab, a non-breaking hyphen
a plain one). In a `--map` file write them `\t` and `\n`; on a shell command
line, pass the characters themselves (`$'a\tb'` in bash).

A literal `replace`, one pair or a `--map`, also refuses a pair after which a
paragraph that held the placeholder (outside the comments) does not read as its
text with every occurrence replaced: the occurrences it keeps are out of the
replacement's reach, such as a match across the edge of a content control, a
tracked change or a simple field, text in a text box inside a note, or the
paragraphs of a block custom XML element. The error names the pair, how many
occurrences are left, and the first paragraphs that hold them with their part
and style, and nothing is written. A value that forms the placeholder again
with its neighbours, as `"  "` to `" "` on three spaces, or a value equal to
the placeholder, is not refused.
Comments are not searched and do not refuse. A document whose stories the
check cannot read, such as one whose header relationship targets a missing
part, is refused with the reason. `--allow-unreached` skips the check and
publishes with the reachable occurrences replaced. `--regex` pairs are not
checked, and `--allow-unreached` conflicts with `--regex`. A table of contents
is reached like the body: its cached entries are replaced with their headings.

The count covers the text a reader sees in the body, tables,
content controls, headers, footers, footnotes, endnotes, and tracked insertions,
in the text boxes of the body, headers, and footers, in the labels of the
charts in the body, and inside smart tags, inline custom XML elements, and the
cached results of simple fields. Deleted text is not counted, except in a text
box inside a deleted run, which is replaced like any other text box. A text box
inside a note and the separators of the notes parts are not counted.

`convert` and `render` refuse an output file that already exists unless
`--force` is given. Even with `--force` they refuse their own input file under
any spelling of its path, and an output that is not a regular file, such as a
directory, a symbolic link, a FIFO, or a device like `/dev/null`. A run checks
every file it would write before it writes the first one, and publishes each
file only once it is complete, so a failed run leaves no truncated output.

`validate` exits unsuccessfully on a structural error: a relationship to a
missing part, a part without a content type, or a prefix that `mc:Ignorable`
or `mc:MustUnderstand` lists without a namespace declaration. Empty
paragraphs, heading level gaps, missing metadata, and a paragraph numbered by a
numbering instance or a level the document does not define (Word shows it
without a number) are warnings only.

The documented WordprocessingML subset also checks well-formed XML 1.0 in
XML parts, style ids, direct body/p/r children and order, text placement,
paragraph/table `jc` values, `color`, tracked-change `id` and `author`, drawing
extents, required `m:rad` children, relationship ids, note references and deleted
text. Relationship attributes use their actual namespace and owning part.
Repeated imported bookmark ids are warnings. This is not a full schema
validator. Both Word and PowerPoint opens refuse exact duplicate ZIP part names
and literal forbidden XML characters, naming the affected part. Validators
report the character fault without rewriting it to an OOXML escape.


Run `oxoffice docx --help` or `oxoffice docx <command> --help` for the complete option set.

## Ordered one-shot edits

`replace INPUT -p OLD -v NEW -o OUTPUT` retains the single-pair command.
`replace INPUT --map PAIRS.json -o OUTPUT --json` reads a UTF-8 JSON array:

```json
[
  {"placeholder": "A", "value": "B", "expect": 2},
  {"placeholder": "B", "value": "C", "expect": null}
]
```

Pairs run in array order, including repeated placeholders. Each later pair
searches the earlier result. An omitted or null `expect` is unrestricted.
An empty array succeeds with empty counts and publishes the unchanged document
to the required output. Unknown keys, wrong types, invalid replacement text,
negative or fractional counts and late expected-count mismatches refuse the
whole edit. Duplicate object keys follow serde_json's last occurrence wins
policy. `--map` cannot be combined with `-p`, `-v` or command-line `--expect`.

`--regex` uses native Rust regex syntax and capture substitutions such as `$1`.
Patterns match replaceable stretches of adjacent ordinary runs within one
paragraph. Wrapper, field-instruction, deleted-revision and paragraph boundaries
remain boundaries. Supported body, table, control, header, footer, note and
textbox stories participate. Chart labels are excluded. Zero-width matches are
skipped. Every pattern and replacement is validated before publishing one
candidate. JSON success carries schema1, operation, mode, ordered pairs with
index, placeholder, value, count and nullable expected count, total_count and
output. Errors go to stderr with a nonzero exit and no success JSON.

```bash
oxoffice docx fields update input.docx --now 2026-10-10T08:30:00 -o fields.docx --json
oxoffice docx fill input.docx --tag client=Ada --tag status=ready -o filled.docx --json
oxoffice docx meta get input.docx --json
oxoffice docx meta set input.docx --title Report --custom count=int:4 -o metadata.docx --json
oxoffice docx images extract input.docx images --json
```

`fields update` atomically combines ordinary and deterministic layout cache
updates through the native facade. Omitted `--now` stays unresolved, preserving
stored DATE/TIME caches and retention diagnostics. Explicit dates use checked
Gregorian wall-clock values without time-zone conversion. The record reports
ordinary and per-kind page-cache counts, complete ordered evaluation facts,
combined diagnostics, total_updated and saved_cache_scope. The selected input
basename and canonical full path supply FILENAME context. Dynamic header/footer
PAGE and NUMPAGES caches retain their provider policy. Unsupported switches do
not become successful updates.

`fill --tag NAME=VALUE` selects tags only, never aliases. Values retain every
character after the first `=`. Duplicate tags, a missing tag, invalid text or
an unsupported bound owner refuse the complete batch. Each matching control,
in the body, a header, a footer, a note, a comment or a text box, and its
supported custom XML binding changes together. A value replaces the control's
content as an edit made with track changes off: the tracked insertions,
deletions and moves in it, its paragraph marks' included, go with it, and a
change holding a comment or note mark, a content control, one end of a
bookmark or tracked range, or part of a field is refused (accept or reject it
first). Counts follow the supplied tag order.

`meta set` changes supplied core fields and preserves unspecified metadata.
An empty core string removes that value. Custom syntax is `NAME=TYPE:VALUE`:
`str`, signed32 `int`, finite64 `float`, `bool` with true/false, `filetime` with
a checked Gregorian RFC3339 value and explicit timezone, or `empty` with no
payload. Strings retain additional `:` and `=` characters. Existing property
PID, fmtid, position and string subtype remain preserved. Duplicate custom
names and set/remove collisions refuse. `--remove-custom NAME` is a no-op when
absent. Reads include immutable typed property identity and preserved raw data.

`inspect --json` retains existing schema1 keys and adds word, Unicode scalar
character and deterministic page counts, their accepted-main-body scope and
layout diagnostics. Spaces contribute to character_count. Pictures come from
the existing physical occurrence provider, including supported related story
owners. Occurrences retain story identity, index path, ordinal, relationship,
EMU dimensions, description, title and decorative state.

`images extract` writes each unique internal media part once, in first
occurrence order. Generated flat names such as `image-000001.png` use a bounded
content-type mapping, with `bin` for an unknown image type. Shared media has one
file and multiple occurrence records. Distinct parts remain distinct even when
bytes are equal. Embedded bytes are preserved exactly. External linked images
are listed with status external, URL metadata and file null, without fetching.
Their native media_part_name is null and bytes/content_type are empty.
Unsupported physical ownership and missing internal media refuse through the
checked provider. Linked occurrence resizing retains its existing refusal.

Extraction rejects symlink directories and existing file collisions and has no
`--force`. It preloads and stages the complete output set before publishing.
Recoverable publication failure removes this invocation's files and a newly
created directory only if empty. Existing unrelated files remain. This is
recoverable rollback, not crash-atomic multi-file replacement. Empty extraction
creates no directory. JSON is printed only after publication.

Every mutation requires an output and refuses existing outputs, input identity,
symlink aliases and hard links. Native package-class, VBA and signature checks
remain authoritative. No success report precedes publication.

## Bibliography

```bash
oxoffice docx bibliography add-source input.docx --source sources.json -o 1.docx
oxoffice docx bibliography set-style 1.docx --style apa6 -o 2.docx
oxoffice docx bibliography cite 2.docx --after "as shown" --source "Smith20,pages=12-14" -o 3.docx
oxoffice docx bibliography insert 3.docx --index 9 --style apa6 -o 4.docx
oxoffice docx bibliography update 4.docx --default-locale 1033 -o 5.docx --json
oxoffice docx bibliography sources 5.docx --json
```

`sources --json` lists each source as `{tag, guid, source_type, source,
diagnostics}`; `source` is null when the source cannot be edited as a whole.
`add-source` and `replace-source` take that `source` object, or an array of
them: `{"tag", "kind", "guid", "locale", "contributors": [{"role", "people":
[{"last", "first", "middle"}]} | {"role", "corporate"}], "fields": [[field,
value], ...]}`. A name part is a string or an array of strings; `fields` keeps
its order and may repeat a field. A source with no `guid` gets one derived from
its content; `replace-source` keeps the GUID of the source with that tag.

Styles, kinds, roles and fields are snake-case names (`apa6`,
`harvard_anglia`, `journal_article`, `book_author`, `reference_order`). Flags
also take them in kebab case. A `cite --source` value is a tag followed by
switches after commas: `pages=`, `volume=`, `prefix=`, `suffix=`, `no-author`,
`no-year`, `no-title`. A tag or a value holding a comma goes in `--sources
CITE.json`, an array of `{"tag", "pages", "volume", "prefix", "suffix",
"suppress_author", "suppress_year", "suppress_title"}`. `cite --after TEXT`
finds the text as `comment add --anchor` does; `--paragraph N --run R` counts
as `text --json` does. `update` fails as a whole, writing nothing, when a field
is not covered yet, and needs `--default-locale` when a field and its sources
name no locale.

## Licensed font embedding

Use caller-owned font files only after confirming their embedding license.
Embedding requires explicit authorization and exact license identity. Commands
write a new output and refuse input/font aliases or existing destinations.

```bash
oxoffice docx font embed input.docx --name "My Face" --font ./MyFace-Regular.ttf \
  --license-authorized --license-identity "SIL Open Font License 1.1" \
  --output embedded.docx
oxoffice docx font list embedded.docx --json
oxoffice docx font settings embedded.docx --embed-true-type-fonts false --output disabled.docx
oxoffice docx font remove embedded.docx --name "My Face" --kind regular --output removed.docx
```

A name several records carry (LibreOffice writes `Symbol` twice) is refused;
`--index N`, the record's `index` in `font list --json`, addresses one of
them in `font embed` and `font remove`.

The four CLI kinds are regular, bold, italic and bold-italic. Settings accept
true, false or none, with none removing the owned value. Listing reports actual
native part, format and byte-length facts in the schema1 JSON envelope.
Word also accepts --font-key and --subsetted, and its settings include
--embed-system-fonts and --save-subset-fonts.
