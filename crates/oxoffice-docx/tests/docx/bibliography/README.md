# Bibliography oracle

`word/<style>.docx` are the probes of the `bibliography_probe` example, updated and saved by Word for Mac 16.115:
every source kind with the fields Word's dialog offers, book variants (2, 3, 6 and 8 authors, a corporate author, no
author, no year), the citation switches and one bibliography, all in en-US. `bibliography::oracle_tests` compares
each citation and entry with ours, text and formatting, and lists in `KNOWN` the fields that still differ.

Only APA is recorded. Word for Mac renders a probe in the style set in Word, not in the probe's own, and no automatic
route sets it, so the other 11 styles are listed in `WORD_PENDING`: their results are not checked against Word yet.
To record one: set the style in Word, run `update_in_word.py` (it says how), keep the file of that style in `word/`,
and take the style out of `WORD_PENDING`.
