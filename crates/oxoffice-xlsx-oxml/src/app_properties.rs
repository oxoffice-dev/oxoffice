//! `docProps/app.xml` after a change of the sheet list.
//!
//! The extended properties name the sheets (`HeadingPairs` counts them by
//! kind, `TitlesOfParts` lists their names). Nothing reads them to open a
//! workbook and Excel writes them again at its next save, but a list that
//! lacks a new sheet says something false, so a change of the sheet list
//! drops both, as the pptx facade does after a slide list change. The rest
//! of the part stays as written.

use oxoffice_core::OxmlError;

use crate::xml::{Child, Splice, XmlSource};

/// The elements of `app.xml` that list the sheets.
const TITLE_LISTS: [&str; 2] = ["HeadingPairs", "TitlesOfParts"];

/// `xml`, an `app.xml` part, without its sheet lists; the same bytes when
/// it has none.
pub fn without_title_lists(xml: &[u8]) -> Result<Vec<u8>, OxmlError> {
    let mut source = XmlSource::new(xml);
    let root = source.root()?;
    let mut splice = Splice::default();
    if !root.empty {
        loop {
            match source.next_child()? {
                Child::Element(child) => {
                    let list = TITLE_LISTS.iter().any(|name| child.is(name));
                    let span = source.finish(&child)?;
                    if list {
                        splice.replace(span.range(), Vec::new());
                    }
                }
                Child::Raw(_) => {}
                Child::End(_) => break,
            }
        }
    }
    Ok(splice.apply(xml))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sheet_lists_go_and_the_rest_stays_as_written() {
        let part = |lists: &str| {
            format!(
                concat!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                    r#"<Properties xmlns="urn:x" xmlns:vt="urn:vt"><Application>Microsoft Excel</Application>"#,
                    "{}",
                    r#"<AppVersion>16.0300</AppVersion></Properties>"#
                ),
                lists
            )
        };
        let lists = concat!(
            r#"<HeadingPairs><vt:vector size="2" baseType="variant"><vt:variant><vt:lpstr>Worksheets</vt:lpstr></vt:variant><vt:variant><vt:i4>1</vt:i4></vt:variant></vt:vector></HeadingPairs>"#,
            r#"<TitlesOfParts><vt:vector size="1" baseType="lpstr"><vt:lpstr>Sheet1</vt:lpstr></vt:vector></TitlesOfParts>"#
        );
        for (case, before, expected) in [
            ("both lists", part(lists), part("")),
            (
                "only one, and an empty one",
                part("<TitlesOfParts/>"),
                part(""),
            ),
            ("none", part(""), part("")),
        ] {
            let written = without_title_lists(before.as_bytes()).unwrap();
            assert_eq!(String::from_utf8(written).unwrap(), expected, "{case}");
        }
        assert!(without_title_lists(b"<Properties><HeadingPairs>").is_err());
    }
}
