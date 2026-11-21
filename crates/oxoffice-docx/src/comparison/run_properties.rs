//! Run properties the model does not hold.

use super::*;

pub(super) fn modeled_run_properties(run: &CT_R) -> Option<oxoffice_docx_oxml::properties::CT_RPr> {
    run_formatting(run).cloned().and_then(|mut properties| {
        properties.revision_markers.clear();
        properties.change = None;
        properties.revision_xml.clear();
        properties.revision_xml_positions.clear();
        properties.language_extra_attributes.clear();
        (properties != oxoffice_docx_oxml::properties::CT_RPr::default()).then_some(properties)
    })
}

pub(super) fn unmodeled_run_properties_differ(original: &CT_R, edited: &CT_R) -> bool {
    let original = original.properties.as_ref();
    let edited = edited.properties.as_ref();
    original
        .map(|properties| properties.language_extra_attributes.as_slice())
        .unwrap_or_default()
        != edited
            .map(|properties| properties.language_extra_attributes.as_slice())
            .unwrap_or_default()
        || original
            .map(|properties| properties.revision_xml.as_slice())
            .unwrap_or_default()
            != edited
                .map(|properties| properties.revision_xml.as_slice())
                .unwrap_or_default()
        || original
            .map(|properties| properties.revision_xml_positions.as_slice())
            .unwrap_or_default()
            != edited
                .map(|properties| properties.revision_xml_positions.as_slice())
                .unwrap_or_default()
}

pub(super) fn preserve_unmodeled_run_properties(current: &mut CT_R, original: &CT_R) {
    let current_properties = current
        .properties
        .get_or_insert_with(oxoffice_docx_oxml::properties::CT_RPr::default);
    if let Some(original_properties) = original.properties.as_ref() {
        current_properties.language_extra_attributes =
            original_properties.language_extra_attributes.clone();
        current_properties.revision_xml = original_properties.revision_xml.clone();
        current_properties.revision_xml_positions =
            original_properties.revision_xml_positions.clone();
    } else {
        current_properties.language_extra_attributes.clear();
        current_properties.revision_xml.clear();
        current_properties.revision_xml_positions.clear();
    }
}

pub(super) fn run_property_xml(
    properties: Option<&oxoffice_docx_oxml::properties::CT_RPr>,
) -> Result<String> {
    let mut bytes = Vec::new();
    if let Some(properties) = properties {
        properties.to_xml(&mut Writer::new(&mut bytes))?;
    }
    if bytes.is_empty() {
        Ok("<w:rPr/>".to_owned())
    } else {
        String::from_utf8(bytes).map_err(utf8_error)
    }
}
