# W10 from the UI: a reply typed in Word, then saved

Word for Mac 16.115.2 (French UI), 2026-11-28. A script made the document: one paragraph, a root comment "root"
(07:04 local, UTC+1). Then a person clicked Reply, typed "reply", was asked to click Resolve, and pressed Cmd+S
(07:15). The parts are byte for byte what Word saved, except `people.xml`, where `w15:userId` is replaced by
`0000000000000000`.

RESULTS: the reply is `w:comment w:id="1"` with two paragraphs: "reply" (`3685F8A6`) and an empty one
(`10DFE630`). Its `paraId` in `commentsExtended`/`commentsIds` is the last one, `10DFE630`, with
`w15:paraIdParent="634FC803"` (the root's). Its `durableId` is `74E94554`. `w16cex:dateUtc="2026-11-28T06:15:00Z"`
is the true UTC; `w:date` is the local time with `Z`, and both have the seconds zeroed. `w15:done="0"` on the root and
on the reply: the saved file records no resolve. `people.xml` keeps its one `w15:person` (the same account, Windows
Live), and no entry was added. The reply's range starts right after the root's `commentRangeStart`; its
`commentRangeEnd` comes after the root's reference run, with its own reference run. Its paragraphs carry no
`pStyle` and have `w14:textId="77777777"`.
