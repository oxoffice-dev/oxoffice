//! Checking that a package is a Word package of a known class.

use super::*;

pub(super) fn validate_fresh_word_compatible_package(
    package: &OpcPackage,
    expected_class: WordPackageClass,
) -> Result<()> {
    let (_, actual_class) = validated_word_package_class(package)?;
    if actual_class != expected_class {
        return Err(Error::Other(
            "fresh Word package class does not match its profile".to_owned(),
        ));
    }
    let expected_parts = [
        ("/word/document.xml", expected_class.content_type()),
        (DEFAULT_STYLES_PART, STYLES_CONTENT_TYPE),
        (DEFAULT_SETTINGS_PART, SETTINGS_CONTENT_TYPE),
        (DEFAULT_THEME_PART, content_types::THEME),
        (DEFAULT_FONT_TABLE_PART, FONT_TABLE_CONTENT_TYPE),
        (DEFAULT_CORE_PROPERTIES_PART, content_types::CORE_PROPERTIES),
        (
            DEFAULT_APP_PROPERTIES_PART,
            content_types::EXTENDED_PROPERTIES,
        ),
    ];
    for (part_name, expected_content_type) in expected_parts {
        if !package.contains_part(part_name)
            || package
                .content_types
                .override_for(part_name)
                .is_none_or(|actual| actual != expected_content_type)
        {
            return Err(Error::Other(format!(
                "fresh Word package is missing required part {part_name}"
            )));
        }
    }
    if package.parts.len() != expected_parts.len()
        || package.content_types.overrides.len() != expected_parts.len()
    {
        return Err(Error::Other(
            "fresh Word package contains an unexpected owned part".to_owned(),
        ));
    }
    let expected_package_relationships = [
        (rel_types::DOCUMENT, "/word/document.xml"),
        (rel_types::CORE_PROPERTIES, DEFAULT_CORE_PROPERTIES_PART),
        (rel_types::EXTENDED_PROPERTIES, DEFAULT_APP_PROPERTIES_PART),
    ];
    let expected_document_relationships = [
        (rel_types::STYLES, DEFAULT_STYLES_PART),
        (rel_types::SETTINGS, DEFAULT_SETTINGS_PART),
        (rel_types::THEME, DEFAULT_THEME_PART),
        (rel_types::FONT_TABLE, DEFAULT_FONT_TABLE_PART),
    ];
    let document_relationships = package
        .get_part_rels("/word/document.xml")
        .ok_or_else(|| Error::Other("fresh Word package has no main relationships".to_owned()))?;
    if package.package_rels.items.len() != expected_package_relationships.len()
        || document_relationships.items.len() != expected_document_relationships.len()
        || package.part_rels.len() != 1
    {
        return Err(Error::Other(
            "fresh Word package relationship inventory is incomplete".to_owned(),
        ));
    }
    for (source, relationships, expected) in [
        (
            "/",
            &package.package_rels,
            expected_package_relationships.as_slice(),
        ),
        (
            "/word/document.xml",
            document_relationships,
            expected_document_relationships.as_slice(),
        ),
    ] {
        for (relationship_type, expected_target) in expected {
            let matching: Vec<_> = relationships
                .items
                .iter()
                .filter(|relationship| relationship.rel_type == *relationship_type)
                .collect();
            if matching.len() != 1
                || matching[0].target_mode.is_some()
                || OpcPackage::resolve_rel_target(source, &matching[0].target) != *expected_target
            {
                return Err(Error::Other(format!(
                    "fresh Word package relationship {relationship_type} is invalid"
                )));
            }
        }
    }
    for (source, relationships) in std::iter::once(("/", &package.package_rels)).chain(
        package
            .part_rels
            .iter()
            .map(|(source, rels)| (source.as_str(), rels)),
    ) {
        for relationship in &relationships.items {
            if !relationship_is_internal(relationship) {
                continue;
            }
            let target = OpcPackage::resolve_rel_target(source, &relationship.target);
            if !package.contains_part(&target) {
                return Err(Error::Other(format!(
                    "fresh Word relationship targets missing part {target}"
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn validated_word_package_class(
    package: &OpcPackage,
) -> Result<(String, WordPackageClass)> {
    let relationships: Vec<_> = package
        .package_rels
        .items
        .iter()
        .filter(|relationship| relationship.rel_type == rel_types::DOCUMENT)
        .collect();
    let relationship = match relationships.as_slice() {
        [relationship] => *relationship,
        [] => return Err(Error::NoDocumentPart),
        _ => {
            return Err(Error::Other(
                "Word package has multiple officeDocument relationships".to_owned(),
            ));
        }
    };
    if !matches!(relationship.target_mode.as_deref(), None | Some("Internal")) {
        return Err(Error::Other(
            "Word package officeDocument relationship must be internal".to_owned(),
        ));
    }
    let target = relationship.target.as_str();
    if !crate::embedded::relationship_target_is_normalized_pack_uri(target)
        || target
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    {
        return Err(Error::Other(
            "Word package officeDocument relationship has an unsafe target".to_owned(),
        ));
    }
    let doc_part_name = if target.starts_with('/') {
        target.to_owned()
    } else {
        format!("/{target}")
    };
    if !package.contains_part(&doc_part_name) {
        return Err(Error::NoDocumentPart);
    }
    let content_type = package
        .content_types
        .override_for(&doc_part_name)
        .ok_or_else(|| {
            Error::Other("Word main part requires an exact content-type override".to_owned())
        })?;
    let class = WordPackageClass::from_content_type(content_type).ok_or_else(|| {
        Error::Other(format!(
            "unsupported Word main-part content type: {content_type}"
        ))
    })?;
    Ok((doc_part_name, class))
}
