//! Namespaces, relationship types and content types of SpreadsheetML, and
//! the one table that reads Strict Open XML as Transitional.
//!
//! A Strict workbook (`purl.oclc.org` namespaces, as Excel's "Strict Open
//! XML Spreadsheet" writes it) uses other namespaces and relationship types
//! for the same parts. Elements are read by their local names, so a Strict
//! part reads as a Transitional one; relationship types go through
//! [`transitional_relationship_type`]. A part keeps the namespace it was
//! read with.

/// The SpreadsheetML main namespace, Transitional.
pub const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
/// The SpreadsheetML main namespace, Strict.
pub const STRICT_MAIN: &str = "http://purl.oclc.org/ooxml/spreadsheetml/main";
/// Both writings of the main namespace.
pub const MAIN_NAMESPACES: [&str; 2] = [MAIN, STRICT_MAIN];

/// The namespace of relationship ids (`r:id`), Transitional.
pub const RELATIONSHIPS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Both writings of the namespace of relationship ids.
pub fn is_relationships_namespace(value: &str) -> bool {
    transitional(value) == RELATIONSHIPS
}

/// The relationship types of the parts a workbook reads, Transitional.
pub mod rel_types {
    pub const OFFICE_DOCUMENT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
    pub const WORKSHEET: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet";
    pub const CHARTSHEET: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartsheet";
    pub const DIALOGSHEET: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/dialogsheet";
    pub const MACROSHEET: &str =
        "http://schemas.microsoft.com/office/2006/relationships/xlMacrosheet";
    pub const INTERNATIONAL_MACROSHEET: &str =
        "http://schemas.microsoft.com/office/2006/relationships/xlIntlMacrosheet";
    pub const SHARED_STRINGS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings";
    pub const STYLES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
    pub const THEME: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";
    pub const CALC_CHAIN: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/calcChain";
    pub const VBA_PROJECT: &str =
        "http://schemas.microsoft.com/office/2006/relationships/vbaProject";
    pub const COMMENTS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
    /// Excel 365's threaded comments of a sheet (no Strict twin).
    pub const THREADED_COMMENT: &str =
        "http://schemas.microsoft.com/office/2017/10/relationships/threadedComment";
    /// The people who wrote threaded comments, from the workbook part.
    pub const PERSON: &str = "http://schemas.microsoft.com/office/2017/10/relationships/person";
    /// The cell and value metadata of the workbook (`metadata.xml`): what
    /// marks a dynamic array formula.
    pub const SHEET_METADATA: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata";
    /// A table of a sheet.
    pub const TABLE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/table";
    /// A pivot table of a sheet.
    pub const PIVOT_TABLE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotTable";
}

/// The Strict namespaces and relationship types, each beside its
/// Transitional twin.
const STRICT_TWINS: [(&str, &str); 16] = [
    (STRICT_MAIN, MAIN),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    ),
    (
        "http://purl.oclc.org/ooxml/drawingml/main",
        "http://schemas.openxmlformats.org/drawingml/2006/main",
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument",
        rel_types::OFFICE_DOCUMENT,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/worksheet",
        rel_types::WORKSHEET,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/chartsheet",
        rel_types::CHARTSHEET,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/dialogsheet",
        rel_types::DIALOGSHEET,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/sharedStrings",
        rel_types::SHARED_STRINGS,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/styles",
        rel_types::STYLES,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/theme",
        rel_types::THEME,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/calcChain",
        rel_types::CALC_CHAIN,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/comments",
        rel_types::COMMENTS,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/sheetMetadata",
        rel_types::SHEET_METADATA,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/table",
        rel_types::TABLE,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/pivotTable",
        rel_types::PIVOT_TABLE,
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships/extendedProperties",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties",
    ),
];

/// The Transitional twin of a Strict namespace or relationship type, or the
/// value itself.
pub fn transitional(value: &str) -> &str {
    STRICT_TWINS
        .iter()
        .find(|(strict, _)| *strict == value)
        .map_or(value, |(_, transitional)| transitional)
}

/// The Transitional relationship type `value` stands for.
pub fn transitional_relationship_type(value: &str) -> &str {
    transitional(value)
}

/// The Strict twin of a Transitional namespace or relationship type, or the
/// value itself.
pub fn strict(value: &str) -> &str {
    STRICT_TWINS
        .iter()
        .find(|(_, transitional)| *transitional == value)
        .map_or(value, |(strict, _)| strict)
}

/// Whether `value` is a Strict namespace or relationship type this table
/// knows.
pub fn is_strict(value: &str) -> bool {
    STRICT_TWINS.iter().any(|(strict, _)| *strict == value)
}

/// The main content types of a workbook package, and of the formats a
/// workbook is told from.
pub mod content_types {
    pub const WORKBOOK: &str =
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
    pub const WORKBOOK_MACRO_ENABLED: &str = "application/vnd.ms-excel.sheet.macroEnabled.main+xml";
    pub const TEMPLATE: &str =
        "application/vnd.openxmlformats-officedocument.spreadsheetml.template.main+xml";
    pub const TEMPLATE_MACRO_ENABLED: &str =
        "application/vnd.ms-excel.template.macroEnabled.main+xml";
    pub const BINARY_WORKBOOK: &str = "application/vnd.ms-excel.sheet.binary.macroEnabled.main";
    pub const WORKSHEET: &str =
        "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
    pub const SHARED_STRINGS: &str =
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml";
    pub const STYLES: &str =
        "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml";
    pub const CALC_CHAIN: &str =
        "application/vnd.openxmlformats-officedocument.spreadsheetml.calcChain+xml";
    pub const SHEET_METADATA: &str =
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strict_name_reads_as_its_transitional_twin() {
        assert_eq!(transitional(STRICT_MAIN), MAIN);
        assert_eq!(
            transitional_relationship_type(
                "http://purl.oclc.org/ooxml/officeDocument/relationships/worksheet"
            ),
            rel_types::WORKSHEET
        );
        assert_eq!(transitional(MAIN), MAIN);
        assert_eq!(strict(MAIN), STRICT_MAIN);
        assert_eq!(
            strict(rel_types::WORKSHEET),
            "http://purl.oclc.org/ooxml/officeDocument/relationships/worksheet"
        );
        assert_eq!(strict("urn:other"), "urn:other");
        assert!(is_relationships_namespace(RELATIONSHIPS));
        assert!(is_relationships_namespace(strict(RELATIONSHIPS)));
        assert!(!is_relationships_namespace(MAIN));
        assert!(is_strict(STRICT_MAIN) && !is_strict(MAIN));
    }
}
