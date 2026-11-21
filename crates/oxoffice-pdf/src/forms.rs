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
use pdf_writer::types::{AnnotationFlags, CheckBoxState, FieldFlags, FieldType, FontFlags};
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

    /// Liberation Sans Regular as a simple TrueType font with WinAnsi
    /// encoding, the whole file embedded: a subset would drop the `cmap` a
    /// simple font needs to draw what a user types.
    fn write_font(&self, pdf: &mut Pdf) {
        let face = ttf_parser::Face::parse(LIBERATION_SANS_REGULAR, 0)
            .expect("the bundled Liberation Sans parses");
        let scale = 1000.0 / f32::from(face.units_per_em());
        let to_pdf = |value: i16| f32::from(value) * scale;
        let widths: Vec<f32> = (32u8..=255)
            .map(|code| {
                win_ansi_char(code)
                    .and_then(|ch| face.glyph_index(ch))
                    .and_then(|glyph| face.glyph_hor_advance(glyph))
                    .map_or(0.0, |advance| f32::from(advance) * scale)
            })
            .collect();

        let mut font = pdf.indirect(self.font).dict();
        font.pair(Name(b"Type"), Name(b"Font"));
        font.pair(Name(b"Subtype"), Name(b"TrueType"));
        font.pair(Name(b"BaseFont"), Name(b"LiberationSans"));
        font.pair(Name(b"FirstChar"), 32);
        font.pair(Name(b"LastChar"), 255);
        font.insert(Name(b"Widths")).array().items(widths);
        font.pair(Name(b"Encoding"), Name(b"WinAnsiEncoding"));
        font.pair(Name(b"FontDescriptor"), self.font_descriptor);
        font.finish();

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

/// The character a WinAnsi code stands for, `None` for its five holes.
fn win_ansi_char(code: u8) -> Option<char> {
    const HIGH: [u16; 32] = [
        0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
        0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
        0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
    ];
    match code {
        0x80..=0x9F => match HIGH[usize::from(code - 0x80)] {
            0 => None,
            unicode => char::from_u32(u32::from(unicode)),
        },
        0x7F => None,
        _ => Some(char::from(code)),
    }
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
    fn win_ansi_maps_its_high_codes_and_leaves_its_holes() {
        assert_eq!(win_ansi_char(0x80), Some('€'));
        assert_eq!(win_ansi_char(0x81), None);
        assert_eq!(win_ansi_char(0x41), Some('A'));
        assert_eq!(win_ansi_char(0xE9), Some('é'));
    }
}
