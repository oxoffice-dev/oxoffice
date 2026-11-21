# Known limitations

What this version of oxoffice cannot do yet, and what to do instead. Generated from
[`crates/oxoffice-cli/limitations.json`](../crates/oxoffice-cli/limitations.json) by
`scripts/limitations_doc.py`. The installed build gives the same list:

```bash
oxoffice limitations [docx|pptx] [--json]
python -c "import oxoffice; print(oxoffice.limitations('docx'))"
```

## docx

| Area | Limitation | Workaround | Issue |
| --- | --- | --- | --- |
| compare | Comparison refuses to insert or delete a paragraph holding a simple field: compare refuses with "cannot track a whole paragraph's simple field outside a field it replaces", so a heading added under a table of contents made by insert_toc cannot be compared | add or remove the paragraph by hand on the redline | [#66](https://github.com/oxoffice-dev/oxoffice/issues/66) |
| compare | Comparison refuses two versions where a footnote reference moved: compare refuses and writes nothing ("its reference moved to another site") | compare that pair in Word | [#4](https://github.com/oxoffice-dev/oxoffice/issues/4) |
| compare | Comparison refuses a paragraph moved after a final table, or added or deleted after a moved or redlined final table: compare refuses with "needs an adjacent paragraph for a final paragraph move" or "... for a final paragraph change" (after a table moved to the end), or "holds an existing tracked revision" (after a table that holds tracked changes) | put a paragraph after the table in both versions, or resolve the earlier tracked changes on both copies first | [#120](https://github.com/oxoffice-dev/oxoffice/issues/120) |
| replace | Text replacement through Python is slower than the CLI: about 1.5 times the time of the CLI for the same pairs | when only replacing, use `oxoffice docx replace FILE --map pairs.json` | [#5](https://github.com/oxoffice-dev/oxoffice/issues/5) |
| performance | Edits cost more as the document grows, most on files saved by Word or Google Docs: Run.text, Paragraph.text, add_run, Table.add_row, remove_content and notes cost the whole body: tens of milliseconds per call, up to seconds on long reports | batch the edits, or build the content aside and insert it once | [#6](https://github.com/oxoffice-dev/oxoffice/issues/6) |
| forms | Form operations cost seconds on long documents: about 0.1 s at 25 paragraphs, 3 to 9 s at 1,000 | pass every rule in one call | [#7](https://github.com/oxoffice-dev/oxoffice/issues/7) |
| text | Non-breaking hyphens and positional tabs are missing from paragraph text: Paragraph text and text search skip w:noBreakHyphen and w:ptab, so "co-op" written with a non-breaking hyphen reads "coop", a search for "co-op" finds nothing, and replacing "coop" leaves the hyphen after the replacement | search for the text on each side of the hyphen or positional tab | [#26](https://github.com/oxoffice-dev/oxoffice/issues/26) |
| styles | Word files saved in another language: headings and tables of contents find no heading: insert_toc lists nothing, and the Rust headings() and document_outline() are empty, when Word wrote localised style ids such as Titre1 | call rebuild_toc() after insert_toc(); find headings through the styles named "heading N" | [#27](https://github.com/oxoffice-dev/oxoffice/issues/27) |
| headers | Document header and footer calls ignore the headers a section inherits: the Rust header_text() and footer_text() return None when the last section inherits its header, and set_header() changes the last section only | read and edit each section's header: Rust section_story(section, kind, variant), Python document.section(i).header | [#28](https://github.com/oxoffice-dev/oxoffice/issues/28) |
| notes | Removing a paragraph leaves its notes behind, and removing a note leaves its custom mark: remove_content keeps the paragraph's footnotes and endnotes as unreferenced notes; removing a note made with a custom mark (Rust create_footnote_with_mark) keeps the mark | remove the paragraph's notes first (remove_footnote, remove_endnote, same names in Python); delete a leftover mark with replace_text | [#30](https://github.com/oxoffice-dev/oxoffice/issues/30) |
| lists | A list item numbered through its style cannot be restarted: restart_numbering fails with "paragraph has no explicit numbering level" on the List Number paragraphs of Word and python-docx files, whose numbering comes from the style | Rust: give the paragraph its own numbering first (set_numbering with the num_id and level of effective_paragraph_properties), then restart; Python: start a new list (add_numbering_definition, add_numbering_instance, Paragraph.numbering) | [#82](https://github.com/oxoffice-dev/oxoffice/issues/82) |

## pptx

| Area | Limitation | Workaround | Issue |
| --- | --- | --- | --- |
| open | A presentation part with a control character is refused at open: open fails with "PackageError ... holds U+0001" and `oxoffice pptx validate` exits 1, while PowerPoint opens the file | remove the character from that part's bytes, then open the file | [#8](https://github.com/oxoffice-dev/oxoffice/issues/8) |
