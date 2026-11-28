//! The metadata part (`CT_Metadata`, `metadata.xml`): the block of cell
//! metadata that marks a formula as a dynamic array (Excel 365's spilling
//! formulas), found where the part has it or appended, everything else kept
//! as written.
//!
//! A dynamic array cell carries `cm`, the 1-based index of a `cellMetadata`
//! block whose `rc` names the `XLDAPR` metadata type (`t`, 1-based) and a
//! block of the `XLDAPR` future metadata (`v`, 0-based) holding
//! `dynamicArrayProperties fDynamic="1" fCollapsed="0"`, as Excel and
//! XlsxWriter write them. What is missing of the three is appended at the
//! end of its list, the list's `count` updated, or written at its schema
//! position when the part has no such list.

use std::ops::Range;
use std::sync::Arc;

use oxoffice_core::OxmlError;
use oxoffice_core::xml_element::ElementSpan;

use crate::namespaces::MAIN_NAMESPACES;
use crate::xml::{
    Child, Element, Result, Splice, Tag, XmlSource, append_to, insertion_point, prefix_of,
    qualified,
};

/// The namespace of `dynamicArrayProperties`.
pub const DYNAMIC_ARRAY_NAMESPACE: &str =
    "http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray";

/// The extension that holds `dynamicArrayProperties` in a future metadata
/// block.
const DYNAMIC_ARRAY_EXTENSION: &str = "{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}";

/// The name of the metadata type of dynamic arrays.
const DYNAMIC_ARRAY_TYPE: &str = "XLDAPR";

/// The children of `metadata`, in schema order.
const METADATA_CHILDREN: [&str; 7] = [
    "metadataTypes",
    "metadataStrings",
    "mdxMetadata",
    "futureMetadata",
    "cellMetadata",
    "valueMetadata",
    "extLst",
];

/// A list of the part as read: its ranges, its start tag and its records.
#[derive(Clone, Debug)]
struct List {
    span: ElementSpan,
    tag: Tag,
    records: Vec<Element>,
}

/// A record appended to a list, which the part may lack.
#[derive(Clone, Debug, Default)]
struct Added {
    metadata_type: Option<Element>,
    future_block: Option<Element>,
    cell_block: Option<Element>,
}

/// The metadata part. See the [module](self).
#[derive(Clone, Debug)]
pub struct MetadataXml {
    source: Arc<Vec<u8>>,
    root: ElementSpan,
    root_tag: Tag,
    prefix: String,
    /// The prefix the root binds to the dynamic array namespace.
    dynamic_prefix: Option<String>,
    children: Vec<(String, Range<usize>)>,
    types: Option<List>,
    /// The `futureMetadata` named `XLDAPR`.
    future: Option<List>,
    cells: Option<List>,
    added: Added,
    /// The `cm` of a dynamic array cell, once found or appended.
    dynamic_cell: Option<u32>,
}

impl MetadataXml {
    pub fn parse(source: Arc<Vec<u8>>) -> Result<Self> {
        let xml: &[u8] = &source;
        let mut reader = XmlSource::new(xml);
        let root = reader.root()?;
        if !root.is("metadata") {
            return Err(OxmlError::UnexpectedElement(format!(
                "{} where metadata was expected",
                String::from_utf8_lossy(root.start.name().as_ref())
            )));
        }
        let root_tag = Tag::read(&root.start);
        let mut span = ElementSpan {
            name: root_tag.name(),
            start_tag: root.tag.clone(),
            content: root.tag.end..root.tag.end,
            end_tag: root.tag.end..root.tag.end,
        };
        let (mut children, mut types, mut future, mut cells) = (Vec::new(), None, None, None);
        if !root.empty {
            loop {
                match reader.next_child()? {
                    Child::Element(child) => {
                        let name =
                            String::from_utf8_lossy(child.start.local_name().as_ref()).into_owned();
                        let tag = Tag::read(&child.start);
                        let child_span = reader.finish(&child)?;
                        let list = || -> Result<List> {
                            let element = Element::parse(&xml[child_span.range()])?;
                            Ok(List {
                                span: child_span.clone(),
                                tag: tag.clone(),
                                records: element.elements().cloned().collect(),
                            })
                        };
                        match name.as_str() {
                            "metadataTypes" if types.is_none() => types = Some(list()?),
                            "futureMetadata"
                                if future.is_none()
                                    && tag.get("name").as_deref() == Some(DYNAMIC_ARRAY_TYPE) =>
                            {
                                future = Some(list()?)
                            }
                            "cellMetadata" if cells.is_none() => cells = Some(list()?),
                            _ => {}
                        }
                        children.push((name, child_span.range()));
                    }
                    Child::Raw(_) => {}
                    Child::End(end) => {
                        span.content = root.tag.end..end.start;
                        span.end_tag = end;
                        break;
                    }
                }
            }
        }
        let dynamic_prefix = root_tag
            .namespace_declarations()
            .into_iter()
            .find(|(prefix, uri)| !prefix.is_empty() && uri == DYNAMIC_ARRAY_NAMESPACE)
            .map(|(prefix, _)| prefix);
        Ok(MetadataXml {
            prefix: prefix_of(&root_tag, &MAIN_NAMESPACES).unwrap_or_default(),
            dynamic_prefix,
            source,
            root: span,
            root_tag,
            children,
            types,
            future,
            cells,
            added: Added::default(),
            dynamic_cell: None,
        })
    }

    /// A new part in `namespace` (the SpreadsheetML main namespace of the
    /// workbook, Transitional or Strict), for a workbook that has none:
    /// written as Excel writes it, once its blocks are added.
    pub fn empty(namespace: &str) -> Self {
        let namespace = oxoffice_core::xml::escape_attribute_value(namespace);
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<metadata xmlns=\"{namespace}\" xmlns:xda=\"{DYNAMIC_ARRAY_NAMESPACE}\"/>"
        );
        Self::parse(Arc::new(xml.into_bytes())).expect("the empty part reads")
    }

    /// The `cm` of a dynamic array cell: the 1-based index of the
    /// `cellMetadata` block that names the dynamic array type and block,
    /// each found in the part or appended.
    pub fn dynamic_array_cell(&mut self) -> u32 {
        if let Some(cm) = self.dynamic_cell {
            return cm;
        }
        fn records(list: &Option<List>) -> &[Element] {
            list.as_ref().map_or(&[], |list| &list.records)
        }

        // The metadata type, 1-based.
        let types = records(&self.types);
        let t = match types
            .iter()
            .position(|record| record.tag.get("name").as_deref() == Some(DYNAMIC_ARRAY_TYPE))
        {
            Some(position) => position + 1,
            None => {
                let t = types.len() + 1;
                self.added.metadata_type = Some(self.metadata_type());
                t
            }
        };

        // The future metadata block, 0-based.
        let blocks = records(&self.future);
        let v = match blocks.iter().position(is_dynamic_block) {
            Some(position) => position,
            None => {
                let v = blocks.len();
                self.added.future_block = Some(self.future_block());
                v
            }
        };

        // The cell metadata block, 1-based.
        let named = |block: &Element| {
            let rcs: Vec<&Element> = block.elements_named("rc").collect();
            rcs.len() == 1
                && rcs[0].tag.u32("t") == u32::try_from(t).ok()
                && rcs[0].tag.u32("v") == u32::try_from(v).ok()
        };
        let cells = records(&self.cells);
        let cm = match cells.iter().position(named) {
            Some(position) => position + 1,
            None => {
                let cm = cells.len() + 1;
                self.added.cell_block = Some(self.cell_block(t, v));
                cm
            }
        };
        let cm = u32::try_from(cm).expect("a metadata index fits in u32");
        self.dynamic_cell = Some(cm);
        cm
    }

    fn element(&self, local: &str) -> Element {
        Element::new(&qualified(&self.prefix, local))
    }

    /// The `XLDAPR` metadata type, as Excel writes it.
    fn metadata_type(&self) -> Element {
        let mut record = self.element("metadataType");
        record.tag.set("name", Some(DYNAMIC_ARRAY_TYPE));
        record.tag.set("minSupportedVersion", Some("120000"));
        for flag in [
            "copy",
            "pasteAll",
            "pasteValues",
            "merge",
            "splitFirst",
            "rowColShift",
            "clearFormats",
            "clearComments",
            "assign",
            "coerce",
            "cellMeta",
        ] {
            record.tag.set(flag, Some("1"));
        }
        record
    }

    /// A future metadata block of a dynamic array that does not collapse.
    fn future_block(&self) -> Element {
        let mut properties = match &self.dynamic_prefix {
            Some(prefix) => Element::new(&qualified(prefix, "dynamicArrayProperties")),
            None => {
                let mut element = Element::new("xda:dynamicArrayProperties");
                element.tag.set("xmlns:xda", Some(DYNAMIC_ARRAY_NAMESPACE));
                element
            }
        };
        properties.tag.set("fDynamic", Some("1"));
        properties.tag.set("fCollapsed", Some("0"));
        let mut ext = self.element("ext");
        ext.tag.set("uri", Some(DYNAMIC_ARRAY_EXTENSION));
        ext.children.push(crate::xml::Node::Element(properties));
        let mut list = self.element("extLst");
        list.children.push(crate::xml::Node::Element(ext));
        let mut block = self.element("bk");
        block.children.push(crate::xml::Node::Element(list));
        block
    }

    /// A cell metadata block naming type `t` and future block `v`.
    fn cell_block(&self, t: usize, v: usize) -> Element {
        let mut rc = self.element("rc");
        rc.tag.set("t", Some(&t.to_string()));
        rc.tag.set("v", Some(&v.to_string()));
        let mut block = self.element("bk");
        block.children.push(crate::xml::Node::Element(rc));
        block
    }

    pub fn is_changed(&self) -> bool {
        self.added.metadata_type.is_some()
            || self.added.future_block.is_some()
            || self.added.cell_block.is_some()
    }

    /// The part as it stands: its source bytes when nothing changed, else
    /// the source with the appended records and lists spliced in.
    pub fn to_xml(&self) -> Vec<u8> {
        if !self.is_changed() {
            return self.source.to_vec();
        }
        let mut splice = Splice::default();
        let mut inserted: Vec<(usize, usize, Vec<u8>)> = Vec::new();
        let rank = |name: &str| METADATA_CHILDREN.iter().position(|known| *known == name);
        let lists = [
            (
                "metadataTypes",
                &self.types,
                &self.added.metadata_type,
                None,
            ),
            (
                "futureMetadata",
                &self.future,
                &self.added.future_block,
                Some(DYNAMIC_ARRAY_TYPE),
            ),
            ("cellMetadata", &self.cells, &self.added.cell_block, None),
        ];
        for (name, list, added, list_name) in lists {
            let Some(record) = added else {
                continue;
            };
            let content = record.to_xml();
            match list {
                Some(list) => {
                    let mut counted = list.tag.clone();
                    if counted.get("count").is_some() {
                        counted.set("count", Some(&(list.records.len() + 1).to_string()));
                    }
                    append_to(&mut splice, &list.span, &list.tag, Some(&counted), content);
                }
                None => {
                    let mut tag = Tag::new(&qualified(&self.prefix, name));
                    if let Some(list_name) = list_name {
                        tag.set("name", Some(list_name));
                    }
                    tag.set("count", Some("1"));
                    let mut element = Vec::new();
                    tag.write_start(&mut element);
                    element.extend_from_slice(&content);
                    tag.write_end(&mut element);
                    let children = self
                        .children
                        .iter()
                        .map(|(child, range)| (child.as_str(), range.start));
                    let at =
                        insertion_point(children, self.root.content.end, &METADATA_CHILDREN, name);
                    inserted.push((at, rank(name).unwrap_or(usize::MAX), element));
                }
            }
        }
        if !inserted.is_empty() {
            inserted.sort_by_key(|(at, rank, _)| (*at, *rank));
            if self.root.is_self_closing() {
                let content = inserted
                    .into_iter()
                    .flat_map(|(_, _, bytes)| bytes)
                    .collect();
                append_to(&mut splice, &self.root, &self.root_tag, None, content);
            } else {
                for (at, _, bytes) in inserted {
                    splice.insert(at, bytes);
                }
            }
        }
        splice.apply(&self.source)
    }
}

/// Whether a future metadata block marks a dynamic array that does not
/// collapse: `dynamicArrayProperties fDynamic="1"`, `fCollapsed` false.
fn is_dynamic_block(block: &Element) -> bool {
    fn find(element: &Element) -> Option<&Element> {
        if element.tag.is("dynamicArrayProperties") {
            return Some(element);
        }
        element.elements().find_map(find)
    }
    find(block).is_some_and(|properties| {
        properties.tag.bool("fDynamic") == Some(true)
            && !properties.tag.bool("fCollapsed").unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

    #[test]
    fn a_dynamic_array_block_is_found_or_appended_where_the_part_lacks_it() {
        let excel = format!(
            r#"<metadata xmlns="{MAIN}" xmlns:xda="{DYNAMIC_ARRAY_NAMESPACE}"><metadataTypes count="1"><metadataType name="XLDAPR" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1" cellMeta="1"/></metadataTypes><futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{DYNAMIC_ARRAY_EXTENSION}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata><cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata></metadata>"#
        );
        let created = MetadataXml::empty(MAIN).to_xml();
        let mut new = MetadataXml::empty(MAIN);
        assert_eq!(new.dynamic_array_cell(), 1);
        let written = String::from_utf8(new.to_xml()).unwrap();
        assert!(written.ends_with(&excel), "{written}");
        assert!(String::from_utf8(created).unwrap().ends_with("/>"));

        // Found in place: nothing written.
        let mut found = MetadataXml::parse(Arc::new(excel.clone().into_bytes())).unwrap();
        assert_eq!(found.dynamic_array_cell(), 1);
        assert!(!found.is_changed());
        assert_eq!(found.to_xml(), excel.as_bytes());

        // A rich value type first, a collapsed block, another cell block:
        // the type is the second, the block appended, the cell block too.
        let rich = format!(
            r#"<metadata xmlns="{MAIN}"><metadataTypes count="2"><metadataType name="XLRICHVALUE" minSupportedVersion="120000"/><metadataType name="XLDAPR" minSupportedVersion="120000"/></metadataTypes><futureMetadata name="XLRICHVALUE" count="1"><bk/></futureMetadata><futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{DYNAMIC_ARRAY_EXTENSION}"><xda:dynamicArrayProperties xmlns:xda="{DYNAMIC_ARRAY_NAMESPACE}" fDynamic="1" fCollapsed="1"/></ext></extLst></bk></futureMetadata><cellMetadata count="1"><bk><rc t="2" v="0"/></bk></cellMetadata><valueMetadata count="1"><bk><rc t="1" v="0"/></bk></valueMetadata></metadata>"#
        );
        let mut appended = MetadataXml::parse(Arc::new(rich.clone().into_bytes())).unwrap();
        assert_eq!(appended.dynamic_array_cell(), 2);
        let block = format!(
            r#"<bk><extLst><ext uri="{DYNAMIC_ARRAY_EXTENSION}"><xda:dynamicArrayProperties xmlns:xda="{DYNAMIC_ARRAY_NAMESPACE}" fDynamic="1" fCollapsed="0"/></ext></extLst></bk>"#
        );
        let expected = rich
            .replace(
                r#"<futureMetadata name="XLDAPR" count="1">"#,
                r#"<futureMetadata name="XLDAPR" count="2">"#,
            )
            .replace(
                "</bk></futureMetadata><cellMetadata",
                &format!("</bk>{block}</futureMetadata><cellMetadata"),
            )
            .replace(
                r#"<cellMetadata count="1"><bk><rc t="2" v="0"/></bk></cellMetadata>"#,
                r#"<cellMetadata count="2"><bk><rc t="2" v="0"/></bk><bk><rc t="2" v="1"/></bk></cellMetadata>"#,
            );
        assert_eq!(String::from_utf8(appended.to_xml()).unwrap(), expected);

        // Only rich values: the type, the future metadata and the cell
        // metadata written at their schema positions, under the root's prefix.
        let only = r#"<x:metadata xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><x:metadataTypes count="1"><x:metadataType name="XLRICHVALUE"/></x:metadataTypes><x:futureMetadata name="XLRICHVALUE" count="1"><x:bk/></x:futureMetadata><x:valueMetadata count="1"><x:bk><x:rc t="1" v="0"/></x:bk></x:valueMetadata></x:metadata>"#;
        let mut added = MetadataXml::parse(Arc::new(only.as_bytes().to_vec())).unwrap();
        assert_eq!(added.dynamic_array_cell(), 1);
        let written = String::from_utf8(added.to_xml()).unwrap();
        assert!(
            written
                .contains(r#"<x:metadataType name="XLRICHVALUE"/><x:metadataType name="XLDAPR""#),
            "{written}"
        );
        assert!(
            written.contains(
                r#"</x:futureMetadata><x:futureMetadata name="XLDAPR" count="1"><x:bk><x:extLst>"#
            ),
            "{written}"
        );
        assert!(
            written.contains(r#"<x:cellMetadata count="1"><x:bk><x:rc t="2" v="0"/></x:bk></x:cellMetadata><x:valueMetadata"#),
            "{written}"
        );
    }
}
