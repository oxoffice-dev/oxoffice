---
date: 2026-11-23
status: accepted
scope: docx
---

# Date controls with a time, and the US Gregorian calendar

## Context

A date content control shows its date in its `w:dateFormat` (#450). What is
left of #149:

- A format can show a time (`M/d/yyyy h:mm am/pm`). Word lets one type a
  time into such a control and saves it in `w:fullDate`
  (`2015-01-02T15:05:00Z`). oxoffice writes midnight only: `validate_form`
  reports such a control as "unsupported date metadata", and no setter can
  give it a time.
- Word's calendar list has `gregorianUs` (Gregorian, US English): the
  Gregorian calendar with English names; it is refused as an "unsupported date calendar".
- The rest is refused on purpose, see Decision.

## Decision

- **A value may carry a time** when the control's format shows one:
  `YYYY-MM-DDTHH:MM` or `YYYY-MM-DDTHH:MM:SS` (24-hour, no zone), wherever a
  date value is taken: the tag setters and `fill_form`.
  The control shows it in its format (`h hh H HH m mm s ss`, the day period
  `AM`/`PM` in United States English), `w:fullDate` gets
  `YYYY-MM-DDTHH:MM:SSZ`, as Word writes it, and a binding stores what
  `w:storeMappedDataAs` asks (the shown text, the date, or the full value).
  A plain date still shows midnight. A time given to a control whose format
  shows none is refused (the control could not show it), and so are hours,
  minutes or seconds the format does not show: they would be saved unseen.
- **`validate_form` checks a saved time** as it checks a date: the shown
  text and the binding must be what that time gives in the format. A time in
  `w:fullDate` of a control whose format shows none stays a wrong value.
- **`gregorianUs` is the Gregorian calendar in US English**: its month and
  day names are English whatever `w:lid` says (Windows' calendar data); the
  day period stays the language's (Windows' locale data).
- **Refused on purpose**, the named limitation `docx-date-control-names`
  (issue #149 is closed with this list): month and day names in a language
  other than English, French, German, Spanish, Italian, Portuguese and Dutch
  (Windows locale data, genitive and partitive month forms), short names
  outside English (they differ between Windows and Office versions), a day
  period outside United States English (designators differ by region), the
  other calendars (Hijri, Umm al-Qura, Hebrew, Saka, Japanese era, Thai,
  Korean and Taiwanese years, the Arabic, Middle East French and
  transliterated Gregorian ones), tokens Word's list does not offer.

## Plan

One pull request: the formatter takes a time; `prepare_text_value` and
`set_value_metadata` (oxml) take a date and time; the facade's
`validate_form` passes the saved time on. Tests: formatter cases, a time set
by tag and by `fill_form`, a time refused on a date-only format, a time
typed in Word validated, a `gregorianUs` control set and validated.

## Links

- Issue #149, pull request #450 (formats and names).
