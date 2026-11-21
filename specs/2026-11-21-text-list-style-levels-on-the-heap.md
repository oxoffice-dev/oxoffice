---
date: 2026-11-21
status: shipped
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

## Outcome

- Shipped as planned in #47 (`perf(drawing)!`, a breaking change for Rust
  code that builds or reads the levels: `Box::new`, `as_deref()`). One caller
  in `oxoffice-pptx` changed; Python and the CLI did not.
- Sizes: `CT_TextListStyle` 27,000 to 200 bytes, `CT_SlideMaster` 82,408 to
  2,008, `CT_NotesMaster` 28,032 to 1,232, `CT_Presentation` 27,624 to 824,
  `Presentation` 29,376 to 2,576 (`size_of` on Linux x86_64).
  `crates/oxoffice-pptx/tests/pptx/stack.rs` keeps them small.
- Measured locally on Linux x86_64 release wheels, with and without the
  change and nothing else: opening a deck went from about 820 to 190 KiB of
  stack, the smoke work of #20 from 816 to 384 KiB.
- The release dry runs of #20 give the smoke work on every platform
  (smallest thread stack, on the 8-file corpus of the time), before on 0.1.0
  (44843492727) and after on `main` with #47 and the merges of the night
  (44864662766): Windows x64 784 (up to 832 with Windows' 64 KiB rounding)
  to 448 KiB, Linux x86_64 816 to 416, Linux aarch64 816 to 416, macOS arm64
  800 to 400, macOS x86_64 816 to 400. The local comparison above is the one
  that isolates #47.
- The largest frames left are in rendering a slide with a table:
  `resolve_table` and `CT_TableStyle::from_xml`, about 112 KB each. Plan
  step 2 is #160.

## Links

- #15, and #20 for the 1 MiB stack budget the release wheels must fit in
- #47, the change; #160, table styles
