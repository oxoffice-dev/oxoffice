//! A new workbook, and a sheet added to one.
//!
//! A new workbook is assembled at run time from the text files under
//! `assets/default/` (the workbook, one worksheet, the styles and the
//! theme), a package that holds one font (Calibri 11), the two fills Excel
//! always writes, one border, one cell format and the `Normal` style, under
//! the Office 2013-2022 theme. Nothing in it depends on the clock, the
//! machine or the order of a hash map, and the package writer sorts its
//! entries and stamps one ZIP date, so a build is the same bytes everywhere.

use oxoffice_core::app_properties::AppProperties;
use oxoffice_core::core_properties::CoreProperties;
use oxoffice_core::xml::{escape_attribute_value, is_xml_1_0_character};
use oxoffice_opc::OpcPackage;
use oxoffice_opc::content_types as opc_content_types;
use oxoffice_opc::relationship::rel_types as opc_rel_types;
use oxoffice_xlsx_oxml::app_properties::without_title_lists;
use oxoffice_xlsx_oxml::namespaces::{
    MAIN, RELATIONSHIPS, STRICT_MAIN, content_types, rel_types, strict,
    transitional_relationship_type,
};
use oxoffice_xlsx_oxml::xstring;

use crate::sheet::SheetEntry;
use crate::{Error, Result, Workbook, WorksheetMut};

const WORKBOOK: &str = include_str!("../assets/default/workbook.xml");
const STYLES: &str = include_str!("../assets/default/styles.xml");
const THEME: &str = include_str!("../assets/default/theme1.xml");
/// The first sheet of a new workbook, the one in view.
const FIRST_SHEET: &str = include_str!("../assets/default/sheet1.xml");
/// A sheet added to a workbook.
const SHEET: &str = include_str!("../assets/default/sheet.xml");

/// The `sheets` element of the workbook asset, which a new workbook with
/// other sheets replaces.
const SHEETS: &str = r#"<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>"#;

const DEFAULT_SHEET_NAME: &str = "Sheet1";

/// The longest sheet name, in the UTF-16 code units Excel counts.
const MAX_SHEET_NAME: usize = 31;

const WORKBOOK_PART: &str = "/xl/workbook.xml";

impl Workbook {
    /// A new workbook with one empty sheet, `Sheet1`.
    ///
    /// Calibri 11 under the Office 2013-2022 theme, with the cell formats
    /// of a workbook Excel starts. The same build gives the same bytes.
    pub fn new() -> Self {
        Self::with_sheet_names(&[DEFAULT_SHEET_NAME]).expect("the bundled workbook opens")
    }

    /// A new workbook with one empty sheet for each of `names`, in order.
    /// The names follow Excel's rules ([`Workbook::add_sheet`]); a workbook
    /// has at least one sheet.
    pub fn with_sheet_names<S: AsRef<str>>(names: &[S]) -> Result<Self> {
        if names.is_empty() {
            return Err(Error::InvalidArgument(
                "a workbook has at least one sheet".to_owned(),
            ));
        }
        for (position, name) in names.iter().enumerate() {
            check_sheet_name(
                name.as_ref(),
                names[..position].iter().map(|name| name.as_ref()),
            )?;
        }
        Self::from_package(default_package(names))
    }

    /// Adds an empty sheet after the others and gives it to edit.
    ///
    /// The name follows Excel's rules: 1 to 31 characters, none of
    /// `: \ / ? * [ ]`, no apostrophe at either end, not `History`, and not
    /// the name of another sheet in any case. The sheet is a new part with
    /// its relationship, content type and `sheet` record, the next
    /// `sheetId`; the workbook's `docProps/app.xml` loses the lists of
    /// sheet names that Excel writes again at its next save.
    pub fn add_sheet(&mut self, name: &str) -> Result<WorksheetMut<'_>> {
        check_sheet_name(name, self.sheets.iter().map(|sheet| sheet.name.as_str()))?;
        let is_strict = self.workbook.is_strict();
        let (directory, _) = self
            .workbook_part
            .rsplit_once('/')
            .unwrap_or(("", &self.workbook_part));
        let number = (1..)
            .find(|number| {
                !self
                    .package
                    .contains_part(&format!("{directory}/worksheets/sheet{number}.xml"))
            })
            .expect("a free sheet number");
        let part = format!("{directory}/worksheets/sheet{number}.xml");
        let target = format!("worksheets/sheet{number}.xml");

        // What can fail goes first: nothing changes until it has passed.
        let app = self.app_without_sheet_names();

        let rel_type = if is_strict {
            strict(rel_types::WORKSHEET)
        } else {
            rel_types::WORKSHEET
        };
        let relationship_id = self
            .package
            .get_or_create_part_rels(&self.workbook_part)
            .add(rel_type, &target);
        self.package
            .set_part(&part, sheet_template(SHEET, is_strict).into_bytes());
        self.package
            .content_types
            .add_override(&part, content_types::WORKSHEET);
        if let Some((app_part, bytes)) = app {
            self.package.set_part(&app_part, bytes);
        }
        let record = self.workbook.sheet_record(
            &xstring::encode(name),
            self.workbook.next_sheet_id(),
            &relationship_id,
        );
        self.workbook.push_sheet(record);
        self.sheets
            .push(SheetEntry::added(name, relationship_id, part));
        self.mark_edited();
        let position = self.sheets.len() - 1;
        Ok(WorksheetMut {
            workbook: self,
            position,
        })
    }

    /// The extended properties part and its bytes without the lists of
    /// sheet names, when the workbook has such a part that reads.
    fn app_without_sheet_names(&self) -> Option<(String, Vec<u8>)> {
        let relationship = self
            .package
            .package_rels
            .items
            .iter()
            .find(|relationship| {
                transitional_relationship_type(&relationship.rel_type)
                    == opc_rel_types::EXTENDED_PROPERTIES
                    && !crate::open::is_external(relationship)
            })?;
        let part = OpcPackage::resolve_rel_target("/", &relationship.target);
        let bytes = without_title_lists(self.package.get_part(&part)?).ok()?;
        Some((part, bytes))
    }
}

impl Default for Workbook {
    fn default() -> Self {
        Self::new()
    }
}

/// The text of a worksheet part for a package whose main namespace is
/// Transitional, or Strict.
fn sheet_template(template: &str, is_strict: bool) -> String {
    let template = template.trim_end();
    if is_strict {
        template
            .replace(MAIN, STRICT_MAIN)
            .replace(RELATIONSHIPS, strict(RELATIONSHIPS))
    } else {
        template.to_owned()
    }
}

/// The package of a new workbook with a sheet for each of `names`.
fn default_package(names: &[impl AsRef<str>]) -> OpcPackage {
    let mut package = OpcPackage::with_main_part("xl/workbook.xml", content_types::WORKBOOK);
    package
        .package_rels
        .add(opc_rel_types::CORE_PROPERTIES, "docProps/core.xml");
    package
        .package_rels
        .add(opc_rel_types::EXTENDED_PROPERTIES, "docProps/app.xml");

    let sheets: String = names
        .iter()
        .enumerate()
        .map(|(position, name)| {
            let name = escape_attribute_value(&xstring::encode(name.as_ref())).into_owned();
            let number = position + 1;
            format!(r#"<sheet name="{name}" sheetId="{number}" r:id="rId{number}"/>"#)
        })
        .collect();
    debug_assert!(WORKBOOK.contains(SHEETS), "the workbook asset lists Sheet1");
    let workbook = WORKBOOK
        .trim_end()
        .replacen(SHEETS, &format!("<sheets>{sheets}</sheets>"), 1);
    package.set_part(WORKBOOK_PART, workbook.into_bytes());

    for position in 0..names.len() {
        let part = format!("/xl/worksheets/sheet{}.xml", position + 1);
        let text = if position == 0 { FIRST_SHEET } else { SHEET };
        package.set_part(&part, sheet_template(text, false).into_bytes());
        package
            .content_types
            .add_override(&part, content_types::WORKSHEET);
    }
    // Excel numbers the theme and the styles after the sheets.
    let relationships = package.get_or_create_part_rels(WORKBOOK_PART);
    for position in 0..names.len() {
        let number = position + 1;
        relationships.add_with_id(
            &format!("rId{number}"),
            rel_types::WORKSHEET,
            &format!("worksheets/sheet{number}.xml"),
        );
    }
    relationships.add(rel_types::THEME, "theme/theme1.xml");
    relationships.add(rel_types::STYLES, "styles.xml");
    package.set_part("/xl/theme/theme1.xml", THEME.trim_end().as_bytes().to_vec());
    package.set_part("/xl/styles.xml", STYLES.trim_end().as_bytes().to_vec());
    package
        .content_types
        .add_override("/xl/theme/theme1.xml", opc_content_types::THEME);
    package
        .content_types
        .add_override("/xl/styles.xml", content_types::STYLES);

    package.set_part(
        "/docProps/core.xml",
        CoreProperties::default()
            .to_xml()
            .expect("empty core properties write"),
    );
    package
        .content_types
        .add_override("/docProps/core.xml", opc_content_types::CORE_PROPERTIES);
    package.set_part(
        "/docProps/app.xml",
        application_properties()
            .to_xml()
            .expect("application properties write"),
    );
    package
        .content_types
        .add_override("/docProps/app.xml", opc_content_types::EXTENDED_PROPERTIES);
    package
}

/// oxoffice, in the `XX.YYYY` form of its version that the schema asks of
/// `AppVersion`.
fn application_properties() -> AppProperties {
    let mut properties = AppProperties::default();
    properties.application = Some("oxoffice".to_owned());
    properties.application_version = Some(format!(
        "{:0>2}.{:0>4}",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR")
    ));
    properties
}

/// Refuses a sheet name Excel would not accept next to the sheets `others`.
fn check_sheet_name<'a>(name: &str, others: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let refuse = |reason: &str| {
        Err(Error::InvalidArgument(format!(
            "{name:?} is not a sheet name: {reason}"
        )))
    };
    if name.is_empty() {
        return refuse("it is empty");
    }
    if name.encode_utf16().count() > MAX_SHEET_NAME {
        return refuse(&format!("it is longer than {MAX_SHEET_NAME} characters"));
    }
    if let Some(character) = name
        .chars()
        .find(|character| matches!(character, ':' | '\\' | '/' | '?' | '*' | '[' | ']'))
    {
        return refuse(&format!("it holds {character:?}"));
    }
    if name
        .chars()
        .any(|c| c.is_control() || !is_xml_1_0_character(c))
    {
        return refuse("it holds a control character");
    }
    if name.starts_with('\'') || name.ends_with('\'') {
        return refuse("it starts or ends with an apostrophe");
    }
    let lower = name.to_lowercase();
    if lower == "history" {
        return refuse("Excel keeps History for its own use");
    }
    if others
        .into_iter()
        .any(|other| other.to_lowercase() == lower)
    {
        return Err(Error::Refused(format!(
            "a sheet named {name:?} exists already: Excel names its sheets apart whatever their case"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sheet_name_follows_the_rules_of_excel() {
        let long = "x".repeat(31);
        let too_long = "x".repeat(32);
        // 16 characters outside the first plane count twice.
        let wide_ok = "\u{1F600}".repeat(15);
        let wide_too_long = "\u{1F600}".repeat(16);
        let others = ["Data", "Q1 data"];
        for (name, refused) in [
            ("Summary", None),
            (long.as_str(), None),
            (wide_ok.as_str(), None),
            ("a'b", None),
            ("Q_x0031_", None),
            ("", Some("empty")),
            (too_long.as_str(), Some("longer than 31")),
            (wide_too_long.as_str(), Some("longer than 31")),
            ("a:b", Some("':'")),
            ("a\\b", Some("'\\\\'")),
            ("a/b", Some("'/'")),
            ("a?b", Some("'?'")),
            ("a*b", Some("'*'")),
            ("[a]", Some("'['")),
            ("a\nb", Some("control")),
            ("a\u{1}b", Some("control")),
            ("'a", Some("apostrophe")),
            ("a'", Some("apostrophe")),
            ("History", Some("History")),
            ("hISTORY", Some("History")),
            ("data", Some("exists already")),
            ("Q1 DATA", Some("exists already")),
        ] {
            match (check_sheet_name(name, others), refused) {
                (Ok(()), None) => {}
                (Err(error), Some(reason)) => {
                    assert!(error.to_string().contains(reason), "{name:?}: {error}");
                }
                (result, _) => panic!("{name:?}: {result:?}"),
            }
        }
    }

    #[test]
    fn the_workbook_asset_lists_the_sheet_the_new_workbook_replaces() {
        assert_eq!(WORKBOOK.matches(SHEETS).count(), 1);
    }
}
