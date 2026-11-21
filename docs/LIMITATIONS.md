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
| replace | Text replacement through Python is slower than the CLI: about 1.5 times the time of the CLI for the same pairs | when only replacing, use `oxoffice docx replace FILE --map pairs.json` | [#5](https://github.com/oxoffice-dev/oxoffice/issues/5) |
| performance | Edits cost more as the document grows, most on files saved by Word or Google Docs: Run.text, Paragraph.text, add_run, Table.add_row, remove_content and notes cost the whole body: tens of milliseconds per call, up to seconds on long reports | batch the edits, or build the content aside and insert it once | [#6](https://github.com/oxoffice-dev/oxoffice/issues/6) |
| forms | Form operations cost seconds on long documents: about 0.1 s at 25 paragraphs, 3 to 9 s at 1,000 | pass every rule in one call | [#7](https://github.com/oxoffice-dev/oxoffice/issues/7) |
| replace | Text replacement does not see tabs and line breaks: a search holding a tab or a break finds nothing, one across a break moves it, and one across a tab inside a run panics in debug builds and garbles the run in release builds | replace the text on each side of the tab or break separately | [#26](https://github.com/oxoffice-dev/oxoffice/issues/26) |
| styles | Word files saved in another language: headings and tables of contents find no heading: insert_toc lists nothing, and the Rust headings() and document_outline() are empty, when Word wrote localised style ids such as Titre1 | call rebuild_toc() after insert_toc(); find headings through the styles named "heading N" | [#27](https://github.com/oxoffice-dev/oxoffice/issues/27) |
| headers | Document header and footer calls ignore the headers a section inherits: the Rust header_text() and footer_text() return None when the last section inherits its header, and set_header() changes the last section only | read and edit each section's header: Rust section_story(section, kind, variant), Python document.section(i).header | [#28](https://github.com/oxoffice-dev/oxoffice/issues/28) |
| lists | A numbered list cannot be restarted in a file saved by Word: restarting fails with "numbering restart cannot project unsupported override source" (Rust try_add_numbered_list_item_with_restart and restart_numbering, Python add_numbered_list_item(restart=True) and restart_numbering) | start a new list instead: a new numbering definition and instance on the new paragraphs (Rust add_list_definition; Python add_numbering_definition then add_numbering_instance and Paragraph.numbering) | [#29](https://github.com/oxoffice-dev/oxoffice/issues/29) |
| notes | Removing a paragraph leaves its notes behind, and removing a note leaves its custom mark: remove_content keeps the paragraph's footnotes and endnotes as unreferenced notes; removing a note made with a custom mark (Rust create_footnote_with_mark) keeps the mark | remove the paragraph's notes first (remove_footnote, remove_endnote, same names in Python); delete a leftover mark with replace_text | [#30](https://github.com/oxoffice-dev/oxoffice/issues/30) |
| compare | Comparison refuses a paragraph added after a final table: compare fails with "needs an adjacent paragraph for a final paragraph change" when the edited version adds a paragraph after a table that ends the body | put a paragraph after the final table in both versions before comparing | [#32](https://github.com/oxoffice-dev/oxoffice/issues/32) |
| compare | Comparison refuses an endnote added, removed or moved, or a change it writes as a deletion and an insertion around an endnote: compare refuses with "endnotes story shell count changed", "identical related-story shells" or "its reference moved to another site" (also when a table or paragraph holding an endnote is replaced whole) | make the endnote change on the accepted result after comparing, or compare that pair in Word | [#68](https://github.com/oxoffice-dev/oxoffice/issues/68) |

## pptx

| Area | Limitation | Workaround | Issue |
| --- | --- | --- | --- |
| open | A presentation part with a control character is refused at open: open fails with "PackageError ... holds U+0001" and `oxoffice pptx validate` exits 1, while PowerPoint opens the file | remove the character from that part's bytes, then open the file | [#8](https://github.com/oxoffice-dev/oxoffice/issues/8) |
