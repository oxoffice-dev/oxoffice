//! A new Word package: its default parts and application properties.

use super::*;

pub(super) fn new_word_package(class: WordPackageClass) -> OpcPackage {
    let mut package = OpcPackage::with_main_part("word/document.xml", class.content_type());
    package
        .content_types
        .add_override(DEFAULT_STYLES_PART, STYLES_CONTENT_TYPE);
    package
}

pub(super) fn new_word_compatible_package(
    class: WordPackageClass,
    document: &CT_Document,
    styles: &CT_Styles,
    settings: &CT_Settings,
    core_properties: &CoreProperties,
    application_properties: &AppProperties,
) -> OpcPackage {
    let mut package = new_word_package(class);
    package.set_part(
        "/word/document.xml",
        document
            .to_xml()
            .expect("fresh Word document model must serialize"),
    );
    package.set_part(
        DEFAULT_STYLES_PART,
        styles
            .to_xml()
            .expect("fresh Word styles model must serialize"),
    );
    package.set_part(
        DEFAULT_SETTINGS_PART,
        settings
            .to_xml()
            .expect("fresh Word settings model must serialize"),
    );
    package.set_part(
        DEFAULT_CORE_PROPERTIES_PART,
        core_properties
            .to_xml()
            .expect("fresh core properties model must serialize"),
    );
    package.set_part(
        DEFAULT_APP_PROPERTIES_PART,
        application_properties
            .to_xml()
            .expect("fresh application properties model must serialize"),
    );
    package.set_part(
        DEFAULT_THEME_PART,
        oxoffice_drawing::theme::OFFICE_DEFAULT_XML
            .as_bytes()
            .to_vec(),
    );
    package.set_part(
        DEFAULT_FONT_TABLE_PART,
        DEFAULT_FONT_TABLE_XML.as_bytes().to_vec(),
    );

    package
        .content_types
        .add_override(DEFAULT_SETTINGS_PART, SETTINGS_CONTENT_TYPE);
    package
        .content_types
        .add_override(DEFAULT_FONT_TABLE_PART, FONT_TABLE_CONTENT_TYPE);
    package
        .content_types
        .add_override(DEFAULT_THEME_PART, content_types::THEME);
    package
        .content_types
        .add_override(DEFAULT_CORE_PROPERTIES_PART, content_types::CORE_PROPERTIES);
    package.content_types.add_override(
        DEFAULT_APP_PROPERTIES_PART,
        content_types::EXTENDED_PROPERTIES,
    );

    package
        .package_rels
        .add(rel_types::CORE_PROPERTIES, "docProps/core.xml");
    package
        .package_rels
        .add(rel_types::EXTENDED_PROPERTIES, "docProps/app.xml");
    let document_relationships = package.get_or_create_part_rels("/word/document.xml");
    document_relationships.add(rel_types::STYLES, "styles.xml");
    document_relationships.add_with_id("rdocxSettings", rel_types::SETTINGS, "settings.xml");
    document_relationships.add_with_id("rdocxTheme", rel_types::THEME, "theme/theme1.xml");
    document_relationships.add_with_id("rdocxFontTable", rel_types::FONT_TABLE, "fontTable.xml");
    package
}

pub(super) fn default_application_properties() -> AppProperties {
    let mut properties = AppProperties::default();
    properties.application = Some("oxoffice".to_owned());
    properties.application_version = Some(app_version());
    properties
}

/// The AppVersion oxoffice stamps. ECMA-376 Part 1, 22.2.2.3 fixes AppVersion to
/// the form XX.YYYY, and Word refuses to open a package whose AppVersion has
/// another shape, such as 0.14.0.
fn app_version() -> String {
    format!(
        "{:0>2}.{:0>4}",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR")
    )
}

/// Repair an AppVersion Word would refuse, as rdocx 0.14.0 wrote it: the
/// properties of oxoffice, or of rdocx before it, take the current stamp,
/// and any other producer's value is dropped. A valid AppVersion is left
/// alone, so its part stays byte-identical.
pub(super) fn repair_application_version(mut properties: AppProperties) -> AppProperties {
    if properties
        .application_version
        .as_deref()
        .is_some_and(|version| !is_app_version(version))
    {
        properties.application_version = matches!(
            properties.application.as_deref(),
            Some("oxoffice" | "rdocx")
        )
        .then(app_version);
    }
    properties
}

/// Whether `version` has the `XX.YYYY` form ECMA-376 Part 1, 22.2.2.3 gives AppVersion.
pub(super) fn is_app_version(version: &str) -> bool {
    version.split_once('.').is_some_and(|(major, minor)| {
        major.len() == 2
            && minor.len() == 4
            && major
                .bytes()
                .chain(minor.bytes())
                .all(|byte| byte.is_ascii_digit())
    })
}
