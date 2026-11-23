//! Chart and axis titles with plain-text authoring.

use super::*;

/// A chart title with plain-text authoring and preserved unsupported children.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CT_Title {
    text: Option<CT_TextBody>,
    raw_attributes: Vec<(String, String)>,
    raw_children: OrderedRawChildren,
}

impl CT_Title {
    pub(super) fn plain_text(text: &str) -> Self {
        let mut body = CT_TextBody::new();
        body.set_text(text);
        let mut raw_children = OrderedRawChildren::default();
        raw_children.push(1, br#"<c:layout/><c:overlay val="0"/>"#.to_vec());
        Self {
            text: Some(body),
            raw_children,
            ..Self::default()
        }
    }

    pub(super) fn from_xml(xml: &[u8]) -> Result<Self> {
        let (raw_attributes, raw_children) = parse_raw_shell(xml, b"title", "c:title")?;
        Ok(Self {
            text: None,
            raw_attributes,
            raw_children,
        })
    }

    pub(super) fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        if self.text.is_none() {
            return write_raw_shell(writer, "c:title", &self.raw_attributes, &self.raw_children);
        }
        let mut start = BytesStart::new("c:title");
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(0))?;
        writer
            .write_event(Event::Start(BytesStart::new("c:tx")))
            .map_err(OxmlError::from)?;
        if let Some(text) = &self.text {
            text.write_xml_as(writer, "c:rich")?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("c:tx")))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(1))?;
        writer
            .write_event(Event::End(BytesEnd::new("c:title")))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }
}
