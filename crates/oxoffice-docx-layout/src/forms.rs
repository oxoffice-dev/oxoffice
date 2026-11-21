//! Word content controls and legacy form fields as fillable form fields.
//!
//! The engine marks what it draws for a control with a [`FormMark`]: the
//! paragraph's source node and the control's position among the paragraph's
//! inline controls, [`BLOCK_FIELD`] for the block control around the
//! paragraph, or [`legacy_mark`] for a legacy form field. After pagination,
//! [`number_form_fields`] turns marks into [`FormFieldId`]s, in document
//! order, and builds the fields from the model.

use std::collections::HashMap;

use oxoffice_docx_oxml::content_control::{SdtContent, SdtFormPayload};
use oxoffice_docx_oxml::document::BodyContent;
use oxoffice_docx_oxml::table::{CT_Tbl, CellContent};
use oxoffice_docx_oxml::text::{
    CT_P, CT_R, Field, LegacyFormFieldValue, RunContent, SpecialCharacter,
};
use oxoffice_docx_oxml::{CT_Body, CT_Sdt};
use oxoffice_layout::{ChoiceOption, FieldAlignment, FormField, FormFieldKind};

/// The mark field value of the block control around a paragraph.
pub(crate) const BLOCK_FIELD: u32 = u32::MAX;

/// Mark field values from this bit up name a legacy form field: the bit plus
/// the field's index among the paragraph's fields.
const LEGACY_FIELD: u32 = 1 << 31;

/// The mark field value of the legacy form field at this index among the
/// paragraph's fields, in source order.
pub(crate) fn legacy_mark(index: u32) -> u32 {
    LEGACY_FIELD | index.min(BLOCK_FIELD - LEGACY_FIELD - 1)
}

/// Whether a field is a legacy form field that can be a field on its own:
/// `FORMTEXT`, `FORMCHECKBOX` or `FORMDROPDOWN` whose value reads.
pub(crate) fn is_mapped_legacy(field: &Field) -> bool {
    field.legacy_form.is_some() && field.legacy_form_value().is_ok()
}

/// Whether a run holds a legacy form field that maps.
fn run_holds_legacy(run: &CT_R) -> bool {
    run.content
        .iter()
        .any(|content| matches!(content, RunContent::Field(field) if is_mapped_legacy(field)))
}

/// Whether a tag marks a signature: `signature`, or `signature:` and a name,
/// ASCII case ignored.
pub(crate) fn is_signature_tag(tag: Option<&str>) -> bool {
    tag.is_some_and(|tag| {
        tag.eq_ignore_ascii_case("signature")
            || tag
                .get(..10)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("signature:"))
    })
}

/// Whether a control's kind can be a field on its own: text, list, date,
/// check box, or a signature-tagged text or picture control.
fn mappable_kind(control: &CT_Sdt) -> bool {
    let Some(properties) = control.properties.as_ref() else {
        return true;
    };
    match properties.form_facts().map(|facts| facts.payload) {
        Ok(
            SdtFormPayload::PlainText { .. }
            | SdtFormPayload::RichText
            | SdtFormPayload::CheckBox { .. }
            | SdtFormPayload::DropDownList { .. }
            | SdtFormPayload::ComboBox { .. }
            | SdtFormPayload::Date { .. },
        ) => true,
        Ok(SdtFormPayload::Picture) => is_signature_tag(properties.tag.as_deref()),
        // Containers, and kinds this crate does not know yet, stay static.
        _ => false,
    }
}

/// Why a control of a field's kind stays static because its properties do
/// not decode: a field from half-read facts could be wrong.
fn facts_fault(control: &CT_Sdt) -> Option<String> {
    use oxoffice_docx_oxml::content_control::SdtType;
    let properties = control.properties.as_ref()?;
    let field_kind = match properties.control_type {
        None
        | Some(
            SdtType::RichText
            | SdtType::PlainText
            | SdtType::CheckBox
            | SdtType::ComboBox
            | SdtType::DropDownList
            | SdtType::Date,
        ) => true,
        Some(SdtType::Picture) => is_signature_tag(properties.tag.as_deref()),
        Some(_) => false,
    };
    let error = properties.form_facts().err()?;
    field_kind.then(|| {
        let name = properties
            .tag
            .clone()
            .or_else(|| properties.alias.clone())
            .unwrap_or_else(|| "untitled".to_owned());
        format!("form control {name:?} stays static: its properties do not decode ({error})")
    })
}

/// A control that holds other mappable controls, or legacy form fields, is a
/// container.
fn holds_mappable(control: &CT_Sdt) -> bool {
    fn revision_holds(revision: &oxoffice_docx_oxml::revision::CT_Revision) -> bool {
        !deletes(revision) && revision.content_paragraph().is_some_and(paragraph_holds)
    }
    fn paragraph_holds(paragraph: &CT_P) -> bool {
        paragraph
            .content_controls
            .iter()
            .any(|(_, _, _, nested)| mappable_kind(nested) || holds_mappable(nested))
            || paragraph
                .revisions
                .iter()
                .any(|(_, _, revision)| revision_holds(revision))
            || crate::engine::accepted_runs(paragraph)
                .into_iter()
                .any(|(run, _)| run_holds_legacy(run))
    }
    fn table_holds(table: &CT_Tbl) -> bool {
        !table.content_controls.is_empty()
            || table.rows.iter().any(|row| {
                !row.content_controls.is_empty()
                    || row.cells.iter().any(|cell| {
                        cell.content.iter().any(|content| match content {
                            CellContent::Paragraph(paragraph) => paragraph_holds(paragraph),
                            _ => true,
                        })
                    })
            })
    }
    control.content.iter().any(|content| match content {
        SdtContent::ContentControl(nested) => mappable_kind(nested) || holds_mappable(nested),
        SdtContent::Paragraph(paragraph) => paragraph_holds(paragraph),
        SdtContent::Table(table) => table_holds(table),
        SdtContent::Row(_) | SdtContent::Cell(_) => true,
        SdtContent::Run(run) => run_holds_legacy(run),
        SdtContent::RawXml(_) => false,
    }) || control
        .revisions()
        .iter()
        .any(|(_, revision)| revision_holds(revision))
}

/// Whether a control holds paragraphs only, around unmapped wrappers and
/// raw markers such as a block-level bookmark, which draw nothing.
fn paragraphs_only(control: &CT_Sdt) -> bool {
    control.content.iter().all(|content| match content {
        SdtContent::Paragraph(_) | SdtContent::RawXml(_) => true,
        SdtContent::ContentControl(nested) => !mappable_kind(nested) && paragraphs_only(nested),
        _ => false,
    })
}

/// Whether a block control is a field: a mappable kind, paragraphs only, no
/// mappable control inside.
pub(crate) fn block_is_field(control: &CT_Sdt) -> bool {
    mappable_kind(control) && !holds_mappable(control) && paragraphs_only(control)
}

/// Whether a control holds a floating picture, which is not a line item:
/// such a control stays static.
fn holds_floating_picture(control: &CT_Sdt) -> bool {
    fn floats(run: &CT_R) -> bool {
        run.content.iter().any(
            |content| matches!(content, RunContent::Drawing(drawing) if drawing.anchor.is_some()),
        ) || run
            .alt_drawings
            .iter()
            .any(|drawing| drawing.anchor.is_some())
    }
    control.content.iter().any(|content| match content {
        SdtContent::Run(run) => floats(run),
        SdtContent::Paragraph(paragraph) => paragraph.source_runs().into_iter().any(floats),
        SdtContent::ContentControl(nested) => holds_floating_picture(nested),
        _ => false,
    })
}

/// A control's name in a diagnostic: its tag, else its title.
fn control_name(control: &CT_Sdt) -> String {
    control
        .properties
        .as_ref()
        .and_then(|properties| {
            properties
                .tag
                .clone()
                .filter(|tag| !tag.is_empty())
                .or_else(|| properties.alias.clone().filter(|alias| !alias.is_empty()))
        })
        .unwrap_or_else(|| "untitled".to_owned())
}

/// The inline controls of one paragraph, in preorder, with their parents.
pub(crate) struct InlineControls<'a> {
    order: Vec<&'a CT_Sdt>,
    index: HashMap<*const CT_Sdt, u32>,
    parent: HashMap<*const CT_Sdt, &'a CT_Sdt>,
    /// Controls a tracked deletion or move removes from the accepted view.
    deleted: Vec<&'a CT_Sdt>,
}

/// Whether the accepted view drops what a tracked change holds.
fn deletes(revision: &oxoffice_docx_oxml::revision::CT_Revision) -> bool {
    use oxoffice_docx_oxml::revision::RevisionKind;
    matches!(
        revision.kind(),
        RevisionKind::Deletion | RevisionKind::MoveFrom
    )
}

impl<'a> InlineControls<'a> {
    pub(crate) fn new(paragraph: &'a CT_P) -> Self {
        let mut controls = Self {
            order: Vec::new(),
            index: HashMap::new(),
            parent: HashMap::new(),
            deleted: Vec::new(),
        };
        controls.paragraph(paragraph, None);
        controls
    }

    /// A paragraph's controls, those inside its tracked insertions and
    /// moves included, in document order: by run boundary, then by the raw
    /// children before them, as the engine draws them (a tracked change
    /// inside a hyperlink orders by the hyperlink).
    fn paragraph(&mut self, paragraph: &'a CT_P, parent: Option<&'a CT_Sdt>) {
        use crate::engine::RawOrder;
        enum Owner<'a> {
            Control(&'a CT_Sdt),
            Revision(&'a oxoffice_docx_oxml::revision::CT_Revision),
        }
        let mut owners: Vec<((usize, RawOrder, u8), Owner<'a>)> = paragraph
            .content_controls
            .iter()
            .map(|(at, raw, _, control)| ((*at, RawOrder::Raw(*raw), 0), Owner::Control(control)))
            .chain(paragraph.revisions.iter().map(|(at, slot, revision)| {
                (
                    (
                        *at,
                        crate::engine::paragraph_revision_raw_order(paragraph, *at, *slot),
                        1,
                    ),
                    Owner::Revision(revision),
                )
            }))
            .collect();
        owners.sort_by_key(|(at, _)| *at);
        for (_, owner) in owners {
            match owner {
                Owner::Control(control) => self.visit(control, parent),
                Owner::Revision(revision) => self.revision(revision, parent),
            }
        }
    }

    fn revision(
        &mut self,
        revision: &'a oxoffice_docx_oxml::revision::CT_Revision,
        parent: Option<&'a CT_Sdt>,
    ) {
        let Some(paragraph) = revision.content_paragraph() else {
            return;
        };
        if deletes(revision) {
            let mut dropped = InlineControls::new(paragraph);
            self.deleted.append(&mut dropped.order);
            self.deleted.append(&mut dropped.deleted);
        } else {
            self.paragraph(paragraph, parent);
        }
    }

    fn visit(&mut self, control: &'a CT_Sdt, parent: Option<&'a CT_Sdt>) {
        let position = u32::try_from(self.order.len()).unwrap_or(u32::MAX - 1);
        self.index.insert(std::ptr::from_ref(control), position);
        self.order.push(control);
        if let Some(parent) = parent {
            self.parent.insert(std::ptr::from_ref(control), parent);
        }
        // By content boundary, the tracked changes at a boundary before the
        // content there, as the engine draws them.
        for boundary in 0..=control.content.len() {
            for (_, revision) in control.revisions().iter().filter(|(at, _)| *at == boundary) {
                self.revision(revision, Some(control));
            }
            match control.content.get(boundary) {
                Some(SdtContent::ContentControl(nested)) => self.visit(nested, Some(control)),
                Some(SdtContent::Paragraph(paragraph)) => self.paragraph(paragraph, Some(control)),
                _ => {}
            }
        }
    }

    /// The mark of a run whose innermost control is `innermost`: the nearest
    /// mappable control around it, unless that one is a container.
    pub(crate) fn mark_of(&self, innermost: &CT_Sdt) -> Option<u32> {
        let mut current = innermost;
        loop {
            if mappable_kind(current) {
                return (!holds_mappable(current))
                    .then(|| self.index.get(&std::ptr::from_ref(current)).copied())
                    .flatten();
            }
            current = self.parent.get(&std::ptr::from_ref(current))?;
        }
    }

    /// Whether `control` is `ancestor` or sits inside it.
    fn within(&self, control: &CT_Sdt, ancestor: &CT_Sdt) -> bool {
        let mut current = control;
        loop {
            if std::ptr::eq(current, ancestor) {
                return true;
            }
            match self.parent.get(&std::ptr::from_ref(current)) {
                Some(parent) => current = parent,
                None => return false,
            }
        }
    }
}

/// Appends the text a run shows. A line feed inside a text node shows as a
/// space, as Word and the layout draw it; a break is a line break.
fn push_shown_text(run: &CT_R, text: &mut String) {
    for content in &run.content {
        match content {
            RunContent::Text(shown) => text.extend(shown.text.chars().map(|character| {
                if matches!(character, '\n' | '\r') {
                    ' '
                } else {
                    character
                }
            })),
            RunContent::Tab => text.push('\t'),
            RunContent::Break(_)
            | RunContent::SpecialCharacter(SpecialCharacter::CarriageReturn) => text.push('\n'),
            RunContent::Field(field) => {
                if let Some(projected) = field.effective_projected_text() {
                    text.push_str(&projected);
                }
            }
            _ => {}
        }
    }
}

/// The text a paragraph's runs show in the accepted view, the view the page
/// draws, kept to the runs whose innermost control `keep` takes.
fn shown_text<'p>(paragraph: &'p CT_P, mut keep: impl FnMut(Option<&'p CT_Sdt>) -> bool) -> String {
    let mut text = String::new();
    for (run, control) in crate::engine::accepted_runs(paragraph) {
        if keep(control) {
            push_shown_text(run, &mut text);
        }
    }
    text
}

/// What a block control shows: its paragraphs, through unmapped wrappers,
/// joined by line breaks.
fn block_text(control: &CT_Sdt) -> String {
    fn paragraphs(control: &CT_Sdt, out: &mut Vec<String>) {
        for content in &control.content {
            match content {
                SdtContent::Paragraph(paragraph) => out.push(shown_text(paragraph, |_| true)),
                SdtContent::ContentControl(nested) => paragraphs(nested, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    paragraphs(control, &mut out);
    out.join("\n")
}

/// The field a control becomes, its name not yet made unique. `shown` is
/// the text the control shows in the accepted view.
pub(crate) fn control_field(control: &CT_Sdt, shown: String, block: bool) -> Option<FormField> {
    let properties = control.properties.clone().unwrap_or_default();
    let facts = properties.form_facts().ok()?;
    let placeholder = facts.showing_placeholder;
    let value = if placeholder {
        String::new()
    } else {
        shown.clone()
    };
    let signature = is_signature_tag(properties.tag.as_deref());
    let kind = if signature {
        FormFieldKind::Signature
    } else {
        let editable = matches!(facts.payload, SdtFormPayload::ComboBox { .. });
        match facts.payload {
            SdtFormPayload::PlainText { multiline } => FormFieldKind::Text {
                multiline: multiline || value.contains('\n'),
                value,
                max_len: None,
            },
            SdtFormPayload::RichText | SdtFormPayload::Date { .. } => FormFieldKind::Text {
                multiline: block && value.contains('\n'),
                value,
                max_len: None,
            },
            SdtFormPayload::CheckBox { checked, .. } => FormFieldKind::CheckBox { checked },
            SdtFormPayload::DropDownList { items } | SdtFormPayload::ComboBox { items } => {
                let selected = items
                    .iter()
                    .find(|item| item.display == shown)
                    .map(|item| item.value.clone());
                FormFieldKind::Choice {
                    options: items
                        .into_iter()
                        .filter(|item| !item.value.is_empty() || !item.display.is_empty())
                        .map(|item| ChoiceOption {
                            value: item.value,
                            display: item.display,
                        })
                        .collect(),
                    value: if placeholder {
                        None
                    } else {
                        selected.or_else(|| (!shown.is_empty()).then_some(shown.clone()))
                    },
                    editable,
                }
            }
            _ => return None,
        }
    };
    let kind_name = match &kind {
        FormFieldKind::Text { .. } => "text",
        FormFieldKind::CheckBox { .. } => "checkbox",
        FormFieldKind::Choice { .. } => "list",
        FormFieldKind::Signature => "signature",
    };
    let name = properties
        .tag
        .clone()
        .filter(|tag| !tag.is_empty())
        .or_else(|| properties.alias.clone().filter(|alias| !alias.is_empty()))
        .unwrap_or_else(|| kind_name.to_owned());
    let tooltip = properties
        .alias
        .clone()
        .filter(|alias| !alias.is_empty())
        .or_else(|| (placeholder && !shown.is_empty()).then_some(shown));
    Some(FormField {
        name,
        tooltip,
        kind,
        read_only: facts.lock.content_locked(),
        font_size: 0.0,
        color: oxoffice_layout::Color::BLACK,
        alignment: FieldAlignment::Left,
        placeholder,
    })
}

/// The field a legacy form field becomes, its name not yet made unique.
fn legacy_field(field: &Field) -> Option<FormField> {
    use oxoffice_docx_oxml::text::{LegacyFormInfoText, LegacyTextInputType};
    let data = field.legacy_form.as_ref()?;
    let kind = match field.legacy_form_value().ok()?? {
        LegacyFormFieldValue::Text(text) => {
            // Word shows an empty text field as five en spaces.
            let value = if text.chars().all(|character| character == '\u{2002}') {
                String::new()
            } else {
                text
            };
            FormFieldKind::Text {
                multiline: value.contains('\n'),
                value,
                max_len: data
                    .max_length
                    .filter(|length| *length > 0)
                    .and_then(|length| u32::try_from(length).ok()),
            }
        }
        LegacyFormFieldValue::Checked(checked) => FormFieldKind::CheckBox { checked },
        LegacyFormFieldValue::SelectedIndex(index) => FormFieldKind::Choice {
            options: data
                .choices
                .iter()
                .map(|choice| ChoiceOption {
                    value: choice.clone(),
                    display: choice.clone(),
                })
                .collect(),
            value: data.choices.get(index).cloned(),
            editable: false,
        },
    };
    let kind_name = match &kind {
        FormFieldKind::CheckBox { .. } => "checkbox",
        FormFieldKind::Choice { .. } => "list",
        _ => "text",
    };
    let computed = matches!(
        data.text_input_type,
        Some(
            LegacyTextInputType::CurrentDate
                | LegacyTextInputType::CurrentTime
                | LegacyTextInputType::Calculated
        )
    );
    Some(FormField {
        name: data
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| kind_name.to_owned()),
        tooltip: match &data.help_text {
            Some(LegacyFormInfoText::Text(text)) if !text.is_empty() => Some(text.clone()),
            _ => None,
        },
        kind,
        read_only: !data.enabled || computed,
        font_size: 0.0,
        color: oxoffice_layout::Color::BLACK,
        alignment: FieldAlignment::Left,
        placeholder: false,
    })
}

/// The block content control around a body path, if it is a field.
pub(crate) fn enclosing_block_field<'a>(
    body: &'a CT_Body,
    path: &[usize],
    known: &mut HashMap<*const CT_Sdt, bool>,
) -> Option<&'a CT_Sdt> {
    // Whether a control is a block field walks all its content: once per
    // control, not once per paragraph inside it.
    let mut is_field = |control: &CT_Sdt| {
        *known
            .entry(std::ptr::from_ref(control))
            .or_insert_with(|| block_is_field(control))
    };
    let BodyContent::ContentControl(outer) = body.content.get(*path.first()?)? else {
        return None;
    };
    let mut control: &CT_Sdt = outer;
    let mut field = is_field(control).then_some(control);
    for index in &path[1..] {
        match control.content.get(*index) {
            Some(SdtContent::ContentControl(nested)) => {
                control = nested;
                if is_field(control) {
                    field = Some(control);
                }
            }
            _ => break,
        }
    }
    field
}

/// Makes field names unique in document order: `.` becomes `_`, an empty
/// name `field`, and a name already used takes the next free suffix.
pub(crate) fn unique_names(names: &[String]) -> Vec<String> {
    let repaired: Vec<String> = names
        .iter()
        .map(|name| {
            if name.is_empty() {
                "field".to_owned()
            } else {
                name.replace('.', "_")
            }
        })
        .collect();
    let held: std::collections::HashSet<&str> = repaired.iter().map(String::as_str).collect();
    let mut used = std::collections::HashSet::new();
    repaired
        .iter()
        .map(|name| {
            let candidate = if used.contains(name) {
                (2..)
                    .map(|suffix| format!("{name}_{suffix}"))
                    .find(|candidate| {
                        !used.contains(candidate) && !held.contains(candidate.as_str())
                    })
                    .expect("an unbounded suffix range finds a free name")
            } else {
                name.clone()
            };
            used.insert(candidate.clone());
            candidate
        })
        .collect()
}

/// Why a control in a vertical section stays static.
const VERTICAL_SECTION: &str = "its section's text is vertical";

/// Why a control in a rotated cell stays static.
const ROTATED_CELL: &str = "its cell's text is rotated";

/// Whether each body layout item lies in a section of vertical text, each
/// item in its own section as layout reads it.
fn vertical_sections(
    body: &CT_Body,
    items: &[crate::engine::MainStoryLayoutItem<'_>],
) -> Vec<bool> {
    let letter;
    let last = match body.sect_pr.as_ref() {
        Some(section) => section,
        None => {
            letter = oxoffice_docx_oxml::CT_SectPr::default_letter();
            &letter
        }
    };
    crate::engine::main_story_item_sections(items, last)
        .into_iter()
        .map(|section| {
            section
                .text_direction
                .as_deref()
                .is_some_and(|direction| direction != "lrTb")
        })
        .collect()
}

/// What a field comes from.
enum Source<'a> {
    Control(&'a CT_Sdt),
    Legacy,
}

/// One mapped control or legacy form field of the main body, in document
/// order.
struct Entry<'a> {
    source: Source<'a>,
    field: FormField,
}

impl Entry<'_> {
    /// How a diagnostic names the entry: a legacy form field is a "form
    /// field" everywhere, a content control a "form control".
    fn kind(&self) -> &'static str {
        match self.source {
            Source::Control(_) => "form control",
            Source::Legacy => "form field",
        }
    }

    /// The symbol a check box draws in its other state.
    fn other_symbol(&self, checked: bool) -> Option<char> {
        match self.source {
            Source::Control(control) => control
                .properties
                .as_ref()
                .and_then(|properties| properties.form_facts().ok())
                .and_then(|facts| match facts.payload {
                    SdtFormPayload::CheckBox {
                        checked_symbol,
                        unchecked_symbol,
                        ..
                    } => Some(if checked {
                        unchecked_symbol.0
                    } else {
                        checked_symbol.0
                    }),
                    _ => None,
                }),
            // As the layout draws a legacy check box: ☒ checked, ☐ not.
            Source::Legacy => Some(if checked { '\u{2610}' } else { '\u{2612}' }),
        }
    }
}

/// An inline field of a paragraph, before document order sorts them.
enum Inline<'a> {
    Control(u32, &'a CT_Sdt),
    Legacy(u32, &'a Field),
}

/// The mapped controls and legacy form fields of the main body in document
/// order, and the (paragraph node, mark) keys that name each one.
struct Inventory<'a> {
    entries: Vec<Entry<'a>>,
    keys: HashMap<oxoffice_layout::FormMark, usize>,
    by_control: HashMap<*const CT_Sdt, usize>,
    /// Controls of a field's kind whose facts do not decode, and legacy form
    /// fields that stay static, said once each.
    faults: Vec<String>,
    faulted: std::collections::HashSet<*const CT_Sdt>,
    /// Every main-story paragraph visited, by source node.
    paragraphs: HashMap<oxoffice_layout::SourceNodeId, FormParagraph>,
    /// Controls already said to stay static for their place.
    said: std::collections::HashSet<*const CT_Sdt>,
    /// Whether a block control is a field, by control.
    block_fields: HashMap<*const CT_Sdt, bool>,
    /// Controls that make no field.
    refused: std::collections::HashSet<*const CT_Sdt>,
}

impl<'a> Inventory<'a> {
    fn new(input: &'a crate::input::LayoutInput, sources: &crate::engine::SourceRegistry) -> Self {
        let mut inventory = Self {
            entries: Vec::new(),
            keys: HashMap::new(),
            by_control: HashMap::new(),
            faults: Vec::new(),
            faulted: std::collections::HashSet::new(),
            paragraphs: HashMap::new(),
            said: std::collections::HashSet::new(),
            block_fields: HashMap::new(),
            refused: std::collections::HashSet::new(),
        };
        let body = &input.document.body;
        let items = crate::engine::body_layout_items(body);
        let vertical = vertical_sections(body, &items);
        // A block field with any paragraph in a vertical section stays
        // static as a whole: one widget cannot hold half rotated text.
        let vertical_blocks: std::collections::HashSet<*const CT_Sdt> = items
            .iter()
            .zip(&vertical)
            .filter(|(_, vertical)| **vertical)
            .filter_map(|(item, _)| match item {
                crate::engine::MainStoryLayoutItem::Paragraph(_, path) => {
                    enclosing_block_field(body, path, &mut inventory.block_fields)
                        .map(std::ptr::from_ref)
                }
                crate::engine::MainStoryLayoutItem::Table(..) => None,
            })
            .collect();
        for (index, item) in items.into_iter().enumerate() {
            match item {
                crate::engine::MainStoryLayoutItem::Paragraph(paragraph, path) => {
                    let node = if path.len() == 1 {
                        sources.body_id(path[0])
                    } else {
                        sources.id(&crate::WordStory::Document, &path)
                    };
                    let block = enclosing_block_field(body, &path, &mut inventory.block_fields);
                    inventory.note_block_faults(body, &path);
                    let in_vertical_block = block
                        .is_some_and(|block| vertical_blocks.contains(&std::ptr::from_ref(block)));
                    let reason = (vertical[index] || in_vertical_block).then_some(VERTICAL_SECTION);
                    inventory.paragraph(input, paragraph, node, block, reason);
                }
                crate::engine::MainStoryLayoutItem::Table(table, path) => {
                    let reason = vertical[index].then_some(VERTICAL_SECTION);
                    inventory.table(input, sources, table, &path, reason);
                }
            }
        }
        inventory
    }

    fn table(
        &mut self,
        input: &'a crate::input::LayoutInput,
        sources: &crate::engine::SourceRegistry,
        table: &'a CT_Tbl,
        path: &[usize],
        reason: Option<&'static str>,
    ) {
        for (row, row_path) in crate::table::layout_table_rows(table, path) {
            for (cell, cell_path) in crate::table::layout_row_cells(row, &row_path) {
                let rotated = cell
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.text_direction.as_deref())
                    .is_some_and(|direction| {
                        crate::table::text_direction_rotation(direction).is_some()
                            || crate::table::text_direction_stacks_upright(direction)
                    });
                let reason = reason.or(rotated.then_some(ROTATED_CELL));
                for (index, content) in cell.content.iter().enumerate() {
                    let mut content_path = cell_path.clone();
                    content_path.push(index);
                    match content {
                        CellContent::Paragraph(paragraph) => {
                            let node = sources.id(&crate::WordStory::Document, &content_path);
                            self.paragraph(input, paragraph, node, None, reason);
                        }
                        CellContent::ContentControl(control) => {
                            self.cell_control(input, sources, control, &content_path, None, reason);
                        }
                        CellContent::Table(nested) => {
                            self.table(input, sources, nested, &content_path, reason);
                        }
                    }
                }
            }
        }
    }

    /// A control at a cell's block level and what it holds, as table layout
    /// walks it: the innermost block control field encloses each paragraph.
    fn cell_control(
        &mut self,
        input: &'a crate::input::LayoutInput,
        sources: &crate::engine::SourceRegistry,
        control: &'a CT_Sdt,
        path: &[usize],
        outer: Option<&'a CT_Sdt>,
        reason: Option<&'static str>,
    ) {
        self.note_fault(control);
        let block = if block_is_field(control) {
            Some(control)
        } else {
            outer
        };
        for (inner, item) in control.content.iter().enumerate() {
            let mut inner_path = path.to_vec();
            inner_path.push(inner);
            match item {
                SdtContent::Paragraph(paragraph) => {
                    let node = sources.id(&crate::WordStory::Document, &inner_path);
                    self.paragraph(input, paragraph, node, block, reason);
                }
                SdtContent::ContentControl(nested) => {
                    self.cell_control(input, sources, nested, &inner_path, block, reason);
                }
                SdtContent::Table(table) => {
                    self.table(input, sources, table, &inner_path, reason);
                }
                _ => {}
            }
        }
    }

    /// Says once why a control that would be a field stays static.
    fn note_static(&mut self, control: &CT_Sdt, reason: &str) {
        if self.said.insert(std::ptr::from_ref(control)) {
            self.faults.push(format!(
                "form control {:?} stays static: {reason}",
                control_name(control)
            ));
        }
    }

    fn paragraph(
        &mut self,
        input: &'a crate::input::LayoutInput,
        paragraph: &'a CT_P,
        node: Option<oxoffice_layout::SourceNodeId>,
        block: Option<&'a CT_Sdt>,
        reason: Option<&'static str>,
    ) {
        let alignment = paragraph_alignment(input, paragraph);
        if let Some(node) = node {
            self.paragraphs.insert(
                node,
                FormParagraph {
                    node,
                    block: block.is_some() && reason.is_none(),
                },
            );
        }
        if let Some(control) = block {
            if let Some(reason) = reason {
                self.note_static(control, reason);
                return;
            }
            let index = self.add(control, true, alignment, || block_text(control));
            if let (Some(index), Some(node)) = (index, node) {
                self.keys.insert(
                    oxoffice_layout::FormMark {
                        node,
                        field: BLOCK_FIELD,
                    },
                    index,
                );
            }
            return;
        }
        let controls = InlineControls::new(paragraph);
        for control in &controls.order {
            self.note_fault(control);
        }
        for control in &controls.deleted {
            if mappable_kind(control) && !holds_mappable(control) {
                self.note_static(control, "a tracked change deletes it");
            }
        }
        // Document order: by the first physical run each field draws.
        let runs = paragraph.source_runs();
        let position: HashMap<*const CT_R, usize> = runs
            .iter()
            .enumerate()
            .map(|(position, run)| (std::ptr::from_ref(*run), position))
            .collect();
        // A control of the paragraph that holds no run at all orders by its
        // own boundary: before the run that follows it.
        let boundary: HashMap<*const CT_Sdt, usize> = paragraph
            .content_controls
            .iter()
            .map(|(at, _, _, control)| {
                let after = paragraph
                    .runs
                    .get(*at)
                    .and_then(|run| position.get(&std::ptr::from_ref(run)).copied())
                    .unwrap_or(runs.len());
                (std::ptr::from_ref(control), after)
            })
            .collect();
        let mut inline = Vec::new();
        let mut last = 0;
        for (ordinal, control) in controls.order.iter().enumerate() {
            let ordinal = u32::try_from(ordinal).unwrap_or(u32::MAX - 1);
            last = first_run(control, &position)
                .or_else(|| boundary.get(&std::ptr::from_ref(*control)).copied())
                .unwrap_or(last);
            if controls.mark_of(control) == Some(ordinal) {
                inline.push((last, Inline::Control(ordinal, control)));
            }
        }
        let fields = crate::engine::source_fields(paragraph);
        let field_index: HashMap<*const Field, u32> = fields
            .iter()
            .map(|field| std::ptr::from_ref(*field))
            .zip(0u32..)
            .collect();
        // A field in a tracked deletion is not drawn: it says so, as a
        // deleted control does, and takes no name.
        let accepted: std::collections::HashSet<*const CT_R> =
            crate::engine::accepted_runs(paragraph)
                .into_iter()
                .map(|(run, _)| std::ptr::from_ref(run))
                .collect();
        let mut top_level = std::collections::HashSet::new();
        for (at, run) in runs.iter().enumerate() {
            let deleted = !accepted.contains(&std::ptr::from_ref(*run));
            for content in &run.content {
                let RunContent::Field(field) = content else {
                    continue;
                };
                top_level.insert(std::ptr::from_ref(field));
                if field.legacy_form.is_none() {
                    // Form data that does not read leaves no legacy form.
                    if !deleted && field.validate_legacy_form_owner().is_err() {
                        self.faults.push(format!(
                            "form field {:?} stays static: its form data does not read",
                            legacy_name(field)
                        ));
                    }
                    continue;
                }
                if deleted {
                    self.faults.push(format!(
                        "form field {:?} stays static: a tracked change deletes it",
                        legacy_name(field)
                    ));
                    continue;
                }
                match field.legacy_form_value() {
                    Ok(_) => {
                        if let Some(index) = field_index.get(&std::ptr::from_ref(field)) {
                            inline.push((at, Inline::Legacy(*index, field)));
                        }
                    }
                    Err(error) => self.faults.push(format!(
                        "form field {:?} stays static: its form data does not read ({error})",
                        legacy_name(field)
                    )),
                }
            }
        }
        for field in &fields {
            if field.legacy_form.is_some() && !top_level.contains(&std::ptr::from_ref(*field)) {
                self.faults.push(format!(
                    "form field {:?} stays static: it sits inside another field",
                    legacy_name(field)
                ));
            }
        }
        inline.sort_by_key(|(at, _)| *at);
        for (_, item) in inline {
            if let Some(reason) = reason {
                match item {
                    Inline::Control(_, control) => self.note_static(control, reason),
                    Inline::Legacy(_, field) => self.faults.push(format!(
                        "form field {:?} stays static: {reason}",
                        legacy_name(field)
                    )),
                }
                continue;
            }
            let (index, field) = match item {
                Inline::Control(ordinal, control) => (
                    self.add(control, false, alignment, || {
                        shown_text(paragraph, |run_control| {
                            run_control
                                .is_some_and(|run_control| controls.within(run_control, control))
                        })
                    }),
                    ordinal,
                ),
                Inline::Legacy(index, field) => (
                    legacy_field(field).map(|mut built| {
                        built.alignment = alignment;
                        self.entries.push(Entry {
                            source: Source::Legacy,
                            field: built,
                        });
                        self.entries.len() - 1
                    }),
                    legacy_mark(index),
                ),
            };
            if let (Some(index), Some(node)) = (index, node) {
                self.keys
                    .insert(oxoffice_layout::FormMark { node, field }, index);
            }
        }
    }

    fn note_fault(&mut self, control: &CT_Sdt) {
        if self.faulted.insert(std::ptr::from_ref(control))
            && let Some(fault) = facts_fault(control)
        {
            self.faults.push(fault);
        }
    }

    /// The faults of the block controls a body path goes through.
    fn note_block_faults(&mut self, body: &CT_Body, path: &[usize]) {
        let Some(BodyContent::ContentControl(outer)) =
            path.first().and_then(|index| body.content.get(*index))
        else {
            return;
        };
        let mut control: &CT_Sdt = outer;
        self.note_fault(control);
        for index in &path[1..] {
            match control.content.get(*index) {
                Some(SdtContent::ContentControl(nested)) => {
                    control = nested;
                    self.note_fault(control);
                }
                _ => break,
            }
        }
    }

    fn add(
        &mut self,
        control: &'a CT_Sdt,
        block: bool,
        alignment: FieldAlignment,
        shown: impl FnOnce() -> String,
    ) -> Option<usize> {
        if let Some(index) = self.by_control.get(&std::ptr::from_ref(control)) {
            return Some(*index);
        }
        // Each paragraph of a block control asks again: a refusal is
        // decided once, not once per paragraph.
        if self.refused.contains(&std::ptr::from_ref(control)) {
            return None;
        }
        if holds_floating_picture(control) {
            self.refused.insert(std::ptr::from_ref(control));
            self.note_static(control, "it holds a floating picture");
            return None;
        }
        let Some(mut field) = control_field(control, shown(), block) else {
            self.refused.insert(std::ptr::from_ref(control));
            return None;
        };
        field.alignment = alignment;
        self.entries.push(Entry {
            source: Source::Control(control),
            field,
        });
        let index = self.entries.len() - 1;
        self.by_control.insert(std::ptr::from_ref(control), index);
        Some(index)
    }
}

/// Where a control's first physical run sits among the paragraph's runs,
/// the runs of its tracked changes included: a control whose own text sits
/// only in a tracked insertion still orders by that text.
fn first_run(control: &CT_Sdt, position: &HashMap<*const CT_R, usize>) -> Option<usize> {
    let at = |run: &CT_R| position.get(&std::ptr::from_ref(run)).copied();
    let content = control.content.iter().filter_map(|content| match content {
        SdtContent::Run(run) => at(run),
        SdtContent::ContentControl(nested) => first_run(nested, position),
        SdtContent::Paragraph(paragraph) => {
            paragraph.source_runs().into_iter().filter_map(at).min()
        }
        _ => None,
    });
    let revisions = control
        .revisions()
        .iter()
        .filter_map(|(_, revision)| revision.content_paragraph())
        .filter_map(|paragraph| paragraph.source_runs().into_iter().filter_map(at).min());
    content.chain(revisions).min()
}

/// A legacy form field's name for a diagnostic.
fn legacy_name(field: &Field) -> String {
    field
        .legacy_form
        .as_ref()
        .and_then(|data| data.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "untitled".to_owned())
}

/// A main-story paragraph as the form marks see it: the source node marks
/// name, and whether a block control field encloses it.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormParagraph {
    pub node: oxoffice_layout::SourceNodeId,
    pub block: bool,
}

/// The main-story paragraphs of one layout, by source node in the registry
/// the inventory used.
#[doc(hidden)]
#[derive(Clone, Debug, Default)]
pub struct FormParagraphs {
    /// The address of that registry. A table laid out with another, a text
    /// box's own, draws no marks.
    registry: usize,
    nodes: HashMap<oxoffice_layout::SourceNodeId, FormParagraph>,
}

/// The main-story paragraphs of an input, by source node.
pub(crate) fn form_paragraphs(
    input: &crate::input::LayoutInput,
    sources: &crate::engine::SourceRegistry,
) -> FormParagraphs {
    FormParagraphs {
        registry: std::ptr::from_ref(sources).addr(),
        nodes: Inventory::new(input, sources).paragraphs,
    }
}

/// The form identity of a body paragraph laid out under this source node.
pub(crate) fn body_form_paragraph(
    input: &crate::input::LayoutInput,
    node: Option<oxoffice_layout::SourceNodeId>,
) -> Option<FormParagraph> {
    if !input.form_fields {
        return None;
    }
    input.form_paragraphs.as_ref()?.nodes.get(&node?).copied()
}

/// The form identity of a table cell paragraph: only for the main story
/// and the registry the inventory used, since table layout serves headers,
/// notes and text boxes too.
pub(crate) fn cell_form_paragraph(
    input: &crate::input::LayoutInput,
    sources: Option<&crate::engine::SourceRegistry>,
    story: &crate::WordStory,
    node: Option<oxoffice_layout::SourceNodeId>,
) -> Option<FormParagraph> {
    let paragraphs = input.form_paragraphs.as_ref()?;
    if !input.form_fields
        || *story != crate::WordStory::Document
        || sources.map(|sources| std::ptr::from_ref(sources).addr()) != Some(paragraphs.registry)
    {
        return None;
    }
    paragraphs.nodes.get(&node?).copied()
}

/// The paragraph's alignment as a field's: start, centre or end, mirrored in
/// a right-to-left paragraph.
fn paragraph_alignment(input: &crate::input::LayoutInput, paragraph: &CT_P) -> FieldAlignment {
    use oxoffice_docx_oxml::shared::ST_Jc;
    let mut properties = crate::style_resolver::resolve_paragraph_properties(
        paragraph
            .properties
            .as_ref()
            .and_then(|properties| properties.style_id.as_deref()),
        &input.styles,
    );
    if let Some(direct) = &paragraph.properties {
        properties.merge_from(direct);
    }
    let right_to_left = properties.bidi == Some(true);
    match properties.jc {
        Some(ST_Jc::Center) => FieldAlignment::Center,
        Some(ST_Jc::Right) => FieldAlignment::Right,
        Some(ST_Jc::Left) => FieldAlignment::Left,
        Some(ST_Jc::End) if !right_to_left => FieldAlignment::Right,
        Some(ST_Jc::End) => FieldAlignment::Left,
        _ if right_to_left => FieldAlignment::Right,
        _ => FieldAlignment::Left,
    }
}

/// The box an element draws, as the numbering pass tests it against a
/// marker: text by its run of advances on its baseline, lines by their two
/// ends, rectangles and pictures whole.
fn extent(element: &oxoffice_layout::PositionedElement) -> Option<oxoffice_layout::Rect> {
    use oxoffice_layout::{PositionedElement as E, Rect};
    match element {
        E::Text(run) => Some(Rect {
            x: run.origin.x,
            y: run.origin.y,
            width: run.advances.iter().sum(),
            height: 0.0,
        }),
        E::MultilingualText(run) => Some(Rect {
            x: run.origin.x,
            y: run.origin.y,
            width: run.x_advances.iter().sum(),
            height: 0.0,
        }),
        E::Line { start, end, .. } => Some(Rect {
            x: start.x.min(end.x),
            y: start.y.min(end.y),
            width: (end.x - start.x).abs(),
            height: (end.y - start.y).abs(),
        }),
        E::FilledRect { rect, .. } | E::Image { rect, .. } => Some(*rect),
        E::MarkedContent { children, .. } if children.len() == 1 => extent(&children[0]),
        _ => None,
    }
}

/// Whether an element's box lies wholly inside a marker's box. A paragraph
/// or cell shading, a border, a leader outside the item, or a list number
/// is wider than any one item, so it stays on the page.
fn inside(rect: oxoffice_layout::Rect, extent: oxoffice_layout::Rect) -> bool {
    const EPSILON: f64 = 0.01;
    extent.x >= rect.x - EPSILON
        && extent.x + extent.width <= rect.x + rect.width + EPSILON
        && extent.y >= rect.y - EPSILON
        && extent.y + extent.height <= rect.y + rect.height + EPSILON
}

/// Whether a marker gathers an element. Text belongs to the item it starts
/// in, so a field result that grows when substituted after layout stays
/// with its field. A picture belongs only to the marker over that very
/// picture: a floating one that happens to lie over a field stays on the
/// page. Other paint must lie wholly inside the item's box.
fn gathers(
    rect: oxoffice_layout::Rect,
    node: oxoffice_layout::SourceNodeId,
    element: &oxoffice_layout::PositionedElement,
) -> bool {
    use oxoffice_layout::PositionedElement as E;
    const EPSILON: f64 = 0.01;
    // Text from another paragraph, such as a floating text box over the
    // field, is not the field's. Field results carry no source.
    let own = |source: Option<oxoffice_layout::SourceSpan>| {
        source.is_none_or(|source| source.node == node)
    };
    match element {
        E::MarkedContent { children, .. } if children.len() == 1 => {
            gathers(rect, node, &children[0])
        }
        E::Text(run) if !own(run.source) => false,
        E::MultilingualText(run) if !own(run.source) => false,
        E::Text(_) | E::MultilingualText(_) => extent(element).is_some_and(|start| {
            start.x >= rect.x - EPSILON
                && start.x < rect.x + rect.width - EPSILON
                && start.y >= rect.y - EPSILON
                && start.y <= rect.y + rect.height + EPSILON
        }),
        E::Image { rect: picture, .. } => {
            (picture.x - rect.x).abs() < EPSILON
                && (picture.y - rect.y).abs() < EPSILON
                && (picture.width - rect.width).abs() < EPSILON
                && (picture.height - rect.height).abs() < EPSILON
        }
        _ => extent(element).is_some_and(|extent| inside(rect, extent)),
    }
}

/// The box around a set of boxes.
fn union_rect(boxes: &[oxoffice_layout::Rect]) -> oxoffice_layout::Rect {
    let left = boxes
        .iter()
        .map(|rect| rect.x)
        .fold(f64::INFINITY, f64::min);
    let top = boxes
        .iter()
        .map(|rect| rect.y)
        .fold(f64::INFINITY, f64::min);
    let right = boxes
        .iter()
        .map(|rect| rect.x + rect.width)
        .fold(f64::NEG_INFINITY, f64::max);
    let bottom = boxes
        .iter()
        .map(|rect| rect.y + rect.height)
        .fold(f64::NEG_INFINITY, f64::max);
    oxoffice_layout::Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

/// Whether a box starts strictly inside another.
fn starts_inside(rect: oxoffice_layout::Rect, extent: oxoffice_layout::Rect) -> bool {
    const EPSILON: f64 = 0.01;
    extent.x > rect.x + EPSILON
        && extent.x < rect.x + rect.width - EPSILON
        && extent.y > rect.y + EPSILON
        && extent.y < rect.y + rect.height - EPSILON
}

/// The boxes of the text a page draws outside any fragment.
fn page_text_extents(
    elements: &[oxoffice_layout::PositionedElement],
) -> Vec<oxoffice_layout::Rect> {
    use oxoffice_layout::PositionedElement as E;
    let mut extents = Vec::new();
    for element in elements {
        match element {
            E::Text(run) if !run.glyph_ids.is_empty() => extents.extend(extent(element)),
            E::MultilingualText(_) => extents.extend(extent(element)),
            E::MarkedContent { children, .. } => extents.extend(page_text_extents(children)),
            _ => {}
        }
    }
    extents
}

/// The text runs among a fragment's children, wrappers looked through.
fn text_runs(elements: &[oxoffice_layout::PositionedElement]) -> Vec<&oxoffice_layout::GlyphRun> {
    use oxoffice_layout::PositionedElement as E;
    let mut runs = Vec::new();
    for element in elements {
        match element {
            E::Text(run) if !run.glyph_ids.is_empty() => runs.push(run),
            E::MarkedContent { children, .. } => runs.extend(text_runs(children)),
            _ => {}
        }
    }
    runs
}

/// Gather what each marker of one element list stands for into its fragment.
/// `figure` tells a figure's structure element: a picture tagged as a
/// figure stays on the page under the widget, so its element keeps content.
fn gather(
    elements: &mut Vec<oxoffice_layout::PositionedElement>,
    resolve: &dyn Fn(oxoffice_layout::FormMark) -> Option<u32>,
    figure: &dyn Fn(oxoffice_layout::StructureId) -> bool,
    tracks: &mut HashMap<u32, Vec<f64>>,
) {
    use oxoffice_layout::{FormFieldFragment, FormFieldId, PositionedElement as E};
    for element in elements.iter_mut() {
        match element {
            E::MarkedContent { children, .. } => gather(children, resolve, figure, tracks),
            E::Group(group) => gather(&mut group.children, resolve, figure, tracks),
            _ => {}
        }
    }
    for element in elements.iter() {
        if let E::FormField(FormFieldFragment {
            mark: Some(marker),
            children,
            ..
        }) = element
            && children.is_empty()
            && let Some(field) = resolve(marker.mark)
        {
            tracks.entry(field).or_default().push(marker.track);
        }
    }
    let markers: Vec<(
        usize,
        oxoffice_layout::Rect,
        Option<u32>,
        oxoffice_layout::Rect,
        oxoffice_layout::SourceNodeId,
    )> = elements
        .iter()
        .enumerate()
        .filter_map(|(index, element)| match element {
            E::FormField(FormFieldFragment {
                rect,
                mark: Some(marker),
                children,
                ..
            }) if children.is_empty() => Some((
                index,
                *rect,
                resolve(marker.mark),
                marker.band.unwrap_or(*rect),
                marker.mark.node,
            )),
            _ => None,
        })
        .collect();
    if markers.is_empty() {
        return;
    }
    let mut owner: Vec<Option<usize>> = vec![None; elements.len()];
    for (marker_index, (_, rect, field, _, node)) in markers.iter().enumerate() {
        if field.is_none() {
            continue;
        }
        for (index, element) in elements.iter().enumerate() {
            let tagged_figure =
                matches!(element, E::MarkedContent { structure: Some(id), .. } if figure(*id));
            let free = owner[index].is_none()
                && !tagged_figure
                && !matches!(element, E::FormField(_) | E::LinkAnnotation { .. });
            if free && gathers(*rect, *node, element) {
                owner[index] = Some(marker_index);
            }
        }
    }
    let mut fragments: Vec<Option<(FormFieldFragment, Option<oxoffice_layout::StructureId>)>> =
        markers
            .iter()
            .map(|(_, _, field, widget, _)| {
                field.map(|field| {
                    (
                        FormFieldFragment {
                            field: FormFieldId(field),
                            rect: *widget,
                            children: Vec::new(),
                            alternate: Vec::new(),
                            mark: None,
                        },
                        None,
                    )
                })
            })
            .collect();
    let mut first_capture: Vec<Option<usize>> = vec![None; markers.len()];
    let old = std::mem::take(elements);
    let mut kept: Vec<Option<E>> = Vec::with_capacity(old.len());
    for (index, element) in old.into_iter().enumerate() {
        if let Some(marker_index) = owner[index]
            && let Some((fragment, structure)) = fragments[marker_index].as_mut()
        {
            if let E::MarkedContent {
                structure: Some(id),
                ..
            } = &element
            {
                structure.get_or_insert(*id);
            }
            fragment.children.push(element);
            first_capture[marker_index].get_or_insert(kept.len());
            kept.push(None);
            continue;
        }
        if let Some(marker_index) = markers
            .iter()
            .position(|(marker, _, _, _, _)| *marker == index)
        {
            // A marker that captured nothing keeps its widget here: a
            // signature field, over a picture that stays on the page or over
            // nothing. Any other field that gathered nothing is made static
            // after gathering.
            if first_capture[marker_index].is_none() && fragments[marker_index].is_some() {
                first_capture[marker_index] = Some(kept.len());
                kept.push(None);
            }
            continue;
        }
        kept.push(Some(element));
    }
    let mut placed = vec![false; markers.len()];
    for (position, slot) in kept.into_iter().enumerate() {
        if let Some(element) = slot {
            elements.push(element);
            continue;
        }
        for marker_index in 0..markers.len() {
            if first_capture[marker_index] == Some(position) && !placed[marker_index] {
                placed[marker_index] = true;
                if let Some((fragment, structure)) = fragments[marker_index].take() {
                    let fragment = E::FormField(fragment);
                    elements.push(match structure {
                        Some(id) => E::MarkedContent {
                            structure: Some(id),
                            children: vec![fragment],
                        },
                        None => fragment,
                    });
                }
            }
        }
    }
}

/// Visit every fragment of a page, wrappers looked through.
fn fragments_mut(
    elements: &mut [oxoffice_layout::PositionedElement],
    f: &mut dyn FnMut(&mut oxoffice_layout::FormFieldFragment),
) {
    use oxoffice_layout::PositionedElement as E;
    for element in elements {
        match element {
            E::FormField(fragment) => f(fragment),
            E::MarkedContent { children, .. } => fragments_mut(children, f),
            E::Group(group) => fragments_mut(&mut group.children, f),
            _ => {}
        }
    }
}

/// Replace the fragments of `fields` by their children: those controls stay
/// static.
fn flatten_fields(
    elements: &mut Vec<oxoffice_layout::PositionedElement>,
    fields: &std::collections::HashSet<u32>,
) {
    use oxoffice_layout::PositionedElement as E;
    let old = std::mem::take(elements);
    for mut element in old {
        match &mut element {
            E::FormField(fragment) if fields.contains(&fragment.field.0) => {
                elements.append(&mut fragment.children);
                continue;
            }
            E::MarkedContent { children, .. } => flatten_fields(children, fields),
            E::Group(group) => flatten_fields(&mut group.children, fields),
            _ => {}
        }
        elements.push(element);
    }
}

/// Turn the markers of a laid-out document into form fields: gather what
/// each control drew into fragments, number the fields in document order,
/// and build `form_fields` from the model.
pub(crate) fn number_form_fields(
    result: &mut oxoffice_layout::LayoutResult,
    input: &crate::input::LayoutInput,
    sources: &crate::engine::SourceRegistry,
) {
    use oxoffice_layout::{Diagnostic, FormFieldKind};
    let inventory = Inventory::new(input, sources);
    let resolve = |mark: oxoffice_layout::FormMark| {
        inventory
            .keys
            .get(&mark)
            .and_then(|index| u32::try_from(*index).ok())
    };
    let figures: std::collections::HashSet<oxoffice_layout::StructureId> = result
        .structure
        .iter()
        .flat_map(|structure| &structure.nodes)
        .filter(|node| node.role == oxoffice_layout::StructureRole::Figure)
        .map(|node| node.id)
        .collect();
    let figure = |id: oxoffice_layout::StructureId| figures.contains(&id);
    let mut tracks: HashMap<u32, Vec<f64>> = HashMap::new();
    for page in &mut result.pages {
        let page = std::sync::Arc::make_mut(page);
        gather(&mut page.elements, &resolve, &figure, &mut tracks);
    }

    // Where each field draws, in drawing order: (page, fragment box), and
    // the fields whose fragments gathered something.
    let mut placed: HashMap<u32, Vec<(usize, oxoffice_layout::Rect)>> = HashMap::new();
    let mut gathered: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for (page_index, page) in result.pages.iter_mut().enumerate() {
        let page = std::sync::Arc::make_mut(page);
        fragments_mut(&mut page.elements, &mut |fragment| {
            placed
                .entry(fragment.field.0)
                .or_default()
                .push((page_index, fragment.rect));
            if !fragment.children.is_empty() {
                gathered.insert(fragment.field.0);
            }
        });
    }
    // A field drawn on more than one page, or in more than one column of
    // its page, stays static: one widget over both would cover what lies
    // between. In drawing order, a box above the one before it starts a new
    // column.
    // A field that gathered nothing would be a widget over what it left on
    // the page. A signature field is the exception: it is empty by design,
    // over a picture that stays on the page or over nothing.
    let split: HashMap<u32, &str> = placed
        .iter()
        .filter_map(|(field, places)| {
            let columns = tracks
                .get(field)
                .is_some_and(|tracks| tracks.iter().any(|track| (track - tracks[0]).abs() > 0.01));
            let signature = inventory
                .entries
                .get(*field as usize)
                .is_some_and(|entry| matches!(entry.field.kind, FormFieldKind::Signature));
            if places.iter().any(|(page, _)| *page != places[0].0) {
                Some((*field, "it is split across pages"))
            } else if columns
                || places
                    .windows(2)
                    .any(|pair| pair[1].1.y + pair[1].1.height <= pair[0].1.y + 0.01)
            {
                Some((*field, "it is split across columns"))
            } else if !gathered.contains(field) && !signature {
                Some((*field, "what it draws could not be gathered"))
            } else {
                None
            }
        })
        .collect();
    if !split.is_empty() {
        let split: std::collections::HashSet<u32> = split.keys().copied().collect();
        for page in &mut result.pages {
            flatten_fields(&mut std::sync::Arc::make_mut(page).elements, &split);
        }
    }
    // How many lines each field's widget spans, and the fields under whose
    // widget other text of the page lies: text before a wrapped inline
    // control on its first line, after it on its last, or between pieces.
    let mut lines_of: HashMap<u32, usize> = HashMap::new();
    let mut covering: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for (field, places) in &placed {
        if split.contains_key(field) {
            continue;
        }
        let mut boxes: Vec<oxoffice_layout::Rect> = places.iter().map(|(_, rect)| *rect).collect();
        boxes.sort_by(|a, b| a.y.total_cmp(&b.y));
        let mut lines = 0;
        let mut bottom = f64::NEG_INFINITY;
        for rect in &boxes {
            if rect.y >= bottom - 0.01 {
                lines += 1;
            }
            bottom = bottom.max(rect.y + rect.height);
        }
        lines_of.insert(*field, lines);
        // Several boxes make one widget over their union: wrapped lines, or
        // pieces that bidi reordering set apart on one line.
        if boxes.len() > 1 {
            let union = union_rect(&boxes);
            let page = &result.pages[places[0].0];
            if page_text_extents(&page.elements)
                .iter()
                .any(|extent| starts_inside(union, *extent))
            {
                covering.insert(*field);
            }
        }
    }

    let mut fields: Vec<FormField> = inventory
        .entries
        .iter()
        .map(|entry| entry.field.clone())
        .collect();
    let names = unique_names(
        &fields
            .iter()
            .map(|field| field.name.clone())
            .collect::<Vec<_>>(),
    );
    for (field, name) in fields.iter_mut().zip(names) {
        field.name = name;
    }
    for (index, field) in fields.iter_mut().enumerate() {
        let lines = lines_of.get(&(index as u32)).copied().unwrap_or(0);
        if let FormFieldKind::Text { multiline, .. } = &mut field.kind
            && lines > 1
        {
            *multiline = true;
        }
    }
    let fonts = result.fonts.clone();
    let mut drawn = std::collections::HashSet::new();
    for page in &mut result.pages {
        let page = std::sync::Arc::make_mut(page);
        fragments_mut(&mut page.elements, &mut |fragment| {
            let index = fragment.field.0 as usize;
            let Some(field) = fields.get_mut(index) else {
                return;
            };
            let runs = text_runs(&fragment.children);
            if let Some(run) = runs.first()
                && !drawn.contains(&index)
            {
                field.font_size = run.font_size;
                field.color = run.color;
            }
            drawn.insert(index);
            if let FormFieldKind::CheckBox { checked } = field.kind {
                let other = inventory.entries[index].other_symbol(checked);
                checkbox_fragment(fragment, other, &fonts);
            }
        });
    }
    for (index, entry) in inventory.entries.iter().enumerate() {
        if drawn.contains(&index) {
            continue;
        }
        let reason = split
            .get(&(index as u32))
            .copied()
            .unwrap_or("nothing it draws could be placed");
        result.diagnostics.push(Diagnostic {
            message: format!(
                "{} {:?} stays static: {reason}",
                entry.kind(),
                entry.field.name
            ),
        });
    }
    for field in &covering {
        result.diagnostics.push(Diagnostic {
            message: format!(
                "{} {:?}: other text on its lines sits under its box",
                inventory.entries[*field as usize].kind(),
                fields[*field as usize].name
            ),
        });
    }
    result
        .diagnostics
        .extend(inventory.faults.iter().map(|message| Diagnostic {
            message: message.clone(),
        }));
    result.form_fields = fields;
}

/// A check box's widget covers its symbol, and its other state draws the
/// other symbol in the same font.
fn checkbox_fragment(
    fragment: &mut oxoffice_layout::FormFieldFragment,
    other: Option<char>,
    fonts: &[oxoffice_layout::FontData],
) {
    let Some(run) = text_runs(&fragment.children)
        .first()
        .map(|run| (*run).clone())
    else {
        return;
    };
    let width: f64 = run.advances.iter().sum();
    fragment.rect = oxoffice_layout::Rect {
        x: run.origin.x,
        width,
        ..fragment.rect
    };
    let (Some(other), Some(font)) = (other, fonts.iter().find(|font| font.id == run.font_id))
    else {
        return;
    };
    let Ok(face) = ttf_parser::Face::parse(&font.data, font.face_index) else {
        return;
    };
    let Some(glyph) = face.glyph_index(other) else {
        return;
    };
    let advance = face.glyph_hor_advance(glyph).map_or(width, |advance| {
        f64::from(advance) * run.font_size / f64::from(face.units_per_em())
    });
    fragment.alternate = vec![oxoffice_layout::PositionedElement::Text(
        oxoffice_layout::GlyphRun {
            glyph_ids: vec![glyph.0],
            advances: vec![advance],
            text: other.to_string(),
            source: None,
            ..run
        },
    )];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_tags_need_the_exact_word_or_a_colon() {
        assert!(is_signature_tag(Some("signature")));
        assert!(is_signature_tag(Some("Signature:Client")));
        assert!(!is_signature_tag(Some("signature_date")));
        assert!(!is_signature_tag(Some("sign")));
        assert!(!is_signature_tag(None));
    }

    #[test]
    fn names_are_repaired_and_made_unique_in_document_order() {
        let names: Vec<String> = ["a", "a", "a_2", "x.y", ""]
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        assert_eq!(unique_names(&names), ["a", "a_3", "a_2", "x_y", "field"]);
    }
}
