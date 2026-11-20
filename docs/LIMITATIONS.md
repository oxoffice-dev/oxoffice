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

## pptx

| Area | Limitation | Workaround | Issue |
| --- | --- | --- | --- |
| open | A presentation part with a control character is refused at open: open fails with "PackageError ... holds U+0001" and `oxoffice pptx validate` exits 1, while PowerPoint opens the file | remove the character from that part's bytes, then open the file | [#8](https://github.com/oxoffice-dev/oxoffice/issues/8) |
