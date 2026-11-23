# Noto Sans Math subset

Source: `ofl/notosansmath/NotoSansMath-Regular.ttf` from the Google Fonts
`main` branch at `06bfe0d62a98375cb09c942fa18e68bd32cff057`, retrieved
2026-11-23.

Source SHA-256:
`3f495fe933c06786e4d5f6d86b8ee70b6753a68ee3b9d87528726de0f6e2c47d`

Output SHA-256:
`e09d629faa723d736d884164e38c31ac45523af1b451b10046b346221a61f08e`

The repertoire is seven Unicode blocks the bundled text faces cover only in
part, such as `∼` (U+223C) and `⊤` (U+22A4) in an equation: Letterlike Symbols
(U+2100-214F), Arrows (U+2190-21FF), Mathematical Operators (U+2200-22FF),
Miscellaneous Technical (U+2300-23FF), Miscellaneous Mathematical Symbols-A
(U+27C0-27EF), Miscellaneous Mathematical Symbols-B (U+2980-29FF) and
Supplemental Mathematical Operators (U+2A00-2AFF): 995 characters, about
104 KB. The face is a glyph fallback only, so the MATH table and the layout
tables are dropped; the equation layout reads its MATH constants from the
math font. Reproduce it with FontTools 4.68.0:

```text
pyftsubset NotoSansMath-Regular.ttf --output-file=NotoSansMath-operators-subset.ttf --unicodes=U+2100-214F,U+2190-21FF,U+2200-22FF,U+2300-23FF,U+27C0-27EF,U+2980-29FF,U+2A00-2AFF --glyph-names --symbol-cmap --legacy-cmap --notdef-glyph --notdef-outline --recommended-glyphs --name-IDs=* --name-legacy --name-languages=* --layout-features= --drop-tables+=MATH,GSUB,GPOS,GDEF --no-hinting
```
