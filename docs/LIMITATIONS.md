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
| compare | Comparison refuses two versions whose TOC heading bookmarks were renumbered: compare refuses with "duplicate bookmark id N in authored XML" or "cannot revise paragraph boundary structures" | remove the _Toc bookmarks from both copies, compare, then rebuild the table of contents on the result | [#3](https://github.com/oxoffice-dev/oxoffice/issues/3) |
| compare | Comparison refuses two versions where a footnote reference moved: compare refuses and writes nothing ("its reference moved to another site") | compare that pair in Word | [#4](https://github.com/oxoffice-dev/oxoffice/issues/4) |
| replace | Text replacement through Python is slower than the CLI: about 1.5 times the time of the CLI for the same pairs | when only replacing, use `oxoffice docx replace FILE --map pairs.json` | [#5](https://github.com/oxoffice-dev/oxoffice/issues/5) |
| performance | Edits cost more as the document grows, most on files saved by Word or Google Docs: Run.text, Paragraph.text, add_run, Table.add_row, remove_content and notes cost the whole body: tens of milliseconds per call, up to seconds on long reports | batch the edits, or build the content aside and insert it once | [#6](https://github.com/oxoffice-dev/oxoffice/issues/6) |
| forms | Form operations cost seconds on long documents: about 0.1 s at 25 paragraphs, 3 to 9 s at 1,000 | pass every rule in one call | [#7](https://github.com/oxoffice-dev/oxoffice/issues/7) |
| replace | Text replacement does not see tabs and line breaks: a search holding a tab or a break finds nothing, one across a break moves it, and one across a tab inside a run panics in debug builds and garbles the run in release builds | replace the text on each side of the tab or break separately | [#26](https://github.com/oxoffice-dev/oxoffice/issues/26) |
| styles | Word files saved in another language: headings and tables of contents find no heading: headings() and document_outline() are empty and insert_toc lists nothing when Word wrote localised style ids such as Titre1 | call rebuild_toc() after insert_toc(); find headings through the styles named "heading N" | [#27](https://github.com/oxoffice-dev/oxoffice/issues/27) |
| headers | Document header and footer calls ignore the headers a section inherits: header_text() and footer_text() return None when the last section inherits its header, and set_header() changes the last section only | read and edit each section's header through section_story(section, kind, variant) | [#28](https://github.com/oxoffice-dev/oxoffice/issues/28) |
| lists | A numbered list cannot be restarted in a file saved by Word: try_add_numbered_list_item_with_restart and restart_numbering fail with "numbering restart cannot project unsupported override source" | start a new list with add_list_definition(&[ListLevel::decimal()]) and number its paragraphs with it | [#29](https://github.com/oxoffice-dev/oxoffice/issues/29) |
| notes | Removing a paragraph leaves its notes behind, and removing a note leaves its custom mark: remove_content keeps the paragraph's footnotes as unreferenced notes; remove_footnote keeps the mark of a note made with create_footnote_with_mark | remove the paragraph's notes with remove_footnote or remove_endnote first; delete a leftover mark with replace_text | [#30](https://github.com/oxoffice-dev/oxoffice/issues/30) |
| compare | Comparison refuses a paragraph added after a final table, and edits of LibreOffice files with empty runs: compare fails with "needs an adjacent paragraph for a final paragraph change" or "acceptance does not reproduce the edited stories" | put a paragraph after the final table in both versions; for a LibreOffice file, add and remove a paragraph, save and reopen both copies, then compare | [#32](https://github.com/oxoffice-dev/oxoffice/issues/32) |

## pptx

| Area | Limitation | Workaround | Issue |
| --- | --- | --- | --- |
| open | A presentation part with a control character is refused at open: open fails with "PackageError ... holds U+0001" and `oxoffice pptx validate` exits 1, while PowerPoint opens the file | remove the character from that part's bytes, then open the file | [#8](https://github.com/oxoffice-dev/oxoffice/issues/8) |
