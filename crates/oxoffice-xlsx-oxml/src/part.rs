//! A part whose model is lists of records and single elements under its
//! root, everything else kept as written: the workbook part and the styles
//! part.
//!
//! A part is written from its model only when an edit changed it, and then
//! as its source bytes with each change spliced in: a record appended at the
//! end of its list (the list's `count` updated, the list written at its
//! schema position when the part has none), a single element replaced in
//! place or written at its schema position. Records are never removed or
//! reordered, so the indexes that point at them stay valid.

use std::ops::Range;
use std::sync::Arc;

use oxoffice_core::OxmlError;
use oxoffice_core::xml_element::ElementSpan;

use crate::namespaces::MAIN_NAMESPACES;
use crate::xml::{
    Child, Element, Outline, Result, Splice, Tag, XmlSource, append_to, prefix_of, qualified,
};

/// A typed element of a part and where it was read.
#[derive(Clone, Debug, PartialEq)]
pub struct Located<T> {
    pub range: Range<usize>,
    pub value: T,
}

/// What a part is made of: its root, the order of the root's children, its
/// lists (the list element and the record it holds) and its single
/// elements.
pub(crate) struct Shape {
    pub root: &'static str,
    pub order: &'static [&'static str],
    /// `(list, record, counted)`: a counted list carries `count`.
    pub lists: &'static [(&'static str, &'static str, bool)],
    pub singles: &'static [&'static str],
}

/// One list of records: `sheets`, `fonts`.
#[derive(Clone, Debug)]
pub struct RecordList {
    name: &'static str,
    record: &'static str,
    counted: bool,
    span: Option<ElementSpan>,
    tag: Option<Tag>,
    records: Vec<Located<Element>>,
    added: Vec<Element>,
}

impl RecordList {
    /// The local name of the list element.
    pub fn name(&self) -> &str {
        self.name
    }

    /// Whether the part holds the list element.
    pub fn exists(&self) -> bool {
        self.span.is_some()
    }

    pub fn len(&self) -> usize {
        self.records.len() + self.added.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The record at `index`, read or appended.
    pub fn get(&self, index: usize) -> Option<&Element> {
        match self.records.get(index) {
            Some(record) => Some(&record.value),
            None => self.added.get(index - self.records.len()),
        }
    }

    /// The records, read then appended.
    pub fn iter(&self) -> impl Iterator<Item = &Element> {
        self.records
            .iter()
            .map(|record| &record.value)
            .chain(&self.added)
    }

    /// The records read, with where they were read.
    pub fn located(&self) -> &[Located<Element>] {
        &self.records
    }

    /// Appends `record` and gives its index.
    pub fn push(&mut self, record: Element) -> usize {
        self.added.push(record);
        self.len() - 1
    }
}

/// One single element of the root: `calcPr`, `colors`.
#[derive(Clone, Debug)]
pub struct Single {
    name: &'static str,
    read: Option<Located<Element>>,
    replaced: Option<Element>,
}

/// A part read as lists and single elements. See the [module](self).
#[derive(Clone, Debug)]
pub struct TypedPart {
    source: Arc<Vec<u8>>,
    outline: Outline,
    order: &'static [&'static str],
    prefix: Option<String>,
    lists: Vec<RecordList>,
    singles: Vec<Single>,
}

impl TypedPart {
    pub(crate) fn parse(source: Arc<Vec<u8>>, shape: &Shape) -> Result<Self> {
        let xml: &[u8] = &source;
        let mut reader = XmlSource::new(xml);
        let root = reader.root()?;
        if !root.is(shape.root) {
            return Err(OxmlError::UnexpectedElement(format!(
                "{} where {} was expected",
                String::from_utf8_lossy(root.start.name().as_ref()),
                shape.root
            )));
        }
        let tag = Tag::read(&root.start);
        let mut lists: Vec<RecordList> = shape
            .lists
            .iter()
            .map(|&(name, record, counted)| RecordList {
                name,
                record,
                counted,
                span: None,
                tag: None,
                records: Vec::new(),
                added: Vec::new(),
            })
            .collect();
        let mut singles: Vec<Single> = shape
            .singles
            .iter()
            .map(|&name| Single {
                name,
                read: None,
                replaced: None,
            })
            .collect();
        let mut outline = Outline {
            span: ElementSpan {
                name: tag.name(),
                start_tag: root.tag.clone(),
                content: root.tag.end..root.tag.end,
                end_tag: root.tag.end..root.tag.end,
            },
            tag,
            children: Vec::new(),
        };
        if !root.empty {
            loop {
                match reader.next_child()? {
                    Child::Element(child) => {
                        let name =
                            String::from_utf8_lossy(child.start.local_name().as_ref()).into_owned();
                        let list = lists
                            .iter_mut()
                            .find(|list| list.name == name && list.span.is_none());
                        let single = singles
                            .iter_mut()
                            .find(|single| single.name == name && single.read.is_none());
                        let range = if let Some(list) = list {
                            list.tag = Some(Tag::read(&child.start));
                            let span = read_list(&mut reader, &child, list)?;
                            let range = span.range();
                            list.span = Some(span);
                            range
                        } else if let Some(single) = single {
                            let value = Element::read(&mut reader, &child)?;
                            let range = child.tag.start..reader.position();
                            single.read = Some(Located {
                                range: range.clone(),
                                value,
                            });
                            range
                        } else {
                            reader.finish(&child)?.range()
                        };
                        outline.children.push((name, range));
                    }
                    Child::Raw(_) => {}
                    Child::End(end) => {
                        outline.span.content = root.tag.end..end.start;
                        outline.span.end_tag = end;
                        break;
                    }
                }
            }
        }
        Ok(TypedPart {
            prefix: prefix_of(&outline.tag, &MAIN_NAMESPACES),
            source,
            outline,
            order: shape.order,
            lists,
            singles,
        })
    }

    /// The bytes the part was read from.
    pub fn source(&self) -> &Arc<Vec<u8>> {
        &self.source
    }

    /// The root start tag, as read.
    pub fn root_tag(&self) -> &Tag {
        &self.outline.tag
    }

    /// The children of the root by local name, with their ranges in the
    /// source.
    pub fn children(&self) -> &[(String, Range<usize>)] {
        &self.outline.children
    }

    pub(crate) fn list(&self, name: &str) -> &RecordList {
        self.lists
            .iter()
            .find(|list| list.name == name)
            .expect("a list of the part's shape")
    }

    pub(crate) fn list_mut(&mut self, name: &str) -> &mut RecordList {
        self.lists
            .iter_mut()
            .find(|list| list.name == name)
            .expect("a list of the part's shape")
    }

    /// The lists of the part, in the order of its shape.
    pub fn lists(&self) -> &[RecordList] {
        &self.lists
    }

    /// The single element `name`, as replaced or read.
    pub(crate) fn single(&self, name: &str) -> Option<&Element> {
        let single = self.singles.iter().find(|single| single.name == name)?;
        single
            .replaced
            .as_ref()
            .or(single.read.as_ref().map(|read| &read.value))
    }

    /// Replaces the single element `name` of the part's shape by `element`.
    pub(crate) fn set_single(&mut self, name: &str, element: Element) {
        let single = self
            .singles
            .iter_mut()
            .find(|single| single.name == name)
            .expect("a single element of the part's shape");
        single.replaced = Some(element);
    }

    /// The single elements read, with where they were read.
    pub fn located_singles(&self) -> impl Iterator<Item = &Located<Element>> {
        self.singles
            .iter()
            .filter_map(|single| single.read.as_ref())
    }

    /// The prefix new elements are written with: the one the root binds to
    /// the SpreadsheetML namespace.
    pub fn prefix(&self) -> &str {
        self.prefix.as_deref().unwrap_or("")
    }

    /// Whether an edit changed the part.
    pub fn is_changed(&self) -> bool {
        self.lists.iter().any(|list| !list.added.is_empty())
            || self.singles.iter().any(|single| single.replaced.is_some())
    }

    /// The part as it stands: its source bytes when nothing changed, else
    /// the source with each change spliced in.
    pub fn to_xml(&self) -> Vec<u8> {
        if !self.is_changed() {
            return self.source.to_vec();
        }
        let outline = &self.outline;
        let mut splice = Splice::default();
        let mut inserted: Vec<(usize, usize, Vec<u8>)> = Vec::new();
        let rank = |name: &str| self.order.iter().position(|known| *known == name);
        for single in &self.singles {
            let Some(replaced) = &single.replaced else {
                continue;
            };
            match &single.read {
                Some(read) => splice.replace(read.range.clone(), replaced.to_xml()),
                None => inserted.push((
                    outline.insertion_point(self.order, single.name),
                    rank(single.name).unwrap_or(usize::MAX),
                    replaced.to_xml(),
                )),
            }
        }
        for list in self.lists.iter().filter(|list| !list.added.is_empty()) {
            let mut content = Vec::new();
            for record in &list.added {
                record.write(&mut content);
            }
            match (&list.span, &list.tag) {
                (Some(span), Some(tag)) => {
                    let mut counted = tag.clone();
                    if list.counted && tag.get("count").is_some() {
                        counted.set("count", Some(&list.len().to_string()));
                    }
                    append_to(&mut splice, span, tag, Some(&counted), content);
                }
                _ => {
                    let mut tag = Tag::new(&qualified(self.prefix(), list.name));
                    if list.counted {
                        tag.set("count", Some(&list.len().to_string()));
                    }
                    let mut element = Vec::new();
                    tag.write_start(&mut element);
                    element.extend_from_slice(&content);
                    tag.write_end(&mut element);
                    inserted.push((
                        outline.insertion_point(self.order, list.name),
                        rank(list.name).unwrap_or(usize::MAX),
                        element,
                    ));
                }
            }
        }
        if !inserted.is_empty() {
            inserted.sort_by_key(|(at, rank, _)| (*at, *rank));
            if outline.span.is_self_closing() {
                let content = inserted
                    .into_iter()
                    .flat_map(|(_, _, bytes)| bytes)
                    .collect();
                append_to(&mut splice, &outline.span, &outline.tag, None, content);
            } else {
                for (at, _, bytes) in inserted {
                    splice.insert(at, bytes);
                }
            }
        }
        splice.apply(&self.source)
    }

    /// The part with every record and single element it read written again
    /// by its writer, in place: what a test compares with the source.
    #[doc(hidden)]
    pub fn to_xml_through_writers(&self) -> Vec<u8> {
        let mut splice = Splice::default();
        for list in &self.lists {
            for record in &list.records {
                splice.replace(record.range.clone(), record.value.to_xml());
            }
        }
        for single in self
            .singles
            .iter()
            .filter_map(|single| single.read.as_ref())
        {
            splice.replace(single.range.clone(), single.value.to_xml());
        }
        splice.apply(&self.source)
    }

    /// How many elements [`Self::to_xml_through_writers`] writes.
    #[doc(hidden)]
    pub fn written_through_writers(&self) -> usize {
        self.lists
            .iter()
            .map(|list| list.records.len())
            .sum::<usize>()
            + self.located_singles().count()
    }
}

/// Reads a list element, its records by their range.
fn read_list(
    reader: &mut XmlSource<'_>,
    opened: &crate::xml::Opened<'_>,
    list: &mut RecordList,
) -> Result<ElementSpan> {
    let mut span = ElementSpan {
        name: String::from_utf8_lossy(opened.start.name().as_ref()).into_owned(),
        start_tag: opened.tag.clone(),
        content: opened.tag.end..opened.tag.end,
        end_tag: opened.tag.end..opened.tag.end,
    };
    if opened.empty {
        return Ok(span);
    }
    loop {
        match reader.next_child()? {
            Child::Element(child) if child.is(list.record) => {
                let value = Element::read(reader, &child)?;
                list.records.push(Located {
                    range: child.tag.start..reader.position(),
                    value,
                });
            }
            Child::Element(other) => {
                reader.finish(&other)?;
            }
            Child::Raw(_) => {}
            Child::End(end) => {
                span.content = opened.tag.end..end.start;
                span.end_tag = end;
                return Ok(span);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::shared_strings::SharedStrings;
    use crate::styles::{StyleList, StylesXml};
    use crate::workbook::WorkbookXml;
    use crate::xml::Element;

    fn element(xml: &str) -> Element {
        Element::parse(xml.as_bytes()).unwrap()
    }

    fn source(xml: &str) -> Arc<Vec<u8>> {
        Arc::new(xml.as_bytes().to_vec())
    }

    // Indexes only grow: a record goes at the end of its list, which is
    // written at its schema position when the part has none, its count
    // updated; what was read keeps its bytes.
    #[test]
    fn records_are_appended_at_the_end_of_their_list() {
        let google_styles = concat!(
            r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
            r#"<fonts count="1"><font><sz val="11.0"/></font></fonts><cellXfs count="1"><xf/></cellXfs>"#,
            r#"<dxfs count="0"/></styleSheet>"#
        );
        let mut styles = StylesXml::parse(source(google_styles)).unwrap();
        assert!(!styles.list(StyleList::NumFmts).exists());
        assert_eq!(
            styles.push(
                StyleList::NumFmts,
                element(r#"<numFmt numFmtId="164" formatCode="0.0"/>"#)
            ),
            0
        );
        assert_eq!(
            styles.push(StyleList::Fonts, element("<font><b/></font>")),
            1
        );
        assert_eq!(
            String::from_utf8(styles.to_xml()).unwrap(),
            concat!(
                r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
                r#"<numFmts count="1"><numFmt numFmtId="164" formatCode="0.0"/></numFmts>"#,
                r#"<fonts count="2"><font><sz val="11.0"/></font><font><b/></font></fonts><cellXfs count="1"><xf/></cellXfs>"#,
                r#"<dxfs count="0"/></styleSheet>"#
            )
        );

        let workbook = concat!(
            r#"<x:workbook xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="urn:r">"#,
            r#"<x:sheets><x:sheet name="A" sheetId="1" r:id="rId1"/></x:sheets><x:definedNames/><x:extLst/></x:workbook>"#
        );
        let mut parsed = WorkbookXml::parse(source(workbook)).unwrap();
        assert_eq!(parsed.prefix(), "x");
        assert!(parsed.calc_pr().is_none());
        parsed.set_calc_pr(element(r#"<x:calcPr fullCalcOnLoad="1"/>"#));
        assert_eq!(
            parsed.push_sheet(element(r#"<x:sheet name="B" sheetId="2" r:id="rId9"/>"#)),
            1
        );
        let written = String::from_utf8(parsed.to_xml()).unwrap();
        assert_eq!(
            written,
            workbook
                .replace(
                    r#"rId1"/>"#,
                    r#"rId1"/><x:sheet name="B" sheetId="2" r:id="rId9"/>"#
                )
                .replace(
                    "<x:definedNames/>",
                    r#"<x:definedNames/><x:calcPr fullCalcOnLoad="1"/>"#
                )
        );
        let reread = WorkbookXml::parse(Arc::new(written.into_bytes())).unwrap();
        assert!(reread.calc_pr().unwrap().full_calc_on_load());
        parsed.set_calc_pr(element(r#"<x:calcPr calcId="1"/>"#));
        let mut parsed = WorkbookXml::parse(Arc::new(parsed.to_xml())).unwrap();
        parsed.set_calc_pr(element("<x:calcPr/>"));
        assert!(
            String::from_utf8(parsed.to_xml())
                .unwrap()
                .contains("<x:definedNames/><x:calcPr/><x:extLst/>")
        );

        for (case, part, added, references, expected) in [
            (
                "after the last item, before extLst",
                r#"<sst xmlns="urn:x" count="3" uniqueCount="1"><si><t>a</t></si><extLst/></sst>"#,
                vec![" b", "c\u{1}"],
                2,
                r#"<sst xmlns="urn:x" count="5" uniqueCount="3"><si><t>a</t></si><si><t xml:space="preserve"> b</t></si><si><t>c_x0001_</t></si><extLst/></sst>"#,
            ),
            (
                "into an empty part",
                r#"<sst xmlns="urn:x"/>"#,
                vec!["a"],
                1,
                r#"<sst xmlns="urn:x"><si><t>a</t></si></sst>"#,
            ),
        ] {
            let mut strings = SharedStrings::parse(source(part)).unwrap();
            for text in &added {
                strings.push_plain(text);
            }
            strings.add_references(references);
            assert_eq!(
                String::from_utf8(strings.to_xml()).unwrap(),
                expected,
                "{case}"
            );
            let reread = SharedStrings::parse(Arc::new(strings.to_xml())).unwrap();
            assert_eq!(
                reread.text(reread.len() - 1).as_deref(),
                added.last().copied(),
                "{case}"
            );
        }
        // The namespace is text: escaped once, never taken for markup.
        assert!(SharedStrings::empty("urn:a&b\"c<d").is_empty());
        let mut created = SharedStrings::empty("http://purl.oclc.org/ooxml/spreadsheetml/main");
        assert_eq!(created.push_plain("x"), 0);
        assert!(String::from_utf8(created.to_xml()).unwrap().ends_with(
            r#"<sst xmlns="http://purl.oclc.org/ooxml/spreadsheetml/main" count="0" uniqueCount="1"><si><t>x</t></si></sst>"#
        ));
    }
}
