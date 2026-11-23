//! AcroForm fields from the layout's form field fragments.
//!
//! A field's widget goes on the first page that holds one of its fragments,
//! over the union of its fragments on that page. Its normal appearance is
//! what the layout drew for those fragments, which then leaves the page
//! content. Fragments on later pages are drawn as page content.

use std::collections::{BTreeMap, BTreeSet};

use oxoffice_layout::bundled_fonts::LIBERATION_SANS_REGULAR;
use oxoffice_layout::{
    FieldAlignment, FormField, FormFieldFragment, FormFieldId, FormFieldKind, PositionedElement,
    Rect, Transform,
};
use pdf_writer::types::{
    AnnotationFlags, CheckBoxState, CidFontType, FieldFlags, FieldType, FontFlags, SystemInfo,
    UnicodeCmap,
};
use pdf_writer::{Filter, Finish, Name, Pdf, Ref, Str, TextStr};

/// The `/DR` font key that `/DA` strings name.
const FORM_FONT: &str = "LibSans";

/// Visit every form field fragment in depth-first order, with the transform
/// from its coordinates to the page's.
pub(crate) fn visit_fragments<'a>(
    elements: &'a [PositionedElement],
    f: &mut impl FnMut(&'a FormFieldFragment, Transform),
) {
    fn visit<'a>(
        elements: &'a [PositionedElement],
        accumulated: Transform,
        f: &mut impl FnMut(&'a FormFieldFragment, Transform),
    ) {
        for element in elements {
            match element {
                PositionedElement::Group(group) => {
                    visit(&group.children, group.transform.then(accumulated), f);
                }
                PositionedElement::MarkedContent { children, .. } => {
                    visit(children, accumulated, f);
                }
                PositionedElement::FormField(fragment) => f(fragment, accumulated),
                _ => {}
            }
        }
    }
    visit(elements, Transform::IDENTITY, f);
}

/// One field with a widget, and the objects that write it.
pub(crate) struct PlacedField<'a> {
    pub(crate) field: &'a FormField,
    pub(crate) name: String,
    pub(crate) page_index: usize,
    /// Union of the field's fragments on its page, in page coordinates from
    /// the top-left corner.
    pub(crate) rect: Rect,
    pub(crate) widget: Ref,
    /// The normal appearance, the `/Yes` state of a check box.
    pub(crate) appearance: Ref,
    /// The `/Off` state of a check box.
    pub(crate) off_appearance: Option<Ref>,
    /// In tagged output, the widget's `/Form` element and parent tree key.
    pub(crate) tag: Option<(Ref, i32)>,
}

impl PlacedField<'_> {
    /// Whether the appearance draws the fragments. A placeholder and a
    /// signature show nothing until they are filled.
    pub(crate) fn shows_drawing(&self) -> bool {
        !self.field.placeholder && !matches!(self.field.kind, FormFieldKind::Signature)
    }

    pub(crate) fn is_check_box(&self) -> bool {
        matches!(self.field.kind, FormFieldKind::CheckBox { .. })
    }
}

/// What the page emission collects for one placed field.
#[derive(Default)]
pub(crate) struct FieldDrawing {
    /// The fragments' children, in page coordinates.
    pub(crate) current: Vec<u8>,
    /// A check box's other state.
    pub(crate) other: Vec<u8>,
}

/// Every field with a widget, in field order, and the typing font.
pub(crate) struct FormPlan<'a> {
    pub(crate) fields: Vec<PlacedField<'a>>,
    by_id: BTreeMap<FormFieldId, usize>,
    font: Ref,
    cid_font: Ref,
    to_unicode: Ref,
    font_descriptor: Ref,
    font_file: Ref,
}

impl<'a> FormPlan<'a> {
    /// Place the fields, or `None` when no fragment names a field. References
    /// are allocated only then, so output without fields keeps its objects.
    pub(crate) fn new(
        pages: &[std::sync::Arc<oxoffice_layout::PageFrame>],
        fields: &'a [FormField],
        tagged: bool,
        alloc: &mut impl FnMut() -> Ref,
    ) -> Option<Self> {
        let mut placements: BTreeMap<FormFieldId, (usize, Rect)> = BTreeMap::new();
        for (page_index, page) in pages.iter().enumerate() {
            visit_fragments(&page.elements, &mut |fragment, transform| {
                if fragment.field.0 as usize >= fields.len() || !rect_is_finite(fragment.rect) {
                    return;
                }
                let rect = transform.transform_rect_bbox(fragment.rect);
                placements
                    .entry(fragment.field)
                    .and_modify(|(placed_page, union)| {
                        if *placed_page == page_index {
                            *union = union_rect(*union, rect);
                        }
                    })
                    .or_insert((page_index, rect));
            });
        }
        if placements.is_empty() {
            return None;
        }

        let names = unique_names(placements.keys().map(|id| &fields[id.0 as usize].name));
        let font = alloc();
        let cid_font = alloc();
        let to_unicode = alloc();
        let font_descriptor = alloc();
        let font_file = alloc();
        let mut by_id = BTreeMap::new();
        let placed = placements
            .into_iter()
            .zip(names)
            .enumerate()
            .map(|(index, ((id, (page_index, rect)), name))| {
                by_id.insert(id, index);
                let field = &fields[id.0 as usize];
                let widget = alloc();
                let appearance = alloc();
                let off_appearance =
                    matches!(field.kind, FormFieldKind::CheckBox { .. }).then(&mut *alloc);
                // Parent tree keys after the pages' own.
                let tag = tagged.then(|| (alloc(), (pages.len() + index) as i32));
                PlacedField {
                    field,
                    name,
                    page_index,
                    rect,
                    widget,
                    appearance,
                    off_appearance,
                    tag,
                }
            })
            .collect();
        Some(Self {
            fields: placed,
            by_id,
            font,
            cid_font,
            to_unicode,
            font_descriptor,
            font_file,
        })
    }

    /// The field placed on this page, if the fragment's field is.
    pub(crate) fn placed_on(&self, id: FormFieldId, page_index: usize) -> Option<&PlacedField<'a>> {
        self.by_id
            .get(&id)
            .map(|index| &self.fields[*index])
            .filter(|placed| placed.page_index == page_index)
    }

    /// Widget references of one page, in field order.
    pub(crate) fn widgets_on(&self, page_index: usize) -> impl Iterator<Item = Ref> + '_ {
        self.fields
            .iter()
            .filter(move |placed| placed.page_index == page_index)
            .map(|placed| placed.widget)
    }

    /// The widgets the structure tree tags, in field order.
    pub(crate) fn tagged_widgets(&self) -> Vec<crate::structure::TaggedWidget> {
        self.by_id
            .iter()
            .filter_map(|(id, index)| {
                let placed = &self.fields[*index];
                placed
                    .tag
                    .map(|(element, key)| crate::structure::TaggedWidget {
                        field: *id,
                        page_index: placed.page_index,
                        widget: placed.widget,
                        element,
                        key,
                    })
            })
            .collect()
    }

    /// Write `/AcroForm` into the catalog.
    pub(crate) fn write_catalog_entry(&self, catalog: &mut pdf_writer::writers::Catalog<'_>) {
        let mut form = catalog.form();
        form.fields(self.fields.iter().map(|placed| placed.widget));
        form.default_resources()
            .fonts()
            .pair(Name(FORM_FONT.as_bytes()), self.font);
        form.default_appearance(Str(format!("/{FORM_FONT} 0 Tf 0 g").as_bytes()));
    }

    /// Write every field with its widget and appearances, and the typing font.
    ///
    /// `drawings` holds what the page emission collected per field;
    /// `resources` writes a page's resource dictionary for an appearance.
    pub(crate) fn write(
        &self,
        pdf: &mut Pdf,
        page_ids: &[Ref],
        page_heights: &[f64],
        drawings: &BTreeMap<FormFieldId, FieldDrawing>,
        resources: &mut dyn FnMut(usize, &mut pdf_writer::writers::Resources<'_>),
    ) {
        let empty = FieldDrawing::default();
        for (id, index) in &self.by_id {
            let placed = &self.fields[*index];
            let drawing = drawings.get(id).unwrap_or(&empty);
            let page_height = page_heights[placed.page_index];
            let bbox = pdf_rect(placed.rect, page_height);

            let (on, off) = match placed.field.kind {
                FormFieldKind::CheckBox { checked: true } => (&drawing.current, &drawing.other),
                FormFieldKind::CheckBox { checked: false } => (&drawing.other, &drawing.current),
                _ if placed.shows_drawing() => (&drawing.current, &empty.other),
                _ => (&empty.current, &empty.other),
            };
            write_appearance(
                pdf,
                placed.appearance,
                on,
                bbox,
                placed.page_index,
                resources,
            );
            if let Some(off_ref) = placed.off_appearance {
                write_appearance(pdf, off_ref, off, bbox, placed.page_index, resources);
            }

            let mut field = pdf.form_field(placed.widget);
            field.partial_name(TextStr(&placed.name));
            // Tagged output needs a description on every field, and the
            // name is the last one left.
            match (&placed.field.tooltip, placed.tag) {
                (Some(tooltip), _) => {
                    field.alternate_name(TextStr(tooltip));
                }
                (None, Some(_)) => {
                    field.alternate_name(TextStr(&placed.name));
                }
                (None, None) => {}
            }
            let mut flags = FieldFlags::empty();
            if placed.field.read_only {
                flags |= FieldFlags::READ_ONLY;
            }
            let appearance = default_appearance(placed.field);
            match &placed.field.kind {
                FormFieldKind::Text {
                    value,
                    multiline,
                    max_len,
                } => {
                    field.field_type(FieldType::Text);
                    if *multiline {
                        flags |= FieldFlags::MULTILINE;
                    }
                    if !value.is_empty() && !placed.field.placeholder {
                        field.text_value(TextStr(value));
                    }
                    if let Some(max_len) = max_len {
                        field.text_max_len(i32::try_from(*max_len).unwrap_or(i32::MAX));
                    }
                    field.vartext_default_appearance(Str(appearance.as_bytes()));
                    field.vartext_quadding(quadding(placed.field.alignment));
                }
                FormFieldKind::CheckBox { checked } => {
                    field.field_type(FieldType::Button);
                    field.checkbox_value(if *checked {
                        CheckBoxState::Yes
                    } else {
                        CheckBoxState::Off
                    });
                }
                FormFieldKind::Choice {
                    options,
                    value,
                    editable,
                } => {
                    field.field_type(FieldType::Choice);
                    flags |= FieldFlags::COMBO;
                    if *editable {
                        flags |= FieldFlags::EDIT;
                    }
                    {
                        let mut list = field.choice_options();
                        for option in options {
                            if option.value == option.display {
                                list.option(TextStr(&option.display));
                            } else {
                                list.export(TextStr(&option.display), TextStr(&option.value));
                            }
                        }
                    }
                    if let Some(value) = value.as_deref().filter(|_| !placed.field.placeholder) {
                        field.choice_value(Some(TextStr(value)));
                    }
                    field.vartext_default_appearance(Str(appearance.as_bytes()));
                    field.vartext_quadding(quadding(placed.field.alignment));
                }
                FormFieldKind::Signature => {
                    field.field_type(FieldType::Signature);
                }
            }
            if !flags.is_empty() {
                field.field_flags(flags);
            }

            let mut widget = field.into_annotation();
            widget.rect(bbox);
            widget.flags(AnnotationFlags::PRINT);
            widget.page(page_ids[placed.page_index]);
            if let Some((_, key)) = placed.tag {
                widget.struct_parent(key);
            }
            if let Some(off_ref) = placed.off_appearance {
                let checked =
                    matches!(placed.field.kind, FormFieldKind::CheckBox { checked: true });
                {
                    let mut appearance = widget.appearance();
                    let mut states = appearance.normal().streams();
                    states.pair(Name(b"Off"), off_ref);
                    states.pair(Name(b"Yes"), placed.appearance);
                }
                widget.appearance_state(Name(if checked { b"Yes" } else { b"Off" }));
            } else {
                widget.appearance().normal().stream(placed.appearance);
            }
            widget.finish();
        }
        self.write_font(pdf);
    }

    /// Liberation Sans Regular as a Type0 font, Identity-H, the whole file
    /// embedded with every glyph's width and Unicode value: a viewer types any
    /// character the font has, and a subset could not type new ones.
    fn write_font(&self, pdf: &mut Pdf) {
        let face = typing_face();
        let scale = 1000.0 / f32::from(face.units_per_em());
        let to_pdf = |value: i16| f32::from(value) * scale;

        pdf.type0_font(self.font)
            .base_font(Name(b"LiberationSans"))
            .encoding_predefined(Name(b"Identity-H"))
            .descendant_font(self.cid_font)
            .to_unicode(self.to_unicode);

        let system_info = SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"Identity"),
            supplement: 0,
        };
        let mut cid = pdf.cid_font(self.cid_font);
        cid.subtype(CidFontType::Type2);
        cid.base_font(Name(b"LiberationSans"));
        cid.system_info(system_info);
        cid.font_descriptor(self.font_descriptor);
        cid.default_width(0.0);
        cid.cid_to_gid_map_predefined(Name(b"Identity"));
        cid.widths().consecutive(
            0,
            (0..face.number_of_glyphs()).map(|glyph| {
                face.glyph_hor_advance(ttf_parser::GlyphId(glyph))
                    .map_or(0.0, |advance| f32::from(advance) * scale)
            }),
        );
        cid.finish();

        // Each glyph's lowest code point, so a viewer reverses the map to
        // encode what a user types.
        let mut unicode: BTreeMap<u16, char> = BTreeMap::new();
        if let Some(cmap) = face.tables().cmap {
            for subtable in cmap
                .subtables
                .into_iter()
                .filter(|table| table.is_unicode())
            {
                subtable.codepoints(|codepoint| {
                    let (Some(ch), Some(glyph)) =
                        (char::from_u32(codepoint), subtable.glyph_index(codepoint))
                    else {
                        return;
                    };
                    if glyph.0 != 0 {
                        unicode
                            .entry(glyph.0)
                            .and_modify(|held| *held = (*held).min(ch))
                            .or_insert(ch);
                    }
                });
            }
        }
        let mut cmap = UnicodeCmap::new(Name(b"Adobe-Identity-UCS"), system_info);
        for (glyph, ch) in unicode {
            cmap.pair(glyph, ch);
        }
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&cmap.finish(), 6);
        pdf.stream(self.to_unicode, &compressed)
            .filter(Filter::FlateDecode);

        let bbox = face.global_bounding_box();
        let mut descriptor = pdf.font_descriptor(self.font_descriptor);
        descriptor.name(Name(b"LiberationSans"));
        descriptor.flags(FontFlags::NON_SYMBOLIC);
        descriptor.bbox(pdf_writer::Rect::new(
            to_pdf(bbox.x_min),
            to_pdf(bbox.y_min),
            to_pdf(bbox.x_max),
            to_pdf(bbox.y_max),
        ));
        descriptor.italic_angle(face.italic_angle());
        descriptor.ascent(to_pdf(face.ascender()));
        descriptor.descent(to_pdf(face.descender()));
        descriptor.cap_height(to_pdf(face.capital_height().unwrap_or(face.ascender())));
        descriptor.stem_v(80.0);
        descriptor.font_file2(self.font_file);
        descriptor.finish();

        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(LIBERATION_SANS_REGULAR, 6);
        let mut file = pdf.stream(self.font_file, &compressed);
        file.filter(Filter::FlateDecode);
        file.pair(Name(b"Length1"), LIBERATION_SANS_REGULAR.len() as i32);
        file.finish();
    }
}

/// One message per field a user can type into whose text the typing font
/// cannot encode, naming the field as the PDF does and the text's scripts.
///
/// Viewers rebuild a field's appearance with the typing font once it is
/// edited: text in those scripts may then show nothing, or be saved without
/// an appearance that other viewers draw.
pub fn untypable_form_text(layout: &oxoffice_layout::LayoutResult) -> Vec<String> {
    if layout.form_fields.is_empty() {
        return Vec::new();
    }
    let mut next = 0;
    let Some(plan) = FormPlan::new(&layout.pages, &layout.form_fields, false, &mut || {
        next += 1;
        Ref::new(next)
    }) else {
        return Vec::new();
    };
    let mut messages: Vec<String> = Vec::new();
    for placed in &plan.fields {
        if placed.field.read_only {
            continue;
        }
        let texts: Vec<&str> = match &placed.field.kind {
            FormFieldKind::Text { value, .. } if !placed.field.placeholder => vec![value],
            // A chosen option shows its label; a value no option holds is
            // text typed into an editable list.
            FormFieldKind::Choice { options, value, .. } => options
                .iter()
                .map(|option| option.display.as_str())
                .chain(value.as_deref().filter(|value| {
                    !placed.field.placeholder
                        && !options.iter().any(|option| option.value == *value)
                }))
                .collect(),
            _ => Vec::new(),
        };
        let characters: BTreeSet<char> = texts
            .iter()
            .flat_map(|text| text.chars())
            .filter(|ch| !typing_font_encodes(*ch))
            .collect();
        if characters.is_empty() {
            continue;
        }
        let scripts: BTreeSet<&str> = characters.iter().map(|ch| script_name(*ch)).collect();
        let sample = sample_of(&characters);
        let count = match characters.len() {
            1 => "1 character".to_owned(),
            count => format!("{count} distinct characters"),
        };
        let message = format!(
            "form field \"{}\" holds {} text its typing font (Liberation Sans) has no \
             glyph for ({count}, such as {sample}): once the field is edited, a PDF viewer may \
             show nothing or save it without an appearance",
            placed.name,
            scripts.into_iter().collect::<Vec<_>>().join(", "),
        );
        if !messages.contains(&message) {
            messages.push(message);
        }
    }
    messages
}

/// Whether the typing font encodes `ch`: a character it has a glyph for,
/// or a line break or tab a multiline field holds.
fn typing_font_encodes(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\t')
        || typing_face()
            .glyph_index(ch)
            .is_some_and(|glyph| glyph.0 != 0)
}

/// The typing font, Liberation Sans Regular, parsed once.
fn typing_face() -> &'static ttf_parser::Face<'static> {
    static FACE: std::sync::OnceLock<ttf_parser::Face<'static>> = std::sync::OnceLock::new();
    FACE.get_or_init(|| {
        ttf_parser::Face::parse(LIBERATION_SANS_REGULAR, 0)
            .expect("the bundled Liberation Sans parses")
    })
}

/// A character's Unicode script name, or what it is when it has none.
fn script_name(ch: char) -> &'static str {
    use unicode_script::{Script, UnicodeScript};
    match ch.script() {
        Script::Common => "symbol",
        Script::Inherited => "combining mark",
        Script::Unknown => "unassigned",
        script => script.full_name(),
    }
}

/// Up to eight of `characters`, the visible ones quoted, the others as
/// `U+XXXX` so that a space or a combining mark is not quoted blank.
fn sample_of(characters: &BTreeSet<char>) -> String {
    use unicode_script::{Script, UnicodeScript};
    let mut visible = String::new();
    let mut named = Vec::new();
    for &ch in characters.iter().take(8) {
        let invisible = ch.is_whitespace()
            || ch.is_control()
            || ch.script() == Script::Inherited
            || matches!(ch, '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}');
        if invisible {
            named.push(format!("U+{:04X}", u32::from(ch)));
        } else {
            visible.push(ch);
        }
    }
    let mut parts = Vec::new();
    if !visible.is_empty() {
        parts.push(format!("\"{visible}\""));
    }
    parts.extend(named);
    parts.join(", ")
}

fn write_appearance(
    pdf: &mut Pdf,
    id: Ref,
    content: &[u8],
    bbox: pdf_writer::Rect,
    page_index: usize,
    resources: &mut dyn FnMut(usize, &mut pdf_writer::writers::Resources<'_>),
) {
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(content, 6);
    let mut xobject = pdf.form_xobject(id, &compressed);
    xobject.filter(Filter::FlateDecode);
    xobject.bbox(bbox);
    resources(page_index, &mut xobject.resources());
    xobject.finish();
}

/// `/DA`: the typing font at the field's size and colour.
fn default_appearance(field: &FormField) -> String {
    let size = if field.font_size.is_finite() && field.font_size > 0.0 {
        field.font_size
    } else {
        0.0
    };
    let channel = |value: f64| {
        if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    format!(
        "/{FORM_FONT} {} Tf {} {} {} rg",
        number(size),
        number(channel(field.color.r)),
        number(channel(field.color.g)),
        number(channel(field.color.b)),
    )
}

/// A number with at most three decimals and no trailing zeros.
fn number(value: f64) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn quadding(alignment: FieldAlignment) -> pdf_writer::types::Quadding {
    match alignment {
        FieldAlignment::Left => pdf_writer::types::Quadding::Left,
        FieldAlignment::Center => pdf_writer::types::Quadding::Center,
        FieldAlignment::Right => pdf_writer::types::Quadding::Right,
    }
}

/// Names a PDF accepts, in field order: `.` separates name levels, so it
/// becomes `_`; an empty name becomes `field`; a name already used takes the
/// next suffix `_2`, `_3`... that no field holds.
fn unique_names<'n>(names: impl Iterator<Item = &'n String>) -> Vec<String> {
    let repaired: Vec<String> = names
        .map(|name| {
            if name.is_empty() {
                "field".to_owned()
            } else {
                name.replace('.', "_")
            }
        })
        .collect();
    let held: BTreeSet<&str> = repaired.iter().map(String::as_str).collect();
    let mut used: BTreeSet<String> = BTreeSet::new();
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

fn rect_is_finite(rect: Rect) -> bool {
    [rect.x, rect.y, rect.width, rect.height]
        .into_iter()
        .all(f64::is_finite)
}

fn union_rect(a: Rect, b: Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect {
        x,
        y,
        width: (a.x + a.width).max(b.x + b.width) - x,
        height: (a.y + a.height).max(b.y + b.height) - y,
    }
}

/// A top-left page rectangle in PDF coordinates.
pub(crate) fn pdf_rect(rect: Rect, page_height: f64) -> pdf_writer::Rect {
    pdf_writer::Rect::new(
        rect.x as f32,
        (page_height - rect.y - rect.height) as f32,
        (rect.x + rect.width) as f32,
        (page_height - rect.y) as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(input: &[&str]) -> Vec<String> {
        let owned: Vec<String> = input.iter().map(|name| (*name).to_owned()).collect();
        unique_names(owned.iter())
    }

    #[test]
    fn duplicate_names_take_the_next_suffix_no_field_holds() {
        assert_eq!(names(&["a", "a", "a_2"]), ["a", "a_3", "a_2"]);
        assert_eq!(names(&["a", "a", "a"]), ["a", "a_2", "a_3"]);
    }

    #[test]
    fn dots_and_empty_names_are_repaired() {
        assert_eq!(names(&["a.b", "a_b", ""]), ["a_b", "a_b_2", "field"]);
    }

    #[test]
    fn the_typing_font_encodes_what_liberation_sans_draws_and_line_breaks() {
        for ch in [
            'A', 'é', '€', 'Ł', 'č', 'ğ', 'Ж', 'Ω', 'ש', '\u{2013}', '\n', '\t',
        ] {
            assert!(typing_font_encodes(ch), "{ch:?}");
        }
        for ch in ['中', 'م', 'क', 'ก', '\u{7}'] {
            assert!(!typing_font_encodes(ch), "{ch:?}");
        }
        assert_eq!(script_name('Ж'), "Cyrillic");
        assert_eq!(script_name('→'), "symbol");
        assert_eq!(script_name('\u{0301}'), "combining mark");
    }

    /// The ToUnicode map keeps one character per glyph, so the diagnostic,
    /// which accepts any character the face maps, holds only while no two
    /// characters share a glyph.
    #[test]
    fn the_typing_font_maps_each_glyph_from_one_character() {
        let face = typing_face();
        let mut glyphs: BTreeMap<u16, u32> = BTreeMap::new();
        let mut shared = Vec::new();
        for subtable in face.tables().cmap.unwrap().subtables {
            if !subtable.is_unicode() {
                continue;
            }
            subtable.codepoints(|codepoint| {
                if let Some(glyph) = subtable.glyph_index(codepoint).filter(|glyph| glyph.0 != 0)
                    && let Some(held) = glyphs.insert(glyph.0, codepoint)
                    && held != codepoint
                {
                    shared.push((held, codepoint));
                }
            });
        }
        assert!(glyphs.len() > 2000);
        assert!(shared.is_empty(), "{shared:x?}");
    }

    #[test]
    fn a_sample_names_invisible_characters_by_code_point() {
        let characters: BTreeSet<char> = ['Ж', '\u{202F}', '\u{200B}', '\u{0301}'].into();
        assert_eq!(sample_of(&characters), "\"Ж\", U+0301, U+200B, U+202F");
        let visible: BTreeSet<char> = ['中', 'Ж'].into();
        assert_eq!(sample_of(&visible), "\"Ж中\"");
    }
}
