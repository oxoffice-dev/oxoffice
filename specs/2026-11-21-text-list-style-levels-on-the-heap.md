---
date: 2026-11-21
status: accepted
scope: perf
---

# Text list style levels live on the heap

## Context

The 0.1.0 release wheel needs about 820 KiB of stack to open a deck (Linux
x86_64, measured with `ulimit -s` and a symbol build): 38 frames, a few of
them very large, `resolve_master_records` 165 KB, the binding's
`Presentation.__new__` 148 KB, `from_fragment` 136 KB. Python's own main
thread has room (8 MiB on Linux and macOS, 2,000,000 bytes under Windows'
`python.exe`), but a Windows program that embeds Python usually keeps the
linker's 1 MiB, and #15 sets a 1 MiB budget for the release wheels.

The frames are large because the values they hold are: a
`CT_TextListStyle` carries ten `CT_TextParagraphProperties` of 2.7 KB inline,
27 KB in all. A slide master has three of them (82 KB), the presentation and
the notes master one each (28 KB), so does every text body that has a list
style, and a frame that moves one keeps a copy or two.

## Decision

The ten levels of `CT_TextListStyle` (`default_paragraph_properties`,
`level1` to `level9`) become `Option<Box<CT_TextParagraphProperties>>`. The
type shrinks to 200 bytes, a slide master to 2 KB, `Presentation` from 29 KB
to 2.6 KB. `level(n)` still returns `Option<&CT_TextParagraphProperties>`.

A test in the pptx test binary keeps these sizes under a few KB, so that the
cause comes back as a failing test rather than as a crash in a host program.

Not done here: `CT_TableStyle` (about 112 KB in the frames that render a
table) is the next large value, in a pull request of its own. Running the
bindings on a thread with a larger stack would hide the cause and cost a
thread per call.

## Plan

1. Box the levels, adapt the few callers, add the size test. Measure the
   stack of a release wheel before and after.
2. The same for `CT_TableStyle`, measured the same way.

## Links

- #15, and #20 for the 1 MiB stack budget the release wheels must fit in
