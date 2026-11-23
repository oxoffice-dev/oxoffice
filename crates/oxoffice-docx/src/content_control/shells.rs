//! Markup of new controls: the shell for each placement, its type name and parse, and the checks on tag and alias text.

use super::*;

pub(super) fn reject_control_text(what: &str, value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        oxoffice_core::xml::reject_non_xml_characters(what, value)?;
    }
    Ok(())
}

pub(super) fn plain_or_rich(kind: SdtType) -> Result<&'static str> {
    match kind {
        SdtType::RichText => Ok("richText"),
        SdtType::PlainText => Ok("text"),
        other => Err(Error::Other(format!(
            "{} content control type payload is not supported yet",
            sdt_type_name(other)
        ))),
    }
}

fn sdt_type_name(kind: SdtType) -> &'static str {
    match kind {
        SdtType::RichText => "rich text",
        SdtType::PlainText => "plain text",
        SdtType::Picture => "picture",
        SdtType::CheckBox => "checkbox",
        SdtType::ComboBox => "combo box",
        SdtType::DropDownList => "drop-down list",
        SdtType::Date => "date",
        SdtType::DocumentPartList => "document part list",
        SdtType::DocumentPartObject => "document part object",
        SdtType::Group => "group",
        SdtType::RepeatingSection => "repeating section",
        SdtType::RepeatingSectionItem => "repeating section item",
        SdtType::Citation => "citation",
        SdtType::Equation => "equation",
        SdtType::Bibliography => "bibliography",
    }
}

pub(super) fn control_shell(spec: &ContentControlSpec, id: i32, kind_name: &str) -> Result<String> {
    let body = match spec.placement {
        ContentControlPlacement::Inline => "<w:r/>".to_owned(),
        ContentControlPlacement::Row | ContentControlPlacement::Cell => String::new(),
        ContentControlPlacement::Block | ContentControlPlacement::Paragraph => {
            block_paragraph(spec.kind)?
        }
    };
    control_shell_with_body(spec, id, kind_name, &body)
}

pub(super) fn control_shell_with_body(
    spec: &ContentControlSpec,
    id: i32,
    kind_name: &str,
    body: &str,
) -> Result<String> {
    let mut properties = String::new();
    if let Some(alias) = &spec.alias {
        properties.push_str(&val_element("w:alias", alias));
    }
    if let Some(tag) = &spec.tag {
        properties.push_str(&val_element("w:tag", tag));
    }
    properties.push_str(&val_element("w:id", &id.to_string()));
    if let Some(lock) = spec.lock {
        properties.push_str(&val_element("w:lock", lock.as_str()));
    }
    if spec.placeholder == Some(true) {
        properties.push_str("<w:showingPlcHdr/>");
    }
    properties.push_str(&format!("<w:{kind_name}/>"));
    if let Some(appearance) = spec.appearance {
        properties.push_str(&format!(
            "<w15:appearance w15:val=\"{}\" xmlns:w15=\"{W15_NS}\"/>",
            appearance.as_str()
        ));
    }
    Ok(format!(
        "<w:sdt xmlns:w=\"{W_NS}\"><w:sdtPr>{properties}</w:sdtPr><w:sdtContent>{body}</w:sdtContent></w:sdt>"
    ))
}

fn block_paragraph(kind: SdtType) -> Result<String> {
    match kind {
        SdtType::PlainText => Ok("<w:p><w:r><w:t></w:t></w:r></w:p>".to_owned()),
        SdtType::RichText => Ok("<w:p/>".to_owned()),
        other => Err(Error::Other(format!(
            "{} content control type payload is not supported yet",
            sdt_type_name(other)
        ))),
    }
}

pub(super) fn row_shell(columns: usize, kind: SdtType) -> Result<String> {
    let columns = columns.max(1);
    let cell = cell_shell(kind)?;
    let mut row = String::from("<w:tr>");
    for _ in 0..columns {
        row.push_str(&cell);
    }
    row.push_str("</w:tr>");
    Ok(row)
}

pub(super) fn cell_shell(kind: SdtType) -> Result<String> {
    Ok(format!(
        "<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr>{}</w:tc>",
        block_paragraph(kind)?
    ))
}

fn val_element(name: &str, value: &str) -> String {
    format!("<{name} w:val=\"{}\"/>", xml_attr(value))
}

pub(super) fn xml_attr(value: &str) -> String {
    oxoffice_core::xml::escape_attribute_value(value).into_owned()
}

pub(super) fn parse_control(xml: &str) -> Result<CT_Sdt> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(start)) if start.local_name().as_ref() == b"sdt" => {
                return CT_Sdt::from_xml(&mut reader, &start).map_err(Error::from);
            }
            Ok(Event::Eof) => {
                return Err(Error::Other(
                    "content control shell did not parse".to_owned(),
                ));
            }
            Ok(_) => buffer.clear(),
            Err(error) => {
                return Err(Error::Other(format!(
                    "content control shell did not parse: {error}"
                )));
            }
        }
    }
}

pub(super) fn shell_contains_paragraph(xml: &str) -> Result<bool> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut in_content = false;
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(start)) => {
                let local = start.local_name();
                if in_content && depth == 0 && local.as_ref() == b"p" {
                    return Ok(true);
                }
                if local.as_ref() == b"sdtContent" {
                    in_content = true;
                    depth = 0;
                } else if in_content {
                    depth += 1;
                }
                buffer.clear();
            }
            Ok(Event::Empty(start)) => {
                let local = start.local_name();
                if in_content && depth == 0 && local.as_ref() == b"p" {
                    return Ok(true);
                }
                buffer.clear();
            }
            Ok(Event::End(end)) => {
                if end.local_name().as_ref() == b"sdtContent" {
                    return Ok(false);
                }
                if in_content && depth > 0 {
                    depth -= 1;
                }
                buffer.clear();
            }
            Ok(Event::Eof) => return Ok(false),
            Ok(_) => buffer.clear(),
            Err(error) => {
                return Err(Error::Other(format!(
                    "content control shell did not parse: {error}"
                )));
            }
        }
    }
}
