# oxoffice-numfmt

`oxoffice-numfmt` turns a spreadsheet number format code and a cell value into
the text Excel displays.

## Capabilities

- Codes as stored in files (ECMA-376 Part 1, 18.8.31): up to four sections,
  conditions (`[>=100]`), colours (`[Red]`, `[Color10]`), currency and locale
  tags (`[$€-40C]`), quoted literals, `\x`, `_x`, `*x`, `@` and `General`.
- Numbers: `0 # ?`, thousands separators, scaling commas, `%`, exponents
  (engineering included) and fractions, rounded as Excel rounds them.
- Dates and times in the 1900 and 1904 systems, elapsed `[h]`, `[m]`, `[s]`,
  fractions of a second and `AM/PM`.
- Built-in format ids and `is_date_format`.
- The colour of the section, the fill character and the `_x` spaces, for a
  render.

## Use it when

Use this crate to show a workbook value as Excel shows it: a text or CSV
export, a render, a chart axis or label. It depends on no format family.

## Example

```rust
use oxoffice_numfmt::{FormatOptions, NumberFormat};

let format = NumberFormat::parse("[$-409]mmmm d, yyyy")?;
let shown = format.format_number(45306.0, &FormatOptions::default());
assert_eq!(shown.text, "January 15, 2024");
# Ok::<(), oxoffice_numfmt::Error>(())
```

The text is en-US (decimal point, comma separator, English names); the
options carry the locale for others later. See
`specs/2026-11-27-number-format-codes.md`.
