//! The values that opening and editing a deck move by value stay a few KB.
//!
//! A release wheel opens a deck in a few dozen frames, and each frame holds
//! some of these values, sometimes several copies: when a text list style
//! carried its ten levels inline, a slide master was 82 KB and opening a deck
//! needed about 820 KiB of stack, most of the 1 MiB a Windows program that
//! embeds Python gives its main thread. The release workflow is to measure
//! the stack of each wheel it builds (#15); this test catches the cause in
//! every pull request. Rendering a slide with a table holds its table style
//! in a few frames: with its thirteen regions inline, a style was 55 KB.
